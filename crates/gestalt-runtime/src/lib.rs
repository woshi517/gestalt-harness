#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Reusable Tokio I/O host, OS sandbox drivers, and capability primitives for Gestalt.

pub mod artifacts;
pub mod blobs;
pub mod config;
pub mod credentials;
pub mod driver;
pub mod ext_ipc;
pub mod mcp;
pub mod provider;
pub mod sandbox;
mod stdio_rpc;

use gestalt_kernel::KernelError;
use thiserror::Error;
use tokio::sync::watch;

pub use artifacts::{ArtifactManager, LedgerEntry, StageManifest};
pub use blobs::{BlobStore, DEFAULT_INLINE_SPILL_BYTES};
pub use config::{
    BudgetConfig, ConfigLoadOptions, DefaultsConfig, ExtensionProtocol, ExternalExtensionConfig,
    GestaltConfig, PolicyConfig, PresetConfig, ProviderConfig, ResolvedRuntimeConfig,
    SandboxConfig, WorkspaceTrustStore, load_and_merge_config,
};
pub use credentials::{
    CredentialRecord, CredentialRequest, CredentialResolver, CredentialStore, REDACTED_MARKER,
    StreamRedactor,
};
pub use driver::{
    CommandHandler, EventSink, ExternalPolicyBridgeRule, ExternalReplayOptions,
    HeadlessPauseHandler, LiveSteeringMessage, PauseHandler, RuntimeHost, RuntimeRegistry,
    SessionLockGuard, ToolExecutionContext, ToolHandler, ToolOutcome, TurnOutcome,
    capture_worktree_diff_blob, create_worktree, current_epoch_ms, prune_clean_worktrees,
    replay_session_tree, validate_tool_args,
};
pub use ext_ipc::{
    ExtIpcHost, ExtIpcWorker, ExternalBuilderRequest, ExternalCommandResponse,
    ExternalDecoratorStepRequest, ExternalLoopStepRequest, ExternalLoopStepResponse,
};
pub use mcp::{MCP_PROTOCOL_VERSION, McpClient, McpServerInfo};
pub use provider::{
    ModelProvider, ModelRequest, ModelResponse, ProviderContext, ProviderRegistry,
    ResolvedProviderTarget, SharedHttpClient, SseEvent, SseLineParser, StreamDeltaFn,
    classify_http_error, compute_cost_micro_usd, estimate_tokens_fallback,
};
pub use sandbox::{
    INTERNAL_PROTECTED_PREFIXES, LinuxBwrapDriver, MacOsSeatbeltDriver, SandboxCommand,
    SandboxDriver, SandboxOutput, WorkspacePathGuard, default_sandbox_driver,
};

/// Classification of provider HTTP/SSE failures for retry and compaction handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// Transient rate limit (`429`, `529`), `5xx`, or network reset safe to retry.
    Retryable,
    /// Input exceeded the target model's context window (triggers compaction).
    ContextLengthExceeded,
    /// Missing, expired, or rejected authentication credentials (`401`, `403`).
    AuthError,
    /// Non-retryable request or payload error.
    Fatal,
}

impl ProviderErrorKind {
    /// Canonical lowercase tag embedded in `StepResult::Error.message` so
    /// user-space decorators (e.g., `compaction`) can match error kinds reliably.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::ContextLengthExceeded => "context_length_exceeded",
            Self::AuthError => "auth_error",
            Self::Fatal => "fatal",
        }
    }

    /// Parse a [`ProviderErrorKind`] tag prefix from a `StepResult::Error` message.
    #[must_use]
    pub fn from_error_message(message: &str) -> Option<Self> {
        let rest = message.strip_prefix('[')?;
        let (tag, _) = rest.split_once(']')?;
        match tag {
            "retryable" => Some(Self::Retryable),
            "context_length_exceeded" => Some(Self::ContextLengthExceeded),
            "auth_error" => Some(Self::AuthError),
            "fatal" => Some(Self::Fatal),
            _ => None,
        }
    }
}

/// Structured errors returned by `gestalt-runtime` host primitives and drivers.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Error propagated from `gestalt-kernel`.
    #[error(transparent)]
    Kernel(#[from] KernelError),
    /// Standard I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// TOML deserialization failure.
    #[error("TOML parse error: {0}")]
    TomlDe(#[from] toml::de::Error),
    /// TOML serialization failure.
    #[error("TOML encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    /// HTTP client or transport failure.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// Workspace `.gestalt/gestalt.toml` has not been approved in `trusted_workspaces.json`.
    #[error("untrusted workspace '{workspace_root}' (config sha256: {config_digest})")]
    UntrustedWorkspace {
        /// Canonical workspace root path.
        workspace_root: String,
        /// SHA-256 hex digest of `.gestalt/gestalt.toml`.
        config_digest: String,
    },
    /// Credential file has overly permissive Unix mode (must be `0600`).
    #[error("insecure permissions ({mode:o}) on credential file '{path}'; expected 0600")]
    InsecureCredentialPermissions {
        /// Credential file path.
        path: String,
        /// Observed permission bits.
        mode: u32,
    },
    /// Required OS sandbox binary is unavailable or failed to launch.
    #[error("sandbox driver '{driver}' unavailable: {reason}")]
    SandboxUnavailable {
        /// Sandbox driver name.
        driver: String,
        /// Detailed reason.
        reason: String,
    },
    /// Path or sandbox boundary violation (e.g., symlink escape or `protected_paths` write).
    #[error("sandbox violation: {0}")]
    SandboxViolation(String),
    /// Advisory session lock is held by another active process.
    #[error("session locked by '{holder}' ({lock_path})")]
    SessionLocked {
        /// Lock file path.
        lock_path: String,
        /// Holder metadata string.
        holder: String,
    },
    /// Out-of-process IPC call exceeded its configured timeout.
    #[error("IPC timeout calling '{method}' on extension '{ext_name}' after {timeout_ms} ms")]
    IpcTimeout {
        /// Extension name.
        ext_name: String,
        /// JSON-RPC method name.
        method: String,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// Out-of-process IPC protocol or worker crash failure.
    #[error("IPC error in extension '{ext_name}': {message}")]
    IpcProtocol {
        /// Extension name.
        ext_name: String,
        /// Diagnostic message.
        message: String,
    },
    /// Git worktree creation or diff capture error.
    #[error("worktree error: {0}")]
    WorktreeError(String),
    /// Artifact staging, diff patching, promotion, rollback, or blob error.
    #[error("artifact error: {0}")]
    ArtifactError(String),
    /// Model provider invocation error with classified failure kind.
    #[error("provider error ({kind:?}): {message}")]
    ProviderError {
        /// Failure classification.
        kind: ProviderErrorKind,
        /// Diagnostic message.
        message: String,
    },
    /// `Effect::Pause` could not be resolved in non-interactive headless mode.
    #[error("headless pause denied: {0}")]
    HeadlessPauseDenied(String),
}

/// Cooperative cancellation token shared across async I/O, HTTP streams, and
/// sandboxed subprocesses without adding external dependencies.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    /// Create a fresh uncancelled [`CancellationToken`].
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    /// Signal cancellation to all clones of this token.
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }

    /// Return `true` if [`CancellationToken::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// Wait asynchronously until [`CancellationToken::cancel`] is triggered.
    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        if *rx.borrow() {
            return;
        }
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    }
}
