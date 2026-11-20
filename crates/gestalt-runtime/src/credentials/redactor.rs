#![forbid(unsafe_code)]
//! Sliding-window secret redactor for streaming SSE/tool deltas, JSONL logs,
//! and diagnostic output.

use serde_json::Value;

/// Redaction placeholder substituted for any known host secret.
pub const REDACTED_MARKER: &str = "[REDACTED]";

/// Minimum secret length eligible for automatic stream redaction to avoid
/// false positives on short tokens.
pub(crate) const MIN_REDACT_LEN: usize = 4;

/// Stateful sliding-window secret redactor so secrets split across arbitrary
/// streaming text chunks never leak into `EventStream`, `SessionTree`, or logs.
#[derive(Debug, Clone, Default)]
pub struct StreamRedactor {
    secrets: Vec<String>,
    max_secret_len: usize,
    window: String,
}

impl StreamRedactor {
    /// Construct a [`StreamRedactor`] from an iterator of secret strings.
    #[must_use]
    pub fn from_secrets<I>(secrets: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut filtered: Vec<String> = secrets
            .into_iter()
            .filter(|s| s.len() >= MIN_REDACT_LEN)
            .collect();
        // Sort longest-first so overlapping prefixes do not leave partial tails.
        filtered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        filtered.dedup();
        let max_secret_len = filtered.iter().map(String::len).max().unwrap_or(0);
        Self {
            secrets: filtered,
            max_secret_len,
            window: String::new(),
        }
    }

    /// Add a newly discovered secret (e.g., refreshed OAuth access token) to
    /// this redactor instance.
    pub fn add_secret(&mut self, secret: String) {
        if secret.len() < MIN_REDACT_LEN || self.secrets.contains(&secret) {
            return;
        }
        self.max_secret_len = self.max_secret_len.max(secret.len());
        self.secrets.push(secret);
        self.secrets
            .sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    }

    /// Redact all occurrences of registered secrets in a complete string.
    #[must_use]
    pub fn redact_str(&self, input: &str) -> String {
        if self.secrets.is_empty() || input.is_empty() {
            return input.to_string();
        }
        let mut out = input.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED_MARKER);
            }
        }
        out
    }

    /// Recursively redact all string values inside a `serde_json::Value`.
    pub fn redact_json(&self, value: &mut Value) {
        if self.secrets.is_empty() {
            return;
        }
        match value {
            Value::String(s) => {
                *s = self.redact_str(s);
            }
            Value::Array(items) => {
                for item in items {
                    self.redact_json(item);
                }
            }
            Value::Object(map) => {
                for v in map.values_mut() {
                    self.redact_json(v);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    /// Push a streaming text chunk and return the redacted prefix guaranteed
    /// not to contain the start of a split secret.
    pub fn push_chunk(&mut self, chunk: &str) -> String {
        if self.secrets.is_empty() {
            return chunk.to_string();
        }
        self.window.push_str(chunk);
        self.window = self.redact_str(&self.window);

        let keep_tail = self.max_secret_len.saturating_sub(1);
        if self.window.len() <= keep_tail {
            return String::new();
        }
        let mut split_at = self.window.len() - keep_tail;
        while split_at > 0 && !self.window.is_char_boundary(split_at) {
            split_at -= 1;
        }
        let remainder = self.window.split_off(split_at);
        std::mem::replace(&mut self.window, remainder)
    }

    /// Flush and redact any remaining buffered trailing bytes at end-of-stream.
    pub fn finish(&mut self) -> String {
        if self.window.is_empty() {
            return String::new();
        }
        let remaining = std::mem::take(&mut self.window);
        self.redact_str(&remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_redactor_handles_split_secrets_across_chunks() {
        let mut redactor =
            StreamRedactor::from_secrets(vec!["sk-mock-secret-key-1234567890".to_string()]);
        let mut out = String::new();
        out.push_str(&redactor.push_chunk("data: {\"text\":\"my key is sk-mock-"));
        out.push_str(&redactor.push_chunk("secret-key-"));
        out.push_str(&redactor.push_chunk("1234567890!\"}\n\n"));
        out.push_str(&redactor.finish());

        assert_eq!(out, "data: {\"text\":\"my key is [REDACTED]!\"}\n\n");
    }
}
