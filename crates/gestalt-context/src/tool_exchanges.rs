use gestalt_core::message::{ContentBlock, Message};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExchange {
    pub assistant_message_idx: usize,
    pub tool_result_idxs: Vec<usize>,
    pub tool_use_ids: Vec<String>,
}

impl ToolExchange {
    /// Checks if all tool calls in the assistant message have corresponding tool results.
    pub fn is_complete(&self) -> bool {
        self.tool_result_idxs.len() == self.tool_use_ids.len()
    }
}

/// Scans the history and groups assistant tool calls with their matching tool results.
pub fn group_tool_exchanges(history: &[Message]) -> Vec<ToolExchange> {
    let mut exchanges = Vec::new();

    for (idx, msg) in history.iter().enumerate() {
        if let Message::Assistant { content } = msg {
            let tool_use_ids: Vec<String> = content
                .iter()
                .filter_map(|block| {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();

            if !tool_use_ids.is_empty() {
                let mut tool_result_idxs = Vec::new();
                let mut unresolved = tool_use_ids.clone();

                // Scan forward to find corresponding tool results
                for (f_idx, f_msg) in history.iter().enumerate().skip(idx + 1) {
                    match f_msg {
                        Message::ToolResult { tool_use_id, .. } => {
                            if let Some(pos) = unresolved.iter().position(|id| id == tool_use_id) {
                                tool_result_idxs.push(f_idx);
                                unresolved.remove(pos);
                            }
                        }
                        Message::Assistant { .. } | Message::User { .. } => {
                            // Another turn started, stop scanning for this exchange
                            break;
                        }
                        Message::System { .. } => {
                            // System messages can be ignored or skipped in this scan
                        }
                    }
                    if unresolved.is_empty() {
                        break;
                    }
                }

                exchanges.push(ToolExchange {
                    assistant_message_idx: idx,
                    tool_result_idxs,
                    tool_use_ids,
                });
            }
        }
    }

    exchanges
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_group_tool_exchanges_complete() {
        let history = vec![
            Message::User {
                content: vec![ContentBlock::Text { text: "run tools".to_string() }],
                metadata: None,
            },
            Message::Assistant {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "call_2".to_string(),
                        name: "write_file".to_string(),
                        input: json!({}),
                    },
                ],
            },
            Message::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "content".to_string(),
                is_error: false,
                failure: None,
                tool_name: None,
                output_hash: None,
                artifact_refs: None,
            },
            Message::ToolResult {
                tool_use_id: "call_2".to_string(),
                content: "saved".to_string(),
                is_error: false,
                failure: None,
                tool_name: None,
                output_hash: None,
                artifact_refs: None,
            },
        ];

        let exchanges = group_tool_exchanges(&history);
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].assistant_message_idx, 1);
        assert_eq!(exchanges[0].tool_result_idxs, vec![2, 3]);
        assert_eq!(exchanges[0].tool_use_ids, vec!["call_1".to_string(), "call_2".to_string()]);
        assert!(exchanges[0].is_complete());
    }

    #[test]
    fn test_group_tool_exchanges_incomplete() {
        let history = vec![
            Message::Assistant {
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({}),
                }],
            },
            Message::User {
                content: vec![ContentBlock::Text { text: "interrupted".to_string() }],
                metadata: None,
            },
        ];

        let exchanges = group_tool_exchanges(&history);
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].tool_result_idxs.len(), 0);
        assert!(!exchanges[0].is_complete());
    }
}
