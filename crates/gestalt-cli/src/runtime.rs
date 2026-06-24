use crate::config::{mutate_workspace_config_file, workspace_config_path, EffectiveConfig};
use gestalt_core::error::HarnessError;
use gestalt_core::tool::ToolCatalog;
use gestalt_runtime::{AgentRuntime, AgentRuntimeBuilder, RuntimeConfig};
use std::collections::BTreeMap;
use std::sync::Arc;

#[allow(clippy::missing_errors_doc, clippy::needless_pass_by_value)]
pub async fn build_cli_runtime(
    config: &EffectiveConfig,
    api_key: Option<String>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<gestalt_core::AgentEvent>>,
    approval_override: Option<Arc<dyn gestalt_core::ApprovalProvider>>,
    trace_sink: Option<Arc<dyn gestalt_core::trace::TraceSink>>,
) -> Result<AgentRuntime, HarnessError> {
    let resolved_provider = config.resolve_provider()?;
    let resolver = crate::auth::build_credential_resolver(api_key, event_tx.is_none());
    let provider = gestalt_models::registry::get_with_resolver(
        &resolved_provider.kind,
        resolved_provider.provider_json(),
        resolver,
    )?;
    let provider_default_model = provider.default_model().to_string();

    let tools = Arc::new(gestalt_tools::default_registry()?);
    let mode = config.selected_mode()?;
    let max_turns = config.max_turns();
    let tool_names: Vec<String> = tools
        .schemas()
        .iter()
        .filter_map(|s| s.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let pipeline = Arc::new(crate::run::build_pipeline(
        config,
        mode,
        max_turns,
        &tool_names,
    )?);
    let policy = Arc::new(crate::run::build_policy(config));
    let approval = approval_override.unwrap_or_else(|| crate::run::approval_provider(mode));

    let model = if resolved_provider.model.is_empty() {
        provider_default_model
    } else {
        resolved_provider.model
    };

    let enabled_cli_features = vec![
        #[cfg(feature = "tui")]
        "tui".to_string(),
        "mcp".to_string(),
        #[cfg(feature = "otel")]
        "otel".to_string(),
    ];

    let explicit_loads: Vec<std::path::PathBuf> = config
        .extensions
        .explicit_loads
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();

    let global_dir = dirs::config_dir().map(|d| d.join("gestalt"));
    let discovery =
        gestalt_runtime::ExtensionDiscovery::new(config.workspace_root.clone(), global_dir);

    let mut trusted_extension_ids: Vec<String> = Vec::new();

    if let Ok(discovered) = discovery.discover_all(&explicit_loads) {
        for ext in &discovered {
            if config.extensions.disabled.contains(&ext.manifest.id) {
                continue;
            }

            let mut is_trusted = false;
            for trusted_entry in &config.extensions.trusted {
                if trusted_entry == &ext.manifest.id {
                    is_trusted = true;
                    break;
                } else if trusted_entry.starts_with(&format!("{}:", ext.manifest.id)) {
                    let parts: Vec<&str> = trusted_entry.splitn(2, ':').collect();
                    if parts.len() == 2 {
                        let expected_hash = parts[1];
                        if expected_hash == ext.manifest_hash {
                            is_trusted = true;
                        }
                    }
                    break;
                }
            }

            if is_trusted && !trusted_extension_ids.contains(&ext.manifest.id) {
                trusted_extension_ids.push(ext.manifest.id.clone());
            }
        }
    }

    // Skill discovery
    let skill_explicit: Vec<std::path::PathBuf> = config
        .skills
        .explicit_paths
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();
    let skill_discovery = build_skill_discovery(config);
    let discovered_skills = skill_discovery
        .discover_all(&skill_explicit)
        .unwrap_or_default();

    // Fail-fast: reject unknown or untrusted skill names that were passed via
    // `--skill`, slash command, or gestalt.json. This converts a class of
    // silent-drop failures (where an unknown name was accepted and then
    // filtered out at runtime) into a clear, deterministic error.
    let trusted_names: std::collections::HashSet<&str> =
        config.skills.trusted.iter().map(String::as_str).collect();
    for name in &config.skills.active {
        let Some(desc) = discovered_skills.iter().find(|skill| skill.name == *name) else {
            return Err(HarnessError::Config(
                gestalt_core::ConfigError::InvalidValue {
                    field: "skills.active".to_string(),
                    reason: format!(
                        "Unknown skill '{name}'. Use `gestalt skill list` to see available skills."
                    ),
                },
            ));
        };
        let trusted = matches!(
            desc.trust_level,
            gestalt_skills::SkillTrustLevel::Explicit | gestalt_skills::SkillTrustLevel::Workspace
        ) || trusted_names.contains(name.as_str());
        if !trusted {
            return Err(HarnessError::Config(
                gestalt_core::ConfigError::InvalidValue {
                    field: "skills.active".to_string(),
                    reason: format!(
                        "Skill '{name}' is at trust level {:?} and is not in `skills.trusted`. Add it to `skills.trusted` in gestalt.json to allow activation.",
                        desc.trust_level
                    ),
                },
            ));
        }
    }
    let active_skills = config.skills.active.clone();

    // Publish skill discovery events
    let (mcp_servers, mcp_discovery_threshold) = if let Some(mcp) = &config.mcp {
        (mcp.servers.clone(), mcp.discovery_threshold)
    } else {
        (std::collections::HashMap::new(), Some(5))
    };

    let mut meta_map = serde_json::Map::new();
    if let Some(ref thinking) = resolved_provider.resolved_options.thinking {
        meta_map.insert(
            "thinking".to_string(),
            serde_json::to_value(thinking).unwrap_or_default(),
        );
    }
    if let Some(ref adapter_opts) = resolved_provider.resolved_options.adapter_options {
        for (k, v) in adapter_opts {
            meta_map.insert(k.clone(), v.clone());
        }
    }
    let metadata = serde_json::Value::Object(meta_map);

    fn to_core_reasoning_effort(
        value: crate::config::ReasoningEffort,
    ) -> gestalt_core::provider::ReasoningEffort {
        match value {
            crate::config::ReasoningEffort::None => gestalt_core::provider::ReasoningEffort::None,
            crate::config::ReasoningEffort::Low => gestalt_core::provider::ReasoningEffort::Low,
            crate::config::ReasoningEffort::Medium => {
                gestalt_core::provider::ReasoningEffort::Medium
            }
            crate::config::ReasoningEffort::High => gestalt_core::provider::ReasoningEffort::High,
            crate::config::ReasoningEffort::Xhigh => gestalt_core::provider::ReasoningEffort::Xhigh,
        }
    }

    fn to_core_text_verbosity(
        value: crate::config::TextVerbosity,
    ) -> gestalt_core::provider::TextVerbosity {
        match value {
            crate::config::TextVerbosity::None => gestalt_core::provider::TextVerbosity::None,
            crate::config::TextVerbosity::Low => gestalt_core::provider::TextVerbosity::Low,
            crate::config::TextVerbosity::Medium => gestalt_core::provider::TextVerbosity::Medium,
            crate::config::TextVerbosity::High => gestalt_core::provider::TextVerbosity::High,
        }
    }

    let mut context_management_policy = config.context.management.clone().unwrap_or_default();
    if let Some(buffer_tokens) = config.context.safety_margin_tokens {
        context_management_policy.buffer_tokens = buffer_tokens;
    }

    let runtime_config = RuntimeConfig {
        workspace_root: config.workspace_root.clone(),
        execution_mode: mode,
        max_turns,
        model,
        provider: resolved_provider.provider_name.clone(),
        max_tokens: resolved_provider
            .resolved_options
            .max_output_tokens
            .unwrap_or(4096),
        temperature: resolved_provider.resolved_options.temperature,
        max_context_window: config.context.max_context_window,
        reserved_output_tokens: config.context.reserved_output_tokens,
        context_management_policy: Some(context_management_policy),
        bash_timeout_secs: config.tools.bash_timeout_secs,
        max_output_tokens: config.tools.max_output_tokens,
        allow_network: false,
        environment: std::collections::HashMap::new(),
        enabled_cli_features,
        tool_profile: None,
        trusted_extension_ids,
        discovered_skills: discovered_skills.clone(),
        active_skills: active_skills.clone(),
        mcp_servers,
        mcp_discovery_threshold,
        ignore_patterns: config.tools.ignore_patterns.clone().unwrap_or_default(),
        top_p: resolved_provider.resolved_options.top_p,
        reasoning_effort: resolved_provider
            .resolved_options
            .reasoning_effort
            .map(to_core_reasoning_effort),
        text_verbosity: resolved_provider
            .resolved_options
            .text_verbosity
            .map(to_core_text_verbosity),
        metadata,
        extension_timeouts: gestalt_runtime::config::ExtensionTimeoutsConfig {
            initialize_ms: config.extensions.timeouts.initialize_ms,
            hook_ms: config.extensions.timeouts.hook_ms,
            context_ms: config.extensions.timeouts.context_ms,
            tool_ms: config.extensions.timeouts.tool_ms,
            shutdown_ms: config.extensions.timeouts.shutdown_ms,
        },
        extension_limits: gestalt_runtime::config::ExtensionLimitsConfig {
            max_message_bytes: config.extensions.limits.max_message_bytes,
            max_pending_requests: config.extensions.limits.max_pending_requests,
            max_protocol_errors: config.extensions.limits.max_protocol_errors,
        },
        extension_instances: convert_extension_instances(&config.extensions.instances),
        effective_config_fingerprint: Some(config.compute_fingerprint()),
    };

    let mut verifier_registry = gestalt_verify::VerifierRegistry::new();
    verifier_registry.register(Box::new(gestalt_verify::FileExistsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::NoSecretsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::PatchAppliesVerifier));
    verifier_registry.register(Box::new(gestalt_verify::MarkdownStructureVerifier));
    verifier_registry.register(Box::new(gestalt_verify::CommandVerifier::new(
        "echo 'Command verified'",
    )));

    let verification_hook = Arc::new(gestalt_verify::VerificationToolHook::new(verifier_registry));
    let evaluator = Arc::new(gestalt_trace::evaluator::NoopTraceEvaluator);

    let mut core_hooks = gestalt_core::HookRegistry::new();
    core_hooks.register_tool_hook(verification_hook);

    if let Some(ref sink) = trace_sink {
        let sink_clone = sink.clone();
        let evaluator_hook = Arc::new(
            gestalt_trace::evaluator::EvaluatorHook::new(evaluator, None).with_flush_trigger(
                Arc::new(move || {
                    let _ = sink_clone.flush();
                }),
            ),
        );
        core_hooks.register_session_hook(evaluator_hook);
    }

    let mut builder = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(tools.clone())
        .assembler(pipeline)
        .policy(policy.clone())
        .approval(approval)
        .config(runtime_config)
        .hooks(core_hooks);

    let workspace_cfg = config.context.workspace.clone().unwrap_or_default();
    let memory_cfg = config.context.memory.clone().unwrap_or_default();

    let (ws_contrib, mem_contrib, ws_snapshot) =
        gestalt_runtime::workspace_context::load_and_snapshot_workspace_context(
            &config.workspace_root,
            Some(policy.clone() as Arc<dyn gestalt_core::policy::PolicyEngine>),
            &builder.event_bus,
            &workspace_cfg,
            &memory_cfg,
        )
        .await
        .map_err(|e| {
            gestalt_core::error::HarnessError::Config(
                gestalt_core::error::ConfigError::InvalidValue {
                    field: "workspace_context".to_string(),
                    reason: e.to_string(),
                },
            )
        })?;

    if let Some(contrib) = ws_contrib {
        builder
            .registry
            .register_context_contributor(
                "00_workspace_instructions".to_string(),
                Arc::new(contrib),
            )
            .map_err(|e| {
                gestalt_core::error::HarnessError::Config(
                    gestalt_core::error::ConfigError::InvalidValue {
                        field: "registry".to_string(),
                        reason: e.to_string(),
                    },
                )
            })?;
    }

    if let Some(contrib) = mem_contrib {
        builder
            .registry
            .register_context_contributor("01_markdown_memory".to_string(), Arc::new(contrib))
            .map_err(|e| {
                gestalt_core::error::HarnessError::Config(
                    gestalt_core::error::ConfigError::InvalidValue {
                        field: "registry".to_string(),
                        reason: e.to_string(),
                    },
                )
            })?;
    }

    builder = builder.workspace_context_snapshot(ws_snapshot);

    if let Some(ref sink) = trace_sink {
        builder = builder.trace_sink(sink.clone());
    }

    // Publish skill discovery events
    for skill in &discovered_skills {
        builder
            .event_bus
            .publish(gestalt_runtime::RuntimeEvent::SkillDiscovered {
                skill_name: skill.name.clone(),
                manifest_hash: skill.manifest_hash.clone(),
                source: format!("{:?}", skill.source),
                trust_level: format!("{:?}", skill.trust_level),
            });
    }

    if let Ok(discovered) = discovery.discover_all(&explicit_loads) {
        for ext in discovered {
            if config.extensions.disabled.contains(&ext.manifest.id) {
                continue;
            }

            let mut is_trusted_by_config = false;
            for trusted_entry in &config.extensions.trusted {
                if trusted_entry == &ext.manifest.id {
                    eprintln!(
                        "Warning: Extension '{}' is trusted using a legacy ID-only entry. Please migrate to integrity-aware trust by specifying '{}:{}'",
                        ext.manifest.id, ext.manifest.id, ext.manifest_hash
                    );
                    is_trusted_by_config = true;
                    break;
                } else if trusted_entry.starts_with(&format!("{}:", ext.manifest.id)) {
                    let parts: Vec<&str> = trusted_entry.splitn(2, ':').collect();
                    if parts.len() == 2 {
                        let expected_hash = parts[1];
                        if expected_hash == ext.manifest_hash {
                            is_trusted_by_config = true;
                        } else {
                            eprintln!(
                                "Warning: Extension '{}' trust invalid: manifest integrity hash has changed (expected '{}', found '{}')",
                                ext.manifest.id, expected_hash, ext.manifest_hash
                            );
                        }
                    }
                    break;
                }
            }

            if !is_trusted_by_config && !config.extensions.allow_untrusted {
                builder.event_bus.publish(gestalt_runtime::RuntimeEvent::ExtensionRejected {
                    extension_id: ext.manifest.id.clone(),
                    reason: "Untrusted extension ignored. Enable it by adding its ID/hash to 'extensions.trusted' in gestalt.json or setting 'extensions.allow_untrusted' to true.".to_string(),
                });
                continue;
            }

            builder =
                builder.process_extension(ext.manifest, ext.manifest_hash, is_trusted_by_config);
        }
    }

    // Register tools in registry
    let tool_schemas = tools.schemas();
    for schema in &tool_schemas {
        if let Some(name) = schema.get("name").and_then(|v| v.as_str()) {
            let _ = builder
                .registry
                .register_tool(name.to_string(), schema.clone());
        }
    }

    // Register verifiers in registry
    let _ = builder
        .registry
        .register_verifier("FileExistsVerifier".to_string());
    let _ = builder
        .registry
        .register_verifier("NoSecretsVerifier".to_string());
    let _ = builder
        .registry
        .register_verifier("PatchAppliesVerifier".to_string());
    let _ = builder
        .registry
        .register_verifier("MarkdownStructureVerifier".to_string());
    let _ = builder
        .registry
        .register_verifier("CommandVerifier".to_string());

    // Register hooks in registry
    let _ = builder
        .registry
        .register_hook("VerificationToolHook".to_string());
    if trace_sink.is_some() {
        let _ = builder.registry.register_hook("EvaluatorHook".to_string());
    }

    if let Some(sink) = trace_sink {
        builder = builder.trace_sink(sink);
    }

    builder.build_async().await.map_err(|e| match e {
        gestalt_runtime::RuntimeError::Harness(he) => he,
        other => HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
            field: "runtime".to_string(),
            reason: other.to_string(),
        }),
    })
}

