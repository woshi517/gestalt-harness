#![forbid(unsafe_code)]
//! Kernel and extension error types with manual `Display` and `std::error::Error`
//! implementations so `gestalt-kernel` stays at `<= 2` direct dependencies.

use std::fmt;

/// Error returned by `Extension::register` when populating `ExtensionRegistry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    /// Two extensions attempted to register incompatible definitions or a policy relaxation.
    PolicyViolation(String),
    /// Registration failed due to an invalid slot descriptor or name.
    InvalidDescriptor {
        /// Registry slot name (e.g., `"loops"`, `"tools"`).
        slot: String,
        /// Human-readable reason for rejection.
        reason: String,
    },
}

impl fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PolicyViolation(reason) => write!(f, "extension policy violation: {reason}"),
            Self::InvalidDescriptor { slot, reason } => {
                write!(f, "invalid extension descriptor in slot '{slot}': {reason}")
            }
        }
    }
}

impl std::error::Error for ExtensionError {}

/// Errors produced by `SessionTree`, `PolicyGate`, `ExtensionRegistry`, and `KernelRuntime`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelError {
    /// Failed to parse or serialize JSON / JSONL.
    Serialization(String),
    /// A `SessionTree` node failed SHA-256 digest, parent linkage, or schema validation.
    CorruptSessionTree {
        /// Node ID where corruption was detected (if parsed).
        node_id: Option<String>,
        /// 1-based JSONL line number (if applicable).
        line: Option<usize>,
        /// Detailed corruption description.
        reason: String,
    },
    /// Attempted to append a duplicate node ID to `SessionTree`.
    DuplicateNodeId(String),
    /// Referenced `SessionTree` node ID or branch ID does not exist.
    UnknownNode(String),
    /// Higher-tier config or Phase override attempted to weaken a baseline policy.
    PolicyRelaxationForbidden(String),
    /// Referenced extension slot (`loop`, `decorator`, `builder`, `preset`) is not registered.
    UnresolvedExtensionSlot {
        /// Slot category (`"preset"`, `"loop"`, `"decorator"`, `"builder"`).
        slot: String,
        /// Missing identifier.
        name: String,
    },
    /// Invalid kernel state-machine transition (e.g., polling with no active Turn).
    InvalidState(String),
    /// An `Extension::register` call failed.
    Extension(ExtensionError),
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::CorruptSessionTree {
                node_id,
                line,
                reason,
            } => write!(
                f,
                "corrupt session tree (node={}, line={}): {reason}",
                node_id.as_deref().unwrap_or("unknown"),
                line.map_or_else(|| "none".to_string(), |n| n.to_string())
            ),
            Self::DuplicateNodeId(id) => write!(f, "duplicate session node id: {id}"),
            Self::UnknownNode(id) => write!(f, "unknown session node id: {id}"),
            Self::PolicyRelaxationForbidden(msg) => {
                write!(f, "policy relaxation forbidden: {msg}")
            }
            Self::UnresolvedExtensionSlot { slot, name } => {
                write!(f, "unresolved {slot} '{name}' in extension registry")
            }
            Self::InvalidState(msg) => write!(f, "invalid kernel state: {msg}"),
            Self::Extension(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for KernelError {}

impl From<ExtensionError> for KernelError {
    fn from(err: ExtensionError) -> Self {
        Self::Extension(err)
    }
}

impl From<serde_json::Error> for KernelError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}
