use std::fs;

use crate::output::{ExportFormat, ExportReport};
use crate::trace::resolve_trace_target;
use gestalt_app::config::EffectiveConfig;
use gestalt_app::runs;

/// Exports a run trace into the specified format (Markdown, JSONL).
pub fn export_run(
    config: &EffectiveConfig,
    run_id_or_path: &str,
    format: ExportFormat,
) -> Result<ExportReport, Box<dyn std::error::Error>> {
    if matches!(format, ExportFormat::Sharegpt) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ShareGPT export format is not supported yet.",
        )));
    }

    let (_run_id, run_dir, trace_path) = resolve_trace_target(config, run_id_or_path)?;

    if !trace_path.exists() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "trace.jsonl does not exist in run directory: {}",
                run_dir.display()
            ),
        )));
    }

    match format {
        ExportFormat::Jsonl => {
            let content = fs::read_to_string(&trace_path)?;
            Ok(ExportReport {
                format: "jsonl".to_string(),
                content,
            })
        }
        ExportFormat::Markdown => {
            let events = gestalt_trace::read_trace(&trace_path)?;
            let run_id = run_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let summary = runs::summarize_run_dir(&run_dir)?;

            let prov_mod = match (&summary.provider, &summary.model) {
                (Some(p), Some(m)) => format!("{p}/{m}"),
                (Some(p), None) => p.to_string(),
                (None, Some(m)) => m.to_string(),
                _ => "unknown".to_string(),
            };

            let start_time_str = summary
                .start_time
                .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let tokens_str = match (summary.total_input_tokens, summary.total_output_tokens) {
                (Some(i), Some(o)) => format!("{i} in / {o} out"),
                _ => "unknown".to_string(),
            };

            let cost_str = summary
                .estimated_cost_usd
                .map(|c| format!("${c:.6}"))
                .unwrap_or_else(|| "unknown".to_string());

            let mut markdown = format!(
                "# Run Export: {run_id}\n\n\
                 - **Session ID:** {}\n\
                 - **Start Time:** {start_time_str}\n\
                 - **Provider/Model:** {prov_mod}\n\
                 - **Status:** {}\n\
                 - **Turns:** {}\n\
                 - **Tokens:** {tokens_str}\n\
                 - **Cost:** {cost_str}\n\n\
                 ## Transcript\n\n",
                summary.session_id,
                summary.apparent_status,
                summary
                    .turns
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            );

            let transcript = gestalt_trace::render_display(&events);
            markdown.push_str(&transcript);

            Ok(ExportReport {
                format: "markdown".to_string(),
                content: markdown,
            })
        }
        ExportFormat::Sharegpt => Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ShareGPT export format is not supported yet.",
        ))),
    }
}