fn convert_extension_instances(
    instances: &BTreeMap<String, crate::config::ExtensionInstanceConfig>,
) -> BTreeMap<String, gestalt_runtime::extension::ExtensionInstanceConfig> {
    instances
        .iter()
        .map(|(id, instance)| {
            (
                id.clone(),
                gestalt_runtime::extension::ExtensionInstanceConfig {
                    package: instance.package.clone(),
                    enabled: instance.enabled,
                    components: instance.components.clone(),
                    config: instance.config.clone(),
                    grants: gestalt_runtime::extension::ExtensionGrantConfig {
                        workspace_read: instance.grants.workspace_read,
                        workspace_write: instance.grants.workspace_write,
                        network: instance.grants.network.clone(),
                    },
                },
            )
        })
        .collect()
}

#[allow(clippy::missing_errors_doc)]
pub async fn inspect_runtime(
    overrides: &crate::config::CliOverrides,
    api_key: Option<String>,
) -> Result<gestalt_runtime::RuntimeInspect, Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    let runtime = build_cli_runtime(
        &config,
        api_key,
        None,
        None,
        Some(Arc::new(gestalt_core::trace::NullTraceSink)),
    )
    .await?;
    Ok(runtime.inspect())
}

pub fn enable_extension(
    overrides: &crate::config::CliOverrides,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir()?);
    let config_path = workspace_config_path(&workspace_root);
    mutate_workspace_config_file(&config_path, |config| {
        if let Some(extensions) = config.extensions.as_mut() {
            extensions.disabled.retain(|ext| ext != id);
        }
    })?;
    Ok(())
}

