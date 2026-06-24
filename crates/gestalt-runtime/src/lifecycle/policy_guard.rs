use async_trait::async_trait;
use gestalt_core::policy::PolicyDecision;

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyGuardRequest {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

#[async_trait]
pub trait PolicyGuard: Send + Sync {
    async fn evaluate_policy(&self, request: PolicyGuardRequest) -> crate::Result<PolicyDecision>;
}
