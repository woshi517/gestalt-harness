use crate::config::{load_effective_config, CliOverrides};
use crate::output::ContextExplainReport;
use gestalt_core::ToolCatalog;
use gestalt_core::{context::ContextPipeline, Message, TokenBudget};
use gestalt_runtime::context::{ContextContributor, ContextPatch, RuntimeContextPipeline};
use gestalt_runtime::workspace_context::load_and_snapshot_workspace_context;
use gestalt_tools::default_registry;
use std::sync::{Arc, Mutex};

pub async fn explain_context(
    overrides: &CliOverrides,
    prompt: Option<&str>,
    run_id_or_path: Option<&str>,
) -> Result<ContextExplainReport, Box<dyn std::error::Error>> {
    let config = load_effective_config(overrides)?;

    if let Some(prompt) = prompt {
        let mode = config.selected_mode()?;
        let max_turns = config.max_turns();
        let tools = default_registry()?;
        let tool_names: Vec<String> = tools
            .schemas()
            .iter()
            .filter_map(|s| s.get("name").and_then(|v| v.as_str()).map(String::from))
            .collect();

        let pipeline = crate::run::build_pipeline(&config, mode, max_turns, &tool_names)?;
        let max_output_tokens = config
            .resolve_provider()
            .map(|r| r.resolved_options.max_output_tokens)
            .ok()
            .flatten()
            .map(|v| v as usize);
        let budget = TokenBudget {
            model_limit: config.context.max_context_window.unwrap_or(120_000),
            reserved_output: config
                .context
                .reserved_output_tokens
                .or(max_output_tokens)
                .unwrap_or(4096),
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 16,
        };

        // Load workspace and memory contributors using load_and_snapshot_workspace_context.
        let workspace_cfg = config.context.workspace.clone().unwrap_or_default();
        let memory_cfg = config.context.memory.clone().unwrap_or_default();
        let event_bus = gestalt_runtime::event_bus::RuntimeEventBus::new();

        let policy = Arc::new(crate::run::build_policy(&config));
        let (ws_contrib, mem_contrib, _) = load_and_snapshot_workspace_context(
            &config.workspace_root,
            Some(policy as Arc<dyn gestalt_core::policy::PolicyEngine>),
            &event_bus,
            &workspace_cfg,
            &memory_cfg,
        )
        .await?;

        let mut patches = Vec::new();
        if let Some(contrib) = ws_contrib {
            let msg = contrib.contribute(&config.workspace_root).await?;
            let content_str = match &msg {
                Message::System { content } => content.clone(),
                _ => String::new(),
            };
            let source = contrib.source(&config.workspace_root, &content_str);
            let omissions = contrib.omissions(&config.workspace_root);
            patches.push(ContextPatch::new_with_metadata(
                msg,
                contrib.stability(),
                source,
                omissions,
            ));
        }

        if let Some(contrib) = mem_contrib {
            let msg = contrib.contribute(&config.workspace_root).await?;
            let content_str = match &msg {
                Message::System { content } => content.clone(),
                _ => String::new(),
            };
            let source = contrib.source(&config.workspace_root, &content_str);
            let omissions = contrib.omissions(&config.workspace_root);
            patches.push(ContextPatch::new_with_metadata(
                msg,
                contrib.stability(),
                source,
                omissions,
            ));
        }

        let patch_store = Arc::new(Mutex::new(patches));
        let runtime_pipeline = RuntimeContextPipeline {
            base: Arc::new(pipeline),
            patch_store,
        };

        let history = vec![gestalt_core::SessionMessage {
            id: gestalt_core::MessageId {
                origin_session_id: "context-explain".to_string(),
                origin_message_namespace: "context-explain".to_string(),
                sequence: 0,
            },
            message: Message::User {
                content: vec![gestalt_core::ContentBlock::Text {
                    text: prompt.to_string(),
                }],
                metadata: None,
            },
            metadata: None,
        }];

        let packet = runtime_pipeline.build_packet(&history, &budget);

        let system_prompt_str = packet
            .messages
            .iter()
            .filter_map(|msg| match msg {
                gestalt_core::Message::System { content } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(ContextExplainReport {
            prompt: Some(prompt.to_string()),
            run_id: None,
            token_estimate: packet.token_estimate,
            packet_hash: packet.packet_hash,
            pipeline_version: packet.pipeline_version,
            prompt_source: packet.prompt_source,
            system_prompt: Some(system_prompt_str),
            sources: packet.sources,
            omissions: packet.omissions,
        })
    } else if let Some(run_id_or_path) = run_id_or_path {
        let run_dir = crate::runs::resolve_run_path(&config, run_id_or_path)?;
        let trace_path = run_dir.join("trace.jsonl");
        if !trace_path.exists() {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Trace file not found for run '{}'", run_id_or_path),
            )));
        }

        let envelopes = gestalt_trace::read_trace(&trace_path)?;
        for envelope in envelopes.iter().rev() {
            if let gestalt_core::AgentEvent::ContextBuilt {
                packet_id,
                token_estimate,
                packet_hash,
                sources,
                omissions,
                prompt_source,
            } = &envelope.event
            {
                return Ok(ContextExplainReport {
                    prompt: None,
                    run_id: Some(run_id_or_path.to_string()),
                    token_estimate: *token_estimate,
                    packet_hash: packet_hash.clone().unwrap_or_default(),
                    pipeline_version: packet_id.clone(),
                    prompt_source: prompt_source.clone(),
                    system_prompt: None,
                    sources: sources.clone().unwrap_or_default(),
                    omissions: omissions.clone().unwrap_or_default(),
                });
            }
        }

        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No ContextBuilt event found in the trace log",
        )))
    } else {
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Either --prompt or --run must be specified",
        )))
    }
}