pub fn disable_extension(
    overrides: &crate::config::CliOverrides,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir()?);
    let config_path = workspace_config_path(&workspace_root);
    mutate_workspace_config_file(&config_path, |config| {
        let extensions = config.extensions.get_or_insert_with(Default::default);
        if !extensions.disabled.iter().any(|ext| ext == id) {
            extensions.disabled.push(id.to_string());
        }
    })?;
    Ok(())
}

pub fn validate_extension(
    path: &std::path::Path,
) -> Result<gestalt_runtime::ExtensionManifest, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let manifest = gestalt_runtime::ExtensionManifest::parse(&content)?;
    manifest.validate(true)?;
    Ok(manifest)
}

pub fn inspect_extension(
    overrides: &crate::config::CliOverrides,
    id: &str,
) -> Result<Option<gestalt_runtime::ExtensionManifest>, Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    let explicit_loads: Vec<std::path::PathBuf> = config
        .extensions
        .explicit_loads
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();
    let global_dir = dirs::config_dir().map(|d| d.join("gestalt"));
    let discovery =
        gestalt_runtime::ExtensionDiscovery::new(config.workspace_root.clone(), global_dir);
    let discovered = discovery.discover_all(&explicit_loads)?;
    for ext in discovered {
        if ext.manifest.id == id {
            return Ok(Some(ext.manifest));
        }
    }
    Ok(None)
}

