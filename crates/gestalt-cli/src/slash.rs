#![allow(clippy::pedantic, clippy::missing_errors_doc, clippy::too_many_lines)]

use crate::config::EffectiveConfig;
use crate::output::CliReport;

pub enum SlashOutcome {
    None,
    Quit,
    ChangeMode(String),
}

pub async fn handle_slash_command(
    command_str: &str,
    session_id: &str,
    parent_run_id: Option<&str>,
    overrides: &mut crate::config::CliOverrides,
    config: &EffectiveConfig,
) -> Result<SlashOutcome, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = command_str.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(SlashOutcome::None);
    }

    let cmd = parts[0];
    match cmd {
        "/quit" | "/exit" => {
            Ok(SlashOutcome::Quit)
        }
        "/help" => {
            println!("Available slash commands:");
            println!("  /help                  Show this help message");
            println!("  /quit, /exit           Exit the chat session");
            println!("  /mode <mode>           Change the execution mode (confirm, yolo, human, dry-run, replay)");
            println!("  /cost                  Show the aggregated cost of all runs in this session");
            println!("  /context               Explain the context pipeline of the latest run");
            println!("  /runs                  Display the lineage tree of runs in this session");
            println!("  /export <format>       Export the latest run's trace (markdown, jsonl)");
            println!("  /verify                Run verifiers on the latest run's artifacts");
            Ok(SlashOutcome::None)
        }
        "/mode" => {
            if parts.len() < 2 {
                println!("Usage: /mode <mode>");
                return Ok(SlashOutcome::None);
            }
            let new_mode = parts[1].to_lowercase();
            match new_mode.as_str() {
                "confirm" | "yolo" | "human" | "dry-run" | "replay" => {
                    overrides.mode = Some(new_mode.clone());
                    println!("Switched execution mode to '{new_mode}'");
                    Ok(SlashOutcome::ChangeMode(new_mode))
                }
                _ => {
                    println!("Invalid mode. Supported modes: confirm, yolo, human, dry-run, replay");
                    Ok(SlashOutcome::None)
                }
            }
        }
        "/cost" => {
            let total_cost = calculate_session_cost(config, session_id);
            println!("Aggregated session cost: ${total_cost:.6}");
            Ok(SlashOutcome::None)
        }
        "/context" => {
            if let Some(parent) = parent_run_id {
                match crate::context::explain_context(overrides, None, Some(parent)).await {
                    Ok(report) => println!("{}", report.render_text()),
                    Err(e) => println!("Error explaining context: {e}"),
                }
            } else {
                println!("No runs have been executed in this session yet.");
            }
            Ok(SlashOutcome::None)
        }
        "/runs" => {
            match crate::sessions::inspect_session(config, session_id) {
                Ok(report) => println!("{}", report.render_text()),
                Err(e) => println!("Error inspecting session: {e}"),
            }
            Ok(SlashOutcome::None)
        }
        "/export" => {
            if parts.len() < 2 {
                println!("Usage: /export <markdown|jsonl>");
                return Ok(SlashOutcome::None);
            }
            let format_str = parts[1].to_lowercase();
            let export_format = match format_str.as_str() {
                "markdown" => crate::output::ExportFormat::Markdown,
                "jsonl" => crate::output::ExportFormat::Jsonl,
                "sharegpt" => crate::output::ExportFormat::Sharegpt,
                _ => {
                    println!("Invalid export format. Supported: markdown, jsonl");
                    return Ok(SlashOutcome::None);
                }
            };
            if let Some(parent) = parent_run_id {
                match crate::export::export_run(config, parent, export_format) {
                    Ok(report) => println!("{}", report.render_text()),
                    Err(e) => println!("Error exporting run: {e}"),
                }
            } else {
                println!("No runs have been executed in this session yet.");
            }
            Ok(SlashOutcome::None)
        }
        "/verify" => {
            if let Some(parent) = parent_run_id {
                match crate::verify::verify_run(config, parent).await {
                    Ok(report) => println!("{}", report.render_text()),
                    Err(e) => println!("Error verifying run: {e}"),
                }
            } else {
                println!("No runs have been executed in this session yet.");
            }
            Ok(SlashOutcome::None)
        }
        _ => {
            println!("Unknown slash command: '{cmd}'. Type /help for a list of commands.");
            Ok(SlashOutcome::None)
        }
    }
}

#[must_use]
pub fn calculate_session_cost(config: &EffectiveConfig, session_id: &str) -> f64 {
    let run_log_dir = config.run_log_dir();
    let mut total = 0.0;
    if run_log_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Ok(summary) = crate::runs::summarize_run_dir(&entry.path()) {
                        if summary.session_id == session_id {
                            total += summary.estimated_cost_usd.unwrap_or(0.0);
                        }
                    }
                }
            }
        }
    }
    total
}
