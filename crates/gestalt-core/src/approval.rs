use async_trait::async_trait;
use serde_json::Value;

use crate::policy::PolicyDecision;

#[async_trait]
pub trait ApprovalProvider: Send + Sync {
    async fn approve(&self, request: ApprovalRequest) -> ApprovalDecision;
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
    async fn approve(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Approve
    }
}

pub struct DenyApprovalProvider;

#[async_trait]
impl ApprovalProvider for DenyApprovalProvider {
    async fn approve(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}