pub fn list_extensions(
    overrides: &crate::config::CliOverrides,
) -> Result<Vec<gestalt_runtime::DiscoveredExtension>, Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    let explicit_loads: Vec<std::path::PathBuf> = config
        .extensions
        .explicit_loads
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();
    let global_dir = dirs::config_dir().map(|d| d.join("gestalt"));
    let discovery =
        gestalt_runtime::ExtensionDiscovery::new(config.workspace_root.clone(), global_dir);
    let mut discovered = discovery.discover_all(&explicit_loads)?;
    for ext in &mut discovered {
        if config.extensions.disabled.contains(&ext.manifest.id) {
            ext.enabled = false;
        }
    }
    Ok(discovered)
}

pub async fn get_runtime_events(
    overrides: &crate::config::CliOverrides,
    api_key: Option<String>,
) -> Result<Vec<gestalt_runtime::RuntimeEvent>, Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    let runtime = build_cli_runtime(
        &config,
        api_key,
        None,
        None,
        Some(Arc::new(gestalt_core::trace::NullTraceSink)),
    )
    .await?;
    let events = runtime.event_bus.history();
    Ok(events)
}

pub fn runtime_doctor(
    overrides: &crate::config::CliOverrides,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut checks = Vec::new();
    let config = crate::config::load_effective_config(overrides)?;
    let explicit_loads: Vec<std::path::PathBuf> = config
        .extensions
        .explicit_loads
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();
    let global_dir = dirs::config_dir().map(|d| d.join("gestalt"));
    let discovery =
        gestalt_runtime::ExtensionDiscovery::new(config.workspace_root.clone(), global_dir);

    if let Ok(discovered) = discovery.discover_all(&explicit_loads) {
        checks.push(format!("Discovered {} extension(s).", discovered.len()));
        let mut seen_ids = std::collections::HashMap::new();
        for ext in discovered {
            let manifest_path_str = ext.manifest_path.to_string_lossy().to_string();

            if let Err(e) = ext.manifest.validate(true) {
                checks.push(format!(
                    "ERROR: Manifest validation failed for '{}' at {}: {}",
                    ext.manifest.id, manifest_path_str, e
                ));
            } else {
                checks.push(format!("OK: '{}' manifest is valid.", ext.manifest.id));
            }

            if let Some(prev_path) =
                seen_ids.insert(ext.manifest.id.clone(), manifest_path_str.clone())
            {
                checks.push(format!(
                    "ERROR: Duplicate extension ID '{}' found at {} and {}.",
                    ext.manifest.id, prev_path, manifest_path_str
                ));
            }

            let cmd = &ext.manifest.entrypoint.command;
            let path_exists = std::path::Path::new(cmd).exists();
            if !path_exists && !cmd.contains('/') {
                checks.push(format!(
                    "INFO: '{}' uses system command '{}'. Ensure it is in PATH.",
                    ext.manifest.id, cmd
                ));
            } else if !path_exists {
                checks.push(format!(
                    "WARNING: Command path '{}' for extension '{}' does not exist.",
                    cmd, ext.manifest.id
                ));
            } else {
                checks.push(format!("OK: Command path '{}' exists.", cmd));
            }

            if ext.manifest.permissions.allow_shell {
                checks.push(format!("WARNING: Extension '{}' requests shell execution permission. Use with caution.", ext.manifest.id));
            }
            if ext.manifest.permissions.allow_all_paths {
                checks.push(format!(
                    "WARNING: Extension '{}' requests access to all files. Use with caution.",
                    ext.manifest.id
                ));
            }
        }
    } else {
        checks.push("ERROR: Failed to run extension discovery.".to_string());
    }

    Ok(checks)
}

