use std::sync::Arc;
use gestalt_core::error::HarnessError;
use gestalt_core::tool::ToolCatalog;
use gestalt_runtime::{AgentRuntime, AgentRuntimeBuilder, RuntimeConfig};
use crate::config::EffectiveConfig;

#[allow(clippy::missing_errors_doc, clippy::needless_pass_by_value)]
pub fn build_cli_runtime(
    config: &EffectiveConfig,
    api_key: Option<String>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<gestalt_core::AgentEvent>>,
    approval_override: Option<Arc<dyn gestalt_core::ApprovalProvider>>,
    trace_sink: Option<Arc<dyn gestalt_core::trace::TraceSink>>,
) -> Result<AgentRuntime, HarnessError> {
    let resolved = config.resolve_provider()?;
    let resolver = crate::auth::build_credential_resolver(api_key, event_tx.is_none());
    let provider = gestalt_models::registry::get_with_resolver(&resolved.kind, resolved.provider_json(), resolver)?;
    let provider_default_model = provider.default_model().to_string();

    let tools = Arc::new(gestalt_tools::default_registry()?);
    let mode = config.selected_mode()?;
    let max_turns = config.max_turns();
    let tool_names: Vec<String> = tools
        .schemas()
        .iter()
        .filter_map(|s| s.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let pipeline = Arc::new(crate::run::build_pipeline(config, mode, max_turns, &tool_names)?);
    let policy = Arc::new(crate::run::build_policy(config)?);
    let approval = approval_override.unwrap_or_else(|| crate::run::approval_provider(mode));

    let model = if resolved.model.is_empty() {
        provider_default_model
    } else {
        resolved.model
    };

    #[allow(unused_mut)]
    let mut enabled_cli_features = Vec::new();
    #[cfg(feature = "tui")]
    enabled_cli_features.push("tui".to_string());
    #[cfg(feature = "mcp")]
    enabled_cli_features.push("mcp".to_string());
    #[cfg(feature = "otel")]
    enabled_cli_features.push("otel".to_string());

    let runtime_config = RuntimeConfig {
        workspace_root: config.workspace_root.clone(),
        execution_mode: mode,
        max_turns,
        model,
        provider: resolved.provider_name.clone(),
        max_tokens: 4096,
        temperature: Some(0.0),
        max_context_window: config.context.max_context_window,
        reserved_output_tokens: config.context.reserved_output_tokens,
        bash_timeout_secs: config.tools.bash_timeout_secs,
        max_output_tokens: config.tools.max_output_tokens,
        allow_network: false,
        environment: std::collections::HashMap::new(),
        enabled_cli_features,
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
            gestalt_trace::evaluator::EvaluatorHook::new(evaluator, None).with_flush_trigger(Arc::new(
                move || {
                    let _ = sink_clone.flush();
                },
            )),
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

    // Register tools in registry
    let tool_schemas = tools.schemas();
    for schema in &tool_schemas {
        if let Some(name) = schema.get("name").and_then(|v| v.as_str()) {
            let _ = builder.registry.register_tool(name.to_string(), schema.clone());
        }
    }

    // Register verifiers in registry
    let _ = builder.registry.register_verifier("FileExistsVerifier".to_string());
    let _ = builder.registry.register_verifier("NoSecretsVerifier".to_string());
    let _ = builder.registry.register_verifier("PatchAppliesVerifier".to_string());
    let _ = builder.registry.register_verifier("MarkdownStructureVerifier".to_string());
    let _ = builder.registry.register_verifier("CommandVerifier".to_string());

    // Register hooks in registry
    let _ = builder.registry.register_hook("VerificationToolHook".to_string());
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
pub fn inspect_runtime(
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
    )?;
    Ok(runtime.inspect())
}
