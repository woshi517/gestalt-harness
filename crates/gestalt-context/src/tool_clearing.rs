use gestalt_core::message::Message;
use crate::tool_exchanges::group_tool_exchanges;
use crate::estimate_message_tokens;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClearAction {
    pub message_index: usize,
    pub tool_use_id: String,
    pub tool_name: String,
    pub original_tokens: usize,
    pub output_hash: String,
}

pub fn is_tool_eligible_for_clearing(tool_name: Option<&str>) -> bool {
    let Some(name) = tool_name else {
        return false;
    };
    matches!(
        name,
        "view_file"
            | "grep_search"
            | "list_dir"
            | "search_web"
            | "read_url_content"
            | "read_browser_page"
            | "read_file"
    )
}

pub fn render_tombstone(
    tool_use_id: &str,
    tool_name: &str,
    output_hash: &str,
    original_tokens: usize,
) -> String {
    format!(
        "<tombstone tool_use_id=\"{}\" tool_name=\"{}\" output_hash=\"{}\">\nThis tool result was cleared to save context tokens. Original token count: {}.\n</tombstone>",
        tool_use_id, tool_name, output_hash, original_tokens
    )
}

pub fn find_recent_window_start(history: &[Message], keep_recent_turns: usize) -> usize {
    if keep_recent_turns == 0 {
        return history.len();
    }
    let mut turns_seen = 0;
    for (idx, msg) in history.iter().enumerate().rev() {
        if matches!(msg, Message::User { .. }) {
            turns_seen += 1;
            if turns_seen >= keep_recent_turns {
                return idx;
            }
        }
    }
    0
}

pub fn find_recent_protected_start(
    history: &[Message],
    keep_recent_turns: usize,
    keep_recent_tokens: usize,
) -> usize {
    let window_start = find_recent_window_start(history, keep_recent_turns);

    let mut tail_start = history.len();
    let mut tokens_acc = 0;
    for (idx, msg) in history.iter().enumerate().rev() {
        let msg_tokens = estimate_message_tokens(msg);
        tokens_acc += msg_tokens;
        if tokens_acc > keep_recent_tokens {
            tail_start = idx + 1;
            break;
        }
        tail_start = idx;
    }

    std::cmp::min(window_start, tail_start)
}

pub fn total_tool_results_tokens(history: &[Message]) -> usize {
    history
        .iter()
        .filter(|msg| matches!(msg, Message::ToolResult { .. }))
        .map(estimate_message_tokens)
        .sum()
}

pub fn clear_eligible_tool_results(
    history: &[Message],
    _usable_limit: usize,
    tool_result_budget: usize,
    keep_recent_turns: usize,
    keep_recent_tokens: usize,
) -> (Vec<Message>, Vec<ClearAction>) {
    let mut projected_history = history.to_vec();
    let mut clear_actions = Vec::new();

    let current_tool_tokens = total_tool_results_tokens(history);
    if current_tool_tokens <= tool_result_budget {
        return (projected_history, clear_actions);
    }

    let mut reduction_needed = current_tool_tokens.saturating_sub(tool_result_budget);
    let recent_protected_start =
        find_recent_protected_start(history, keep_recent_turns, keep_recent_tokens);
    let exchanges = group_tool_exchanges(history);

    // Filter to complete, unprotected exchanges
    let mut candidate_exchanges = Vec::new();
    for exchange in exchanges {
        if !exchange.is_complete() {
            continue; // Skip incomplete/unresolved
        }
        if exchange.assistant_message_idx >= recent_protected_start {
            continue; // Inside protected recent window
        }
        if exchange
            .tool_result_idxs
            .iter()
            .any(|&idx| idx >= recent_protected_start)
        {
            continue; // Inside protected recent window
        }
        candidate_exchanges.push(exchange);
    }

    // Choose oldest first: candidate_exchanges is already in chronological order (oldest to newest)
    for exchange in candidate_exchanges {
        if reduction_needed == 0 {
            break;
        }

        // We clear eligible results in this exchange
        for &idx in &exchange.tool_result_idxs {
            if reduction_needed == 0 {
                break;
            }
            let msg = &history[idx];
            if let Message::ToolResult {
                tool_use_id,
                content: _,
                is_error,
                tool_name,
                output_hash,
                artifact_refs,
                failure: _,
            } = msg
            {
                if *is_error {
                    continue; // Skip active errors
                }
                let t_name = tool_name.as_deref().unwrap_or("");
                if !is_tool_eligible_for_clearing(tool_name.as_deref()) {
                    continue; // Skip non-eligible / unknown provenance
                }
                let hash = output_hash.as_deref().unwrap_or("");
                if hash.is_empty() {
                    continue; // Skip if no output hash
                }

                // Calculate token reduction
                let original_tokens = estimate_message_tokens(msg);
                let tombstone_content =
                    render_tombstone(tool_use_id, t_name, hash, original_tokens);
                let tombstone_msg = Message::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: tombstone_content,
                    is_error: false,
                    failure: None,
                    tool_name: tool_name.clone(),
                    output_hash: output_hash.clone(),
                    artifact_refs: artifact_refs.clone(),
                };
                let new_tokens = estimate_message_tokens(&tombstone_msg);
                let saved = original_tokens.saturating_sub(new_tokens);

                if saved > 0 {
                    projected_history[idx] = tombstone_msg;
                    clear_actions.push(ClearAction {
                        message_index: idx,
                        tool_use_id: tool_use_id.clone(),
                        tool_name: t_name.to_string(),
                        original_tokens,
                        output_hash: hash.to_string(),
                    });
                    reduction_needed = reduction_needed.saturating_sub(saved);
                }
            }
        }
    }

    (projected_history, clear_actions)
}