// === Skill surface ===

pub fn build_skill_discovery(config: &EffectiveConfig) -> gestalt_skills::SkillDiscovery {
    let global_dir = if std::env::var_os("GESTALT_NO_GLOBAL_SKILLS").is_some() {
        None
    } else {
        dirs::config_dir().map(|d| d.join("gestalt"))
    };
    let home_dir = if std::env::var_os("GESTALT_NO_GLOBAL_SKILLS").is_some() {
        None
    } else {
        dirs::home_dir()
    };
    gestalt_skills::SkillDiscovery::new(config.workspace_root.clone(), global_dir, home_dir)
}

#[allow(clippy::missing_errors_doc)]
pub fn list_skills(
    overrides: &crate::config::CliOverrides,
) -> Result<Vec<crate::output::SkillListEntry>, Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    let explicit: Vec<std::path::PathBuf> = config
        .skills
        .explicit_paths
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();
    let discovery = build_skill_discovery(&config);
    let discovered = discovery.discover_all(&explicit)?;
    let mut entries = Vec::new();
    for skill in discovered {
        entries.push(crate::output::SkillListEntry {
            name: skill.name,
            description: skill.description,
            trust_level: format!("{:?}", skill.trust_level),
            source: format!("{:?}", skill.source),
            manifest_path: skill.manifest_path.to_string_lossy().to_string(),
        });
    }
    Ok(entries)
}

