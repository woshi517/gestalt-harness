use crate::config::{load_effective_config, CliOverrides};
use crate::output::ContextExplainReport;
use gestalt_core::ToolCatalog;
use gestalt_core::{context::ContextPipeline, Message, TokenBudget};
use gestalt_tools::default_registry;

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
        let budget = TokenBudget {
            model_limit: config.context.max_context_window.unwrap_or(120_000),
            reserved_output: config.context.reserved_output_tokens.unwrap_or(8_000),
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 16,
        };

        let history = vec![Message::User {
            content: vec![gestalt_core::ContentBlock::Text {
                text: prompt.to_string(),
            }],
            metadata: None,
        }];

        let packet = pipeline.build_packet(&history, &budget);

        Ok(ContextExplainReport {
            prompt: Some(prompt.to_string()),
            run_id: None,
            token_estimate: packet.token_estimate,
            packet_hash: packet.packet_hash,
            pipeline_version: packet.pipeline_version,
            prompt_source: packet.prompt_source,
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
