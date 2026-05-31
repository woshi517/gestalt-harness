use std::collections::HashMap;

use serde_json::Value;

use crate::{
    error::{HarnessError, ProviderError},
    event::{AgentEvent, StopReason},
    message::{ContentBlock, Message},
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssistantTurn {
    pub text_deltas: Vec<String>,
    pub thinking_deltas: Vec<String>,
    pub tool_calls: Vec<ProposedToolCall>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposedToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl AssistantTurn {
    pub fn full_text(&self) -> String {
        self.text_deltas.concat()
    }

    pub fn into_message(self) -> Message {
        let mut content = Vec::new();

        let text = self.full_text();
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }

        let thinking = self.thinking_deltas.concat();
        if !thinking.is_empty() {
            content.push(ContentBlock::Thinking { thinking });
        }

        for call in self.tool_calls {
            content.push(ContentBlock::ToolUse {
                id: call.id,
                name: call.name,
                input: call.input,
            });
        }

        Message::Assistant { content }
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

#[derive(Debug, Clone)]
struct PendingToolCall {
    name: String,
    input: String,
}

#[derive(Debug, Default)]
pub struct TurnAccumulator {
    text_deltas: Vec<String>,
    thinking_deltas: Vec<String>,
    pending_tool_calls: HashMap<String, PendingToolCall>,
    tool_call_order: Vec<String>,
}

impl TurnAccumulator {
    pub fn record(&mut self, event: &AgentEvent) -> Result<(), HarnessError> {
        match event {
            AgentEvent::Text { delta } => {
                self.text_deltas.push(delta.clone());
            }
            AgentEvent::Thinking { delta } => {
                self.thinking_deltas.push(delta.clone());
            }
            AgentEvent::ToolCallStreamed {
                id,
                name,
                input_delta,
            } => {
                let entry = self
                    .pending_tool_calls
                    .entry(id.clone())
                    .or_insert_with(|| {
                        self.tool_call_order.push(id.clone());
                        PendingToolCall {
                            name: name.clone(),
                            input: String::new(),
                        }
                    });
                entry.input.push_str(input_delta);
            }
            AgentEvent::Stop {
                reason: StopReason::ToolUse | StopReason::EndTurn,
            } => {}
            AgentEvent::Error { .. } => {}
            _ => {}
        }

        Ok(())
    }

    pub fn finish(self) -> Result<AssistantTurn, HarnessError> {
        let Self {
            text_deltas,
            thinking_deltas,
            pending_tool_calls,
            tool_call_order,
        } = self;

        let mut turn = AssistantTurn {
            text_deltas,
            thinking_deltas,
            tool_calls: Vec::with_capacity(tool_call_order.len()),
        };

        for id in tool_call_order {
            let pending = pending_tool_calls.get(&id).ok_or_else(|| {
                HarnessError::Provider(ProviderError::InvalidResponse(format!(
                    "missing accumulated tool call: {id}"
                )))
            })?;

            let input = if pending.input.trim().is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str::<Value>(&pending.input).map_err(|err| {
                    HarnessError::Provider(ProviderError::InvalidResponse(format!(
                        "invalid tool call input for {id}: {err}"
                    )))
                })?
            };

            turn.tool_calls.push(ProposedToolCall {
                id,
                name: pending.name.clone(),
                input,
            });
        }

        Ok(turn)
    }
}
