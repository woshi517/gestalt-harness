use encoding_rs::Encoding;
use gestalt_core::{ToolError, ToolSchema};
use schemars::{schema_for, JsonSchema};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

pub(super) const DEFAULT_MAX_TOKENS: usize = 4_000;

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_start_line() -> usize {
    1
}

pub(super) fn default_max_tokens() -> usize {
    DEFAULT_MAX_TOKENS
}

pub(super) fn default_search_max_results() -> usize {
    100
}

pub(super) fn default_find_files_max_results() -> usize {
    50
}


pub(super) fn tool_schema<T>(name: &str, description: &str) -> ToolSchema
where
    T: JsonSchema,
{
    json!({
        "name": name,
        "description": description,
        "input_schema": serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null),
    })
}

pub(super) fn parse_input<T>(tool_name: &str, input: Value) -> Result<T, ToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
        tool_name: tool_name.to_string(),
        reason: err.to_string(),
    })
}

pub(super) fn invalid_input(tool_name: &str, reason: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool_name: tool_name.to_string(),
        reason: reason.into(),
    }
}

pub(super) fn decode_text(tool_name: &str, bytes: &[u8]) -> Result<String, ToolError> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_string());
    }

    let (encoding, bom_len) = Encoding::for_bom(bytes).unwrap_or((encoding_rs::UTF_8, 0));
    let (text, _, had_errors) = encoding.decode(&bytes[bom_len..]);
    if had_errors {
        return Err(invalid_input(tool_name, "file is not valid text"));
    }
    Ok(text.into_owned())
}

pub(super) fn limit_tokens(content: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(4);
    if content.chars().count() <= max_chars {
        return content.to_string();
    }

    let truncated = content.chars().take(max_chars).collect::<String>();
    format!(
        "{truncated}\n[Output truncated. Original: {} bytes.]",
        content.len()
    )
}

pub(super) fn calculate_sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(super) fn check_expected_hash(
    tool_name: &str,
    current_content: &str,
    expected_hash: Option<&str>,
) -> Result<(), ToolError> {
    if let Some(expected) = expected_hash {
        let actual = calculate_sha256(current_content);
        if actual != expected {
            return Err(invalid_input(
                tool_name,
                format!("conflict: expected_hash mismatch (expected: {expected}, actual: {actual})"),
            ));
        }
    }
    Ok(())
}

pub(super) fn atomic_write(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;

    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "temp".to_string()),
        now
    );
    let temp_path = parent.join(temp_name);

    std::fs::write(&temp_path, content.as_bytes())?;
    if let Err(err) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    Ok(())
}

