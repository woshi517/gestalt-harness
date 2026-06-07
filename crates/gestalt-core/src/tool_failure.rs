use serde::{Deserialize, Serialize};

/// Stable classification of why a tool call did not succeed.
///
/// The set of variants is intentionally small and matches the
/// `ToolErrorReport.kind` field. Every path that produces a failure
/// result for the model — validation, policy, approval, timeout,
/// execution — must classify itself into exactly one of these kinds
/// so the model and the trace system can react in a structured way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureKind {
    /// Provider returned a tool call for a name that is not registered
    /// in the catalog. The harness cannot resolve it to a canonical
    /// tool.
    ToolNotFound,
    /// JSON arguments could not be normalized into the expected
    /// shape. This is distinct from a schema mismatch: it indicates
    /// the input was malformed, not that valid JSON violated the
    /// schema.
    InvalidArguments,
    /// JSON arguments parsed but did not satisfy the (strict) input
    /// schema the provider saw. The `repair_guidance` should reference
    /// the schema the model can use to repair the call.
    SchemaMismatch,
    /// Two tool calls in the same assistant turn reused the same
    /// call id. Deterministic rejection, not repairable in-turn.
    DuplicateCallId,
    /// The provider referenced a namespace that is not allowed in the
    /// current harness configuration (e.g. MCP tools in yolo-mode).
    DisallowedNamespace,
    /// The tool's `execute` future did not complete inside its
    /// configured timeout.
    Timeout,
    /// Policy evaluation returned Denied.
    PolicyDenied,
    /// The user denied an approval-gated call.
    ApprovalDenied,
    /// The tool itself returned an `Err` or otherwise failed at
    /// runtime. Used as the catch-all when no more specific kind
    /// applies; treat as the "permanent failure" default.
    ExecutionFailed,
    /// The provider returned malformed streaming output that the
    /// harness could not turn into a complete assistant turn.
    Unknown,
}

impl ToolFailureKind {
    /// Whether this failure class is considered "transient" — i.e. a
    /// trusted, read-only, idempotent tool should be eligible for an
    /// automatic retry when its retry policy permits it.
    ///
    /// Only `Timeout` is unambiguously transient; all other failure
    /// kinds (including `ExecutionFailed`) are permanent in the sense
    /// that retrying with the same input is unlikely to succeed
    /// without model or user intervention.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Timeout)
    }

    /// Whether this failure kind means the tool never ran — i.e. the
    /// failure was caught during validation, policy, or approval
    /// before any process was spawned. Trace consumers use this to
    /// avoid counting pre‑execution rejections as executed calls.
    pub fn is_pre_execution(self) -> bool {
        matches!(
            self,
            Self::ToolNotFound
                | Self::InvalidArguments
                | Self::SchemaMismatch
                | Self::DuplicateCallId
                | Self::DisallowedNamespace
                | Self::PolicyDenied
                | Self::ApprovalDenied
                | Self::Unknown
        )
    }
}

impl std::fmt::Display for ToolFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::ToolNotFound => "ToolNotFound",
            Self::InvalidArguments => "InvalidArguments",
            Self::SchemaMismatch => "SchemaMismatch",
            Self::DuplicateCallId => "DuplicateCallId",
            Self::DisallowedNamespace => "DisallowedNamespace",
            Self::Timeout => "Timeout",
            Self::PolicyDenied => "PolicyDenied",
            Self::ApprovalDenied => "ApprovalDenied",
            Self::ExecutionFailed => "ExecutionFailed",
            Self::Unknown => "Unknown",
        };
        f.write_str(label)
    }
}

/// Stable, structured payload describing why a tool call failed.
///
/// `kind` is the machine-readable classification; `message` is the
/// human-readable explanation; `repair_guidance` is optional advice
/// the model can use to recover in a subsequent turn (e.g. for
/// `SchemaMismatch` it includes the expected schema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolErrorReport {
    pub kind: ToolFailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_guidance: Option<String>,
}

impl ToolErrorReport {
    pub fn new(kind: ToolFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            repair_guidance: None,
        }
    }

    pub fn with_repair(mut self, guidance: impl Into<String>) -> Self {
        self.repair_guidance = Some(guidance.into());
        self
    }

    /// Render the report into a single string suitable for embedding
    /// in `Message::ToolResult.content` so the model still sees a
    /// readable explanation even if it does not parse the structured
    /// fields.
    pub fn render_for_model(&self) -> String {
        match &self.repair_guidance {
            Some(guidance) => {
                format!("[{}] {}\nrepair: {}", self.kind, self.message, guidance)
            }
            None => format!("[{}] {}", self.kind, self.message),
        }
    }
}
