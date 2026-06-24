use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalVerifierReport {
    pub component_id: String,
    pub passed: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalVerifierRequest {
    pub session_id: String,
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait ExternalVerifier: Send + Sync {
    async fn verify_external(
        &self,
        request: ExternalVerifierRequest,
    ) -> crate::Result<ExternalVerifierReport>;
}
