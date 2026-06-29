use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, HarnessError>;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("policy error: {0}")]
    Policy(#[from] PolicyError),
    #[error("context error: {0}")]
    Context(#[from] ContextError),
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("trace error: {0}")]
    Trace(#[from] TraceError),
    #[error("approval error: {0}")]
    Approval(#[from] ApprovalError),
    #[error("cancelled: execution was cancelled")]
    Cancelled,
}

impl HarnessError {
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Provider(
                ProviderError::RateLimit { .. }
                    | ProviderError::ContextTooLong { .. }
                    | ProviderError::StreamInterrupted
            ) | Self::Tool(ToolError::Timeout { .. } | ToolError::OutputTooLarge { .. })
        )
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("provider authentication failed: {provider}")]
    AuthFailed { provider: String },
    #[error("provider rate limit; retry after {retry_after_secs:?}s")]
    RateLimit { retry_after_secs: Option<u64> },
    #[error("context too long: {tokens} tokens exceeds limit {limit}")]
    ContextTooLong { tokens: usize, limit: usize },
    #[error("invalid model: {model}")]
    InvalidModel { model: String },
    #[error("provider request timed out")]
    Timeout,
    #[error("provider stream interrupted")]
    StreamInterrupted,
    #[error("provider returned malformed tool call JSON: {details}")]
    MalformedToolCall { details: String },
    #[error("provider does not support requested capability: {capability}")]
    UnsupportedCapability { capability: String },
    #[error("provider returned an unexpected response: {details}")]
    UnexpectedResponse { details: String },
    #[error("provider transport error: {0}")]
    Transport(#[from] io::Error),
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("invalid input for {tool_name}: {reason}")]
    InvalidInput { tool_name: String, reason: String },
    #[error("tool execution failed: {0}")]
    ExecutionFailed(#[from] io::Error),
    #[error("tool timed out after {timeout_secs}s: {tool_name}")]
    Timeout {
        tool_name: String,
        timeout_secs: u64,
    },
    #[error("tool output too large: {tool_name} exceeded {limit} bytes")]
    OutputTooLarge { tool_name: String, limit: usize },
    #[error("path not allowed: {0}")]
    PathNotAllowed(String),
    #[error("network denied: {0}")]
    NetworkDenied(String),
    #[error("policy denied tool: {0}")]
    Denied(String),
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("invalid policy: {0}")]
    InvalidPolicy(String),
    #[error("policy denied: {0}")]
    Denied(String),
    #[error("policy I/O error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("invalid token budget: {0}")]
    InvalidBudget(String),
    #[error("context pipeline failed: {0}")]
    PipelineFailed(String),
    #[error("critical context exhausted: {0}")]
    Exhausted(String),
    #[error("durability contract failed: {0}")]
    DurabilityFailed(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing config field: {0}")]
    MissingField(String),
    #[error("unknown config key: {0}")]
    UnknownKey(String),
    #[error("invalid config value for {field}: {reason}")]
    InvalidValue { field: String, reason: String },
    #[error("feature '{feature}' is not enabled for {operation}")]
    FeatureDisabled { feature: String, operation: String },
}

#[derive(Debug, Error)]
pub enum TraceError {
    #[error("trace write failed: {0}")]
    WriteFailed(#[from] io::Error),
    #[error("trace read failed: {reason}")]
    ReadFailed { reason: String },
    #[error("invalid trace format at line {line}: {reason}")]
    InvalidFormat { line: usize, reason: String },
}

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("approval rejected: {0}")]
    Rejected(String),
    #[error("approval I/O error: {0}")]
    Io(#[from] io::Error),
}