#[allow(clippy::missing_errors_doc)]
pub fn inspect_skill(
    overrides: &crate::config::CliOverrides,
    name: &str,
) -> Result<Option<gestalt_skills::SkillDescriptor>, Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    let explicit: Vec<std::path::PathBuf> = config
        .skills
        .explicit_paths
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();
    let discovery = build_skill_discovery(&config);
    let discovered = discovery.discover_all(&explicit)?;
    for skill in discovered {
        if skill.name == name {
            return Ok(Some(skill));
        }
    }
    Ok(None)
}

#[allow(clippy::missing_errors_doc)]
pub fn validate_skill(
    path: &std::path::Path,
) -> Result<gestalt_skills::manifest::SkillManifest, Box<dyn std::error::Error>> {
    let manifest_path = if path.is_dir() {
        path.join("SKILL.md")
    } else {
        path.to_path_buf()
    };
    let raw = std::fs::read_to_string(&manifest_path)?;
    let file = gestalt_skills::manifest::SkillManifest::parse(&raw)?;
    let dir_name = manifest_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    file.manifest.validate(dir_name)?;
    Ok(file.manifest)
}

/// Validate a skill activation request against the current discovery set.
///
/// Activation is rejected when:
/// * the skill name is not present in the discovered skill index, or
/// * the skill is below the user's configured trust threshold for auto-activation
///   (i.e. `Downloaded` skills require explicit `skills.trusted` listing).
///
/// Returns a structured `SkillValidation` describing what was found and what
/// was rejected, so callers (CLI, slash command, chat) can render a consistent
/// error or success message.
pub fn validate_skill_activation(config: &EffectiveConfig, name: &str) -> SkillValidation {
    let skill_explicit: Vec<std::path::PathBuf> = config
        .skills
        .explicit_paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let discovery = build_skill_discovery(config);
    let discovered = discovery.discover_all(&skill_explicit).unwrap_or_default();
    let trust_list: std::collections::HashSet<String> =
        config.skills.trusted.iter().cloned().collect();

    let descriptor = discovered.iter().find(|s| s.name == name).cloned();
    match descriptor {
        None => SkillValidation::Unknown {
            name: name.to_string(),
        },
        Some(desc) => {
            let trusted = matches!(
                desc.trust_level,
                gestalt_skills::SkillTrustLevel::Explicit
                    | gestalt_skills::SkillTrustLevel::Workspace
            ) || trust_list.contains(&desc.name);
            if trusted {
                SkillValidation::Ok {
                    descriptor: Box::new(desc),
                }
            } else {
                SkillValidation::Untrusted {
                    name: name.to_string(),
                    trust_level: desc.trust_level,
                }
            }
        }
    }
}

