#![allow(clippy::pedantic, clippy::missing_errors_doc, clippy::too_many_lines)]

use crate::output::CliReport;
use gestalt_app::config::EffectiveConfig;

#[derive(Debug, PartialEq, Eq)]
pub enum SlashOutcome {
    None,
    Quit,
    ChangeMode(String),
    SkillActivated(String),
    SkillDeactivated(String),
}

pub async fn handle_slash_command(
    command_str: &str,
    session_id: &str,
    parent_run_id: Option<&str>,
    overrides: &mut gestalt_app::config::CliOverrides,
    config: &EffectiveConfig,
) -> Result<SlashOutcome, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = command_str.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(SlashOutcome::None);
    }

    let cmd = parts[0];
    match cmd {
        "/quit" | "/exit" => Ok(SlashOutcome::Quit),
        "/help" => {
            println!("Available slash commands:");
            println!("  /help                  Show this help message");
            println!("  /quit, /exit           Exit the chat session");
            println!("  /mode <mode>           Change the execution mode (confirm, yolo, human, dry-run, replay)");
            println!("  /skill <name>          Activate a skill for this session");
            println!("  /skill off <name>      Deactivate a skill for this session");
            println!(
                "  /cost                  Show the aggregated cost of all runs in this session"
            );
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
                    println!(
                        "Invalid mode. Supported modes: confirm, yolo, human, dry-run, replay"
                    );
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
                match gestalt_app::context::explain_context(overrides, None, Some(parent)).await {
                    Ok(report) => println!("{}", report.render_text()),
                    Err(e) => println!("Error explaining context: {e}"),
                }
            } else {
                println!("No runs have been executed in this session yet.");
            }
            Ok(SlashOutcome::None)
        }
        "/runs" => {
            match gestalt_app::sessions::inspect_session(config, session_id) {
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
                match gestalt_app::verify::verify_run(config, parent).await {
                    Ok(report) => println!("{}", report.render_text()),
                    Err(e) => println!("Error verifying run: {e}"),
                }
            } else {
                println!("No runs have been executed in this session yet.");
            }
            Ok(SlashOutcome::None)
        }
        "/skill" => {
            if parts.len() < 2 {
                println!("Usage: /skill <name> | /skill off <name>");
                return Ok(SlashOutcome::None);
            }
            if parts[1] == "off" {
                if parts.len() < 3 {
                    println!("Usage: /skill off <name>");
                    return Ok(SlashOutcome::None);
                }
                let name = parts[2];
                match gestalt_app::runtime_factory::validate_skill_activation(config, name) {
                    gestalt_app::runtime_factory::SkillValidation::Unknown { .. } => {
                        println!(
                            "Cannot deactivate unknown skill '{name}'. Use `gestalt skill list` to see available skills."
                        );
                        return Ok(SlashOutcome::None);
                    }
                    _ => {
                        println!("Deactivating skill '{name}'...");
                        return Ok(SlashOutcome::SkillDeactivated(name.to_string()));
                    }
                }
            }
            let name = parts[1];
            match gestalt_app::runtime_factory::validate_skill_activation(config, name) {
                gestalt_app::runtime_factory::SkillValidation::Ok { .. } => {
                    println!("Activating skill '{name}'...");
                    Ok(SlashOutcome::SkillActivated(name.to_string()))
                }
                other => {
                    if let Some(msg) = other.render_error() {
                        println!("{msg}");
                    }
                    Ok(SlashOutcome::None)
                }
            }
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
                    if let Ok(summary) = gestalt_app::runs::summarize_run_dir(&entry.path()) {
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
