use gestalt_core::context::HistoryRange;
use gestalt_core::message::Message;
use crate::tool_exchanges::group_tool_exchanges;
use crate::estimate_message_tokens;

/// Computes the history range that is eligible and safe to compact.
///
/// * `history`: The message history projection to inspect.
/// * `last_checkpoint_end_idx`: End index of the previous checkpoint (starts compaction at this index).
/// * `recent_protected_start`: Start index of the recent protected window (compaction cannot cross this).
/// * `compactor_input_limit`: Token limit for the compaction model's input.
pub fn plan_compaction_range(
    history: &[Message],
    last_checkpoint_end_idx: usize,
    recent_protected_start: usize,
    compactor_input_limit: usize,
) -> Option<HistoryRange> {
    let start = last_checkpoint_end_idx;
    if start >= recent_protected_start || start >= history.len() {
        return None;
    }

    let message_tokens: Vec<usize> = history.iter().map(estimate_message_tokens).collect();
    let exchanges = group_tool_exchanges(history);

    // Start with the largest possible range
    let mut end = std::cmp::min(recent_protected_start, history.len());

    // 1. Trim from the right until it fits token budget
    while end > start {
        let range_tokens: usize = message_tokens[start..end].iter().sum();
        if range_tokens <= compactor_input_limit {
            break;
        }
        end -= 1;
    }

    // 2. Adjust end to ensure no split tool exchanges or incomplete exchanges
    let mut adjusted = true;
    while adjusted {
        adjusted = false;
        for exchange in &exchanges {
            if exchange.assistant_message_idx >= start && exchange.assistant_message_idx < end {
                if !exchange.is_complete() || exchange.tool_result_idxs.iter().any(|&r_idx| r_idx >= end) {
                    end = exchange.assistant_message_idx;
                    adjusted = true;
                }
            }
        }
    }

    if end > start {
        Some(HistoryRange::new(start, end))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::message::ContentBlock;
    use serde_json::json;

    #[test]
    fn test_plan_compaction_range_happy_path() {
        let history = vec![
            Message::System { content: "System instructions".to_string() },
            Message::User {
                content: vec![ContentBlock::Text { text: "hello".to_string() }],
                metadata: None,
            },
            Message::Assistant {
                content: vec![ContentBlock::Text { text: "world".to_string() }],
            },
            Message::User {
                content: vec![ContentBlock::Text { text: "next".to_string() }],
                metadata: None,
            },
        ];

        // start at index 1 (skip system prompt), recent protected start at 3
        let range = plan_compaction_range(&history, 1, 3, 1000);
        assert_eq!(range, Some(HistoryRange::new(1, 3)));
    }

    #[test]
    fn test_plan_compaction_range_trims_to_fit_limit() {
        let history = vec![
            Message::System { content: "System instructions".to_string() },
            Message::User {
                content: vec![ContentBlock::Text { text: "large text ".repeat(100) }],
                metadata: None,
            },
            Message::Assistant {
                content: vec![ContentBlock::Text { text: "small".to_string() }],
            },
            Message::User {
                content: vec![ContentBlock::Text { text: "next".to_string() }],
                metadata: None,
            },
        ];

        // limit is small (e.g. 50 tokens), so first message won't fit, but second might (if start was 2)
        let range = plan_compaction_range(&history, 1, 3, 50);
        // It should trim range down to empty or exclude the large message
        assert_eq!(range, None);

        let range_ok = plan_compaction_range(&history, 2, 3, 50);
        assert_eq!(range_ok, Some(HistoryRange::new(2, 3)));
    }

    #[test]
    fn test_plan_compaction_range_prevents_split_exchanges() {
        let history = vec![
            Message::User {
                content: vec![ContentBlock::Text { text: "hello".to_string() }],
                metadata: None,
            },
            Message::Assistant {
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({}),
                }],
            },
            Message::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file content".to_string(),
                is_error: false,
                failure: None,
                tool_name: Some("read_file".to_string()),
                output_hash: Some("hash1".to_string()),
                artifact_refs: None,
            },
            Message::User {
                content: vec![ContentBlock::Text { text: "thanks".to_string() }],
                metadata: None,
            },
        ];

        // Proposed end is 2 (so it includes the tool call but not the tool result at index 2)
        let range = plan_compaction_range(&history, 0, 2, 1000);
        // It must shrink the range to end at 1 (excluding the tool call) to avoid splitting the exchange
        assert_eq!(range, Some(HistoryRange::new(0, 1)));
    }

    #[test]
    fn test_plan_compaction_range_prevents_incomplete_exchanges() {
        let history = vec![
            Message::User {
                content: vec![ContentBlock::Text { text: "hello".to_string() }],
                metadata: None,
            },
            Message::Assistant {
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({}),
                }],
            },
        ];

        // The exchange at 1 is incomplete (no tool result exists). Proposed end is 2.
        let range = plan_compaction_range(&history, 0, 2, 1000);
        // It must shrink the range to end at 1 (excluding the incomplete exchange)
        assert_eq!(range, Some(HistoryRange::new(0, 1)));
    }
}
