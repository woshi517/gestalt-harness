use gestalt_core::{AgentEvent, HarnessError, ProviderError, event::StopReason};
use serde_json::Value;

pub fn parse_sse(input: &str) -> Vec<(Option<String>, String)> {
    let mut output = Vec::new();
    let mut event = None;
    let mut data = String::new();

    for line in input.lines() {
        if line.is_empty() {
            if !data.is_empty() {
                output.push((event.take(), data.trim_end_matches('\n').to_string()));
                data.clear();
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim_start());
            data.push('\n');
        }
    }
    if !data.is_empty() {
        output.push((event, data.trim_end_matches('\n').to_string()));
    }
    output
}

pub fn provider_error_for_status(status: reqwest::StatusCode, body: &str) -> ProviderError {
    let lower = body.to_ascii_lowercase();
    let details = sanitize_detail(body);

    match status.as_u16() {
        401 | 403 => ProviderError::AuthFailed {
            provider: "provider".to_string(),
        },
        408 | 504 => ProviderError::Timeout,
        429 => ProviderError::RateLimit {
            retry_after_secs: Some(1),
        },
        400 if lower.contains("context") || lower.contains("maximum context") => {
            ProviderError::ContextTooLong { tokens: 0, limit: 0 }
        }
        400 if lower.contains("model") && lower.contains("invalid") => ProviderError::InvalidModel {
            model: details,
        },
        _ => ProviderError::UnexpectedResponse {
            details: format!("HTTP {status}: {details}"),
        },
    }
}

pub fn json(data: &str) -> Result<Value, HarnessError> {
    serde_json::from_str(data).map_err(|err| {
        HarnessError::Provider(ProviderError::UnexpectedResponse {
            details: format!("invalid SSE JSON: {err}"),
        })
    })
}

#[must_use]
pub fn stop_reason(value: Option<&str>) -> StopReason {
    match value {
        Some("tool_use" | "tool_calls") => StopReason::ToolUse,
        Some("max_tokens" | "length") => StopReason::MaxOutput,
        Some("content_filter") => StopReason::ContentFiltered,
        _ => StopReason::EndTurn,
    }
}

#[must_use]
pub fn sanitize_detail(input: &str) -> String {
    input
        .split_whitespace()
        .map(redact_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_token(token: &str) -> String {
    if looks_like_secret(token) {
        "[REDACTED]".to_string()
    } else {
        token.to_string()
    }
}

fn looks_like_secret(token: &str) -> bool {
    let trimmed = token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ',' | ';' | ')' | '('));
    let lowered = trimmed.to_ascii_lowercase();

    trimmed.starts_with("sk-")
        || trimmed.starts_with("sk_ant_")
        || trimmed.starts_with("sk-ant-")
        || trimmed.starts_with("Bearer sk-")
        || is_jwt_like(trimmed)
        || (trimmed.contains("://") && trimmed.contains('@'))
        || lowered.contains("api_key")
}

fn is_jwt_like(token: &str) -> bool {
    let parts = token.split('.').collect::<Vec<_>>();
    parts.len() == 3 && parts.iter().all(|part| part.len() >= 8)
}

#[allow(dead_code)]
pub fn stream_from_events(events: Vec<Result<AgentEvent, HarnessError>>) -> gestalt_core::EventStream {
    Box::pin(futures::stream::iter(events))
}
