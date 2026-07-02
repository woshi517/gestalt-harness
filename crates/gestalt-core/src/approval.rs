use std::fmt::Write as _;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::PolicyDecision;
use crate::tool::RiskLevel;

#[async_trait]
pub trait ApprovalProvider: Send + Sync {
    async fn approve(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalDecision, crate::error::HarnessError>;

    async fn approve_cancellable(
        &self,
        request: ApprovalRequest,
        cancel_token: &crate::cancel::CancelToken,
    ) -> Result<ApprovalDecision, crate::error::HarnessError> {
        tokio::select! {
            res = self.approve(request) => res,
            _ = cancel_token.cancelled() => Err(crate::error::HarnessError::Cancelled),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    pub decision: PolicyDecision,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
    Edit(Value),
    AlwaysAllowForSession,
}

pub struct AutoApprovalProvider;

#[async_trait]
impl ApprovalProvider for AutoApprovalProvider {
    async fn approve(
        &self,
        _request: ApprovalRequest,
    ) -> Result<ApprovalDecision, crate::error::HarnessError> {
        Ok(ApprovalDecision::Approve)
    }
}

pub struct DenyApprovalProvider;

#[async_trait]
impl ApprovalProvider for DenyApprovalProvider {
    async fn approve(
        &self,
        _request: ApprovalRequest,
    ) -> Result<ApprovalDecision, crate::error::HarnessError> {
        Ok(ApprovalDecision::Deny)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGrant {
    pub tool_name: String,
    pub input_hash: String,
    pub risk_ceiling: RiskLevel,
    pub matched_rule: String,
    pub policy_source: String,
    pub granted_at_turn: usize,
    pub expires_in_turns: usize,
}

impl SessionGrant {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tool_name: impl Into<String>,
        input: &Value,
        risk_ceiling: RiskLevel,
        matched_rule: impl Into<String>,
        policy_source: impl Into<String>,
        granted_at_turn: usize,
        expires_in_turns: usize,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            input_hash: hash_input(input),
            risk_ceiling,
            matched_rule: matched_rule.into(),
            policy_source: policy_source.into(),
            granted_at_turn,
            expires_in_turns,
        }
    }

    pub fn matches_tool(&self, tool_name: &str) -> bool {
        self.tool_name == tool_name
    }

    pub fn covers(
        &self,
        tool_name: &str,
        input: &Value,
        risk: RiskLevel,
        current_turn: usize,
    ) -> bool {
        if self.tool_name != tool_name {
            return false;
        }
        if risk > self.risk_ceiling {
            return false;
        }
        if self.input_hash != hash_input(input) {
            return false;
        }
        self.granted_at_turn
            .saturating_add(self.expires_in_turns)
            .saturating_sub(current_turn)
            > 0
    }
}

pub fn hash_input(input: &Value) -> String {
    let canonical = canonicalize(input);
    fnv1a_64(canonical.as_bytes())
}

fn fnv1a_64(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let mut out = String::with_capacity(16);
    let _ = write!(&mut out, "{hash:016x}");
    out
}

pub fn hash_input_short(input: &Value) -> String {
    hash_input(input).chars().take(8).collect()
}

fn canonicalize(value: &Value) -> String {
    let mut buf = String::new();
    write_canonical(value, &mut buf);
    buf
}

fn write_canonical(value: &Value, buf: &mut String) {
    match value {
        Value::Null => buf.push_str("null"),
        Value::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => buf.push_str(&n.to_string()),
        Value::String(s) => {
            buf.push('"');
            buf.push_str(&escape_json_string(s));
            buf.push('"');
        }
        Value::Array(items) => {
            buf.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                write_canonical(item, buf);
            }
            buf.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            buf.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                buf.push('"');
                buf.push_str(&escape_json_string(k));
                buf.push_str("\":");
                write_canonical(v, buf);
            }
            buf.push('}');
        }
    }
}

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(&mut out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_input_is_deterministic_under_key_reorder() {
        let a = json!({"b": 1, "a": 2, "c": {"y": true, "x": null}});
        let b = json!({"c": {"x": null, "y": true}, "a": 2, "b": 1});
        assert_eq!(hash_input(&a), hash_input(&b));
    }

    #[test]
    fn hash_input_differs_for_different_values() {
        assert_ne!(hash_input(&json!({"a": 1})), hash_input(&json!({"a": 2})));
        assert_ne!(hash_input(&json!({"a": 1})), hash_input(&json!({"b": 1})));
    }

    #[test]
    fn session_grant_matches_only_matching_tool_and_hash() {
        let grant = SessionGrant::new(
            "bash",
            &json!({"command": "ls"}),
            RiskLevel::Medium,
            "allow-listed",
            "session_grant",
            0,
            usize::MAX,
        );
        assert!(grant.covers("bash", &json!({"command": "ls"}), RiskLevel::Low, 0));
        assert!(!grant.covers("read", &json!({"command": "ls"}), RiskLevel::Low, 0));
        assert!(!grant.covers("bash", &json!({"command": "rm"}), RiskLevel::Low, 0));
    }

    #[test]
    fn session_grant_limited_by_tool_and_risk() {
        let grant = SessionGrant::new(
            "bash",
            &json!({"command": "ls"}),
            RiskLevel::Medium,
            "allow-listed",
            "session_grant",
            0,
            usize::MAX,
        );
        assert!(grant.covers("bash", &json!({"command": "ls"}), RiskLevel::Medium, 0));
        assert!(!grant.covers("read", &json!({"command": "ls"}), RiskLevel::Medium, 0));
        assert!(!grant.covers("bash", &json!({"command": "ls"}), RiskLevel::High, 0));
        assert!(!grant.covers("bash", &json!({"command": "ls"}), RiskLevel::Critical, 0));
    }

    #[test]
    fn session_grant_limited_by_turns() {
        let grant = SessionGrant::new(
            "bash",
            &json!({"command": "ls"}),
            RiskLevel::Medium,
            "allow-listed",
            "session_grant",
            2,
            3,
        );
        assert!(grant.covers("bash", &json!({"command": "ls"}), RiskLevel::Medium, 4));
        assert!(!grant.covers("bash", &json!({"command": "ls"}), RiskLevel::Medium, 5));
    }
}
