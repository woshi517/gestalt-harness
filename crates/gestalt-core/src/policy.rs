use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{event::PolicyStatus, session::ExecutionMode, tool::RiskLevel};

#[async_trait]
pub trait PolicyEngine: Send + Sync {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision;

    async fn evaluate_cancellable(
        &self,
        request: PolicyRequest,
        cancel_token: &crate::cancel::CancelToken,
    ) -> Result<PolicyDecision, crate::error::HarnessError> {
        tokio::select! {
            res = self.evaluate(request) => Ok(res),
            _ = cancel_token.cancelled() => Err(crate::error::HarnessError::Cancelled),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    pub risk: RiskLevel,
    pub mode: ExecutionMode,
    pub working_dir: PathBuf,
    pub workspace_root: Option<PathBuf>,
    pub user_approved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub status: PolicyStatus,
    pub reason: Option<String>,
    pub policy_source: String,
}

impl PolicyDecision {
    pub fn allowed(reason: Option<String>) -> Self {
        Self {
            status: PolicyStatus::Allowed,
            reason,
            policy_source: String::new(),
        }
    }

    pub fn confirm(reason: String, source: String) -> Self {
        Self {
            status: PolicyStatus::Confirm,
            reason: Some(reason),
            policy_source: source,
        }
    }

    pub fn denied(reason: String, source: String) -> Self {
        Self {
            status: PolicyStatus::Denied,
            reason: Some(reason),
            policy_source: source,
        }
    }

    pub fn is_allowed(&self) -> bool {
        matches!(self.status, PolicyStatus::Allowed)
    }
}