/// Outcome of validating a skill activation request.
#[derive(Debug, Clone)]
pub enum SkillValidation {
    /// Skill was found and trusted.
    Ok {
        descriptor: Box<gestalt_skills::SkillDescriptor>,
    },
    /// Skill name was not present in the discovered set.
    Unknown { name: String },
    /// Skill was found but its trust level is below the threshold for the
    /// current activation request (e.g. `Downloaded` skill not in
    /// `skills.trusted`).
    Untrusted {
        name: String,
        trust_level: gestalt_skills::SkillTrustLevel,
    },
}

impl SkillValidation {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    pub fn render_error(&self) -> Option<String> {
        match self {
            Self::Ok { .. } => None,
            Self::Unknown { name } => Some(format!(
                "Unknown skill '{name}'. Use `gestalt skill list` to see available skills."
            )),
            Self::Untrusted { name, trust_level } => Some(format!(
                "Skill '{name}' is at trust level {trust_level:?} and is not in `skills.trusted`. \
                 Add it to `skills.trusted` in gestalt.json to allow activation."
            )),
        }
    }
}

#[allow(clippy::missing_errors_doc)]
pub fn activate_skill(
    overrides: &crate::config::CliOverrides,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    match validate_skill_activation(&config, name) {
        SkillValidation::Ok { .. } => {}
        SkillValidation::Unknown { .. } | SkillValidation::Untrusted { .. } => {
            // Persist the validation result so downstream consumers see the
            // same error. We use a guard to write the failed-validation state
            // into the workspace config so that subsequent resume / inspect
            // surfaces know the activation was rejected.
            return Err(validate_skill_activation(&config, name)
                .render_error()
                .unwrap_or_else(|| "unknown error".to_string())
                .into());
        }
    }
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir()?);
    let config_path = crate::config::workspace_config_path(&workspace_root);
    crate::config::mutate_workspace_config_file(&config_path, |config| {
        let skills = config.skills.get_or_insert_with(Default::default);
        if !skills.active.iter().any(|s| s == name) {
            skills.active.push(name.to_string());
        }
    })?;
    Ok(())
}

#[allow(clippy::missing_errors_doc)]
pub fn deactivate_skill(
    overrides: &crate::config::CliOverrides,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = crate::config::load_effective_config(overrides)?;
    if matches!(
        validate_skill_activation(&config, name),
        SkillValidation::Unknown { .. }
    ) {
        return Err(format!(
            "Cannot deactivate unknown skill '{name}'. Use `gestalt skill list` to see available skills."
        )
        .into());
    }
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir()?);
    let config_path = crate::config::workspace_config_path(&workspace_root);
    crate::config::mutate_workspace_config_file(&config_path, |config| {
        if let Some(skills) = config.skills.as_mut() {
            skills.active.retain(|s| s != name);
        }
    })?;
    Ok(())
}
