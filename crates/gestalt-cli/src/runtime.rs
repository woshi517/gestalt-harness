use crate::config::EffectiveConfig;
use gestalt_core::error::HarnessError;
use gestalt_core::tool::ToolCatalog;
use gestalt_runtime::{AgentRuntime, AgentRuntimeBuilder, RuntimeConfig};
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
    let policy = Arc::new(crate::run::build_policy(config)?);
    let approval = approval_override.unwrap_or_else(|| crate::run::approval_provider(mode));

    let model = if resolved_provider.model.is_empty() {
        provider_default_model
    } else {
        resolved_provider.model
    };

    #[allow(unused_mut)]
    let mut enabled_cli_features = Vec::new();
    #[cfg(feature = "tui")]
    enabled_cli_features.push("tui".to_string());
    #[cfg(feature = "mcp")]
    enabled_cli_features.push("mcp".to_string());
    #[cfg(feature = "otel")]
    enabled_cli_features.push("otel".to_string());

    let explicit_loads: Vec<std::path::PathBuf> = config
        .extensions
        .explicit_loads
        .iter()
        .map(|s| std::path::PathBuf::from(s))
        .collect();

    let global_dir = dirs::config_dir().map(|d| d.join("gestalt"));
    let discovery =
        gestalt_runtime::ExtensionDiscovery::new(config.workspace_root.clone(), global_dir);

    let mut trusted_extension_ids: Vec<String> = config.extensions.trusted.clone();

    if let Ok(discovered) = discovery.discover_all(&explicit_loads) {
        for ext in &discovered {
            if config.extensions.disabled.contains(&ext.manifest.id) {
                continue;
            }

            let is_explicit = explicit_loads.iter().any(|p| {
                p == &ext.manifest_path || p.parent() == Some(&ext.manifest_path)
            });
            let is_trusted = is_explicit 
                || config.extensions.trusted.contains(&ext.manifest.id)
                || config.extensions.allow_untrusted;

            if is_trusted && !trusted_extension_ids.contains(&ext.manifest.id) {
                trusted_extension_ids.push(ext.manifest.id.clone());
            }
        }
    }

    let runtime_config = RuntimeConfig {
        workspace_root: config.workspace_root.clone(),
        execution_mode: mode,
        max_turns,
        model,
        provider: resolved_provider.provider_name.clone(),
        max_tokens: 4096,
        temperature: Some(0.0),
        max_context_window: config.context.max_context_window,
        reserved_output_tokens: config.context.reserved_output_tokens,
        bash_timeout_secs: config.tools.bash_timeout_secs,
        max_output_tokens: config.tools.max_output_tokens,
        allow_network: false,
        environment: std::collections::HashMap::new(),
        enabled_cli_features,
        tool_profile: None,
        trusted_extension_ids,
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
        .middleware(pipeline)
        .policy(policy)
        .approval(approval)
        .config(runtime_config)
        .hooks(core_hooks);

    if let Some(ref sink) = trace_sink {
        builder = builder.trace_sink(sink.clone());
    }

    if let Ok(discovered) = discovery.discover_all(&explicit_loads) {
        for ext in discovered {
            if config.extensions.disabled.contains(&ext.manifest.id) {
                continue;
            }

            let is_explicit = explicit_loads.iter().any(|p| {
                p == &ext.manifest_path || p.parent() == Some(&ext.manifest_path)
            });
            let is_trusted = is_explicit 
                || config.extensions.trusted.contains(&ext.manifest.id)
                || config.extensions.allow_untrusted;

            let is_project_local = ext.manifest_path.starts_with(config.workspace_root.join(".gestalt/extensions"));
            
            if is_project_local && !is_trusted {
                builder.event_bus.publish(gestalt_runtime::RuntimeEvent::ExtensionRejected {
                    extension_id: ext.manifest.id.clone(),
                    reason: "Untrusted project extension ignored. Enable it by adding its ID to 'extensions.trusted' in config.toml".to_string(),
                });
                continue;
            }

            let broker_res = gestalt_runtime::ProcessExtensionBroker::spawn(
                ext.manifest.clone(),
                builder.event_bus.clone(),
            )
            .await;

            match broker_res {
                Ok(broker) => {
                    let wrapped_ext = Arc::new(gestalt_runtime::ProcessExtension::new(
                        ext.manifest,
                        Arc::new(broker),
                    ));
                    builder = builder.extension(wrapped_ext);
                }
                Err(e) => {
                    builder.event_bus.publish(gestalt_runtime::RuntimeEvent::ExtensionRejected {
                        extension_id: ext.manifest.id.clone(),
                        reason: format!("Startup failure: {}", e),
                    });
                }
            }
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

    builder.build().map_err(|e| match e {
        gestalt_runtime::RuntimeError::Harness(he) => he,
        other => HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
            field: "runtime".to_string(),
            reason: other.to_string(),
        }),
    })
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
    ).await?;
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
    let config_path = workspace_root.join(".gestalt/config.toml");
    let mut doc = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse::<toml_edit::DocumentMut>()?
    } else {
        toml_edit::DocumentMut::new()
    };

    if let Some(extensions) = doc.get_mut("extensions") {
        if let Some(disabled) = extensions
            .get_mut("disabled")
            .and_then(|v| v.as_array_mut())
        {
            let mut index_to_remove = None;
            for (i, val) in disabled.iter().enumerate() {
                if val.as_str() == Some(id) {
                    index_to_remove = Some(i);
                    break;
                }
            }
            if let Some(idx) = index_to_remove {
                disabled.remove(idx);
            }
        }
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, doc.to_string())?;
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
    let config_path = workspace_root.join(".gestalt/config.toml");
    let mut doc = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse::<toml_edit::DocumentMut>()?
    } else {
        toml_edit::DocumentMut::new()
    };

    let extensions = doc
        .entry("extensions")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let table = extensions
        .as_table_mut()
        .ok_or("extensions is not a table")?;
    let disabled =
        table
            .entry("disabled")
            .or_insert(toml_edit::Item::Value(toml_edit::Value::Array(
                toml_edit::Array::new(),
            )));

    if let Some(arr) = disabled.as_array_mut() {
        let mut exists = false;
        for val in arr.iter() {
            if val.as_str() == Some(id) {
                exists = true;
                break;
            }
        }
        if !exists {
            arr.push(id);
        }
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, doc.to_string())?;
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
    ).await?;
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
