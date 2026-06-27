use std::fs;

use gestalt_core::{AgentEvent, StopReason};
use gestalt_models::{AnthropicProvider, OpenAiChatCompletionsProvider};

fn fixture(path: &str) -> String {
    fs::read_to_string(format!("../../tests/fixtures/provider-streams/{path}"))
        .expect("fixture exists")
}

#[test]
fn openai_normalizes_multiple_tool_calls_in_order() {
    let events = OpenAiChatCompletionsProvider::normalize_sse(&fixture("openai-multiple-tools.sse"));
    let events = events
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("events parse");

    let streamed = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCallStreamed {
                id,
                name,
                input_delta,
            } => Some((id.clone(), name.clone(), input_delta.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(streamed.len(), 4);
    assert_eq!(streamed[0].0, "call_1");
    assert_eq!(streamed[1].0, "call_2");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Stop {
            reason: StopReason::ToolUse
        }
    )));
}

#[test]
fn anthropic_normalizes_tool_use_and_usage() {
    let events = AnthropicProvider::normalize_sse(&fixture("anthropic-single-tool.sse"));
    let events = events
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("events parse");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Usage {
            input_tokens: 12,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(event, AgentEvent::ToolCallStreamed { id, name, .. } if id == "toolu_1" && name == "read")));
}
