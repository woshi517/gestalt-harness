pub use crate::context::compaction::plan_compaction_range;
use crate::context::CompactionCheckpoint;
use chrono::Utc;
use gestalt_core::{
    context::HistoryRange,
    error::{HarnessError, ProviderError},
    message::{ContentBlock, Message},
    provider::{Provider, ProviderRequest},
    turn::TurnAccumulator,
};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

#[derive(serde::Deserialize, serde::Serialize)]
struct CompactorOutput {
    goal: String,
    constraints: Vec<String>,
    completed_work: Vec<String>,
    in_progress_work: Vec<String>,
    blocked_items: Vec<String>,
    key_decisions: Vec<String>,
    next_steps: Vec<String>,
    critical_context: String,
    relevant_references: Vec<String>,
}

fn is_checkpoint_message(message: &Message) -> bool {
    match message {
        Message::System { content } => content.starts_with("### Session Checkpoint Summary"),
        _ => false,
    }
}

pub fn build_compactor_prompt(
    history_to_compact: &[Message],
    previous_checkpoint: Option<&CompactionCheckpoint>,
) -> Vec<Message> {
    let mut prompt_content = String::new();
    prompt_content.push_str("You are an expert context compactor. Your job is to compress a range of conversational history into a structured checkpoint summary JSON object.\n\n");

    if let Some(prev) = previous_checkpoint {
        prompt_content.push_str("Here is the PREVIOUS checkpoint summary which summarizes the history prior to the current chunk:\n");
        prompt_content.push_str(&prev.render_markdown());
        prompt_content.push('\n');
    }

    prompt_content.push_str("Please summarize the following new sequence of messages. Integrate it with any previous checkpoint details to produce the NEW consolidated checkpoint summary.\n\n");

    prompt_content
        .push_str("Output MUST be a single JSON object with EXACTLY the following structure:\n");
    prompt_content.push_str(
        r#"{
  "goal": "The overall goal of the session",
  "constraints": ["Constraint 1", "Constraint 2"],
  "completed_work": ["Completed item 1", "Completed item 2"],
  "in_progress_work": ["In progress item 1"],
  "blocked_items": ["Blocked item 1"],
  "key_decisions": ["Decision 1"],
  "next_steps": ["Next step 1"],
  "critical_context": "Any critical background or context",
  "relevant_references": ["Reference 1"]
}
"#,
    );
    prompt_content.push_str("\nBe extremely precise. Do not drop important user constraints or completed work. Ensure that 'goal' and 'critical_context' are not empty.\n\n");

    prompt_content.push_str("--- Messages to Compact ---\n");
    for (idx, msg) in history_to_compact.iter().enumerate() {
        if is_checkpoint_message(msg) {
            continue;
        }
        let _ = writeln!(
            prompt_content,
            "Message {} ({}):",
            idx,
            match msg {
                Message::System { .. } => "System",
                Message::User { .. } => "User",
                Message::Assistant { .. } => "Assistant",
                Message::ToolResult { .. } => "ToolResult",
            }
        );

        match msg {
            Message::System { content } => {
                prompt_content.push_str(content);
            }
            Message::User { content, .. } => {
                for block in content {
                    if let ContentBlock::Text { text } = block {
                        prompt_content.push_str(text);
                    } else if let ContentBlock::Document { source, .. } = block {
                        prompt_content.push_str(&source.data);
                    }
                }
            }
            Message::Assistant { content } => {
                for block in content {
                    if let ContentBlock::Text { text } = block {
                        prompt_content.push_str(text);
                    } else if let ContentBlock::ToolUse { name, input, .. } = block {
                        let _ =
                            writeln!(prompt_content, "Tool Use: {} with input: {}", name, input);
                    }
                }
            }
            Message::ToolResult {
                tool_name,
                content,
                is_error,
                output_hash,
                ..
            } => {
                let name = tool_name.as_deref().unwrap_or("unknown");
                let success = if *is_error { "failed" } else { "succeeded" };
                let hash = output_hash.as_deref().unwrap_or("none");
                let snippet: String = content.chars().take(300).collect();
                let _ = write!(
                    prompt_content,
                    "Tool Result: {} ({})\nOutput Hash: {}\nSnippet: {}\n",
                    name, success, hash, snippet
                );
            }
        }
        prompt_content.push_str("\n\n");
    }

    vec![
        Message::System {
            content: "You are an expert context compactor helper.".to_string(),
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: prompt_content,
            }],
            metadata: None,
        },
    ]
}

pub fn parse_json_from_response(text: &str) -> Option<serde_json::Value> {
    let parsed_text = text.trim();
    if let Some(start) = parsed_text.find('{') {
        if let Some(end) = parsed_text.rfind('}') {
            let json_str = &parsed_text[start..=end];
            if let Ok(val) = serde_json::from_str(json_str) {
                return Some(val);
            }
        }
    }
    None
}

pub async fn run_compactor(
    provider: &dyn Provider,
    model: &str,
    history_to_compact: &[Message],
    history_range: HistoryRange,
    history_range_hash: String,
    policy_version: String,
    previous_checkpoint: Option<&CompactionCheckpoint>,
) -> Result<CompactionCheckpoint, HarnessError> {
    let compactor_messages = build_compactor_prompt(history_to_compact, previous_checkpoint);

    let request = ProviderRequest {
        model: model.to_string(),
        messages: compactor_messages,
        tools: Vec::new(),
        tool_name_map: Vec::new(),
        max_tokens: 4096,
        temperature: Some(0.1),
        top_p: Some(1.0),
        stop_sequences: Vec::new(),
        cache_plan: None,
        metadata: serde_json::Value::Null,
        reasoning_effort: None,
        text_verbosity: None,
    };

    let mut stream = provider.stream(request).await?;
    let mut accumulator = TurnAccumulator::default();

    while let Some(event_res) = futures::StreamExt::next(&mut stream).await {
        let event = event_res?;
        accumulator.push(event)?;
    }

    let turn = accumulator.finish()?;
    let text_response = turn.full_text();

    let json_val = parse_json_from_response(&text_response).ok_or_else(|| {
        HarnessError::Provider(ProviderError::UnexpectedResponse {
            details: format!(
                "Failed to parse JSON compaction checkpoint from response: {}",
                text_response
            ),
        })
    })?;

    let output: CompactorOutput = serde_json::from_value(json_val).map_err(|err| {
        HarnessError::Provider(ProviderError::UnexpectedResponse {
            details: format!("Compaction JSON format mismatch: {}", err),
        })
    })?;

    let mut hasher = Sha256::new();
    hasher.update(text_response.as_bytes());
    let checkpoint_id = format!("{:x}", hasher.finalize());

    let compactor_prompt_serialized = serde_json::to_string(&build_compactor_prompt(
        history_to_compact,
        previous_checkpoint,
    ))
    .unwrap_or_default();
    let mut prompt_hasher = Sha256::new();
    prompt_hasher.update(compactor_prompt_serialized.as_bytes());
    let prompt_hash = format!("{:x}", prompt_hasher.finalize());

    Ok(CompactionCheckpoint {
        v: 1,
        checkpoint_id,
        history_range,
        history_range_hash,
        policy_version,
        compactor_model: model.to_string(),
        prompt_hash,
        created_at: Utc::now(),
        goal: output.goal,
        constraints: output.constraints,
        completed_work: output.completed_work,
        in_progress_work: output.in_progress_work,
        blocked_items: output.blocked_items,
        key_decisions: output.key_decisions,
        next_steps: output.next_steps,
        critical_context: output.critical_context,
        relevant_references: output.relevant_references,
    })
}
