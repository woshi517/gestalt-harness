//! Deliberate public contracts for embedding and controlling Gestalt.
//!
//! Only items reachable through [`v1`] are covered by the v0.1 compatibility
//! policy. Runtime subsystems outside this namespace are implementation detail
//! or explicitly unstable.

/// Stable v0.1 embedding and runtime-control API.
pub mod v1 {
    pub use crate::artifact_store::{
        ArtifactStore, FilesystemArtifactStore, InMemoryArtifactStore,
    };
    pub use crate::builder::AgentRuntimeBuilder;
    pub use crate::config::RuntimeConfig;
    pub use crate::context::assembler::{
        estimate_message_tokens, estimate_text_tokens, ContextMessageAssembler,
    };
    pub use crate::control::contract::*;
    pub use crate::control::{
        ControlHostOptions, InMemoryControlHost, MockControlHost, RuntimeBackedControlHost,
        DEFAULT_CONTROL_QUEUE_CAPACITY, MAX_ARTIFACT_READ_BYTES,
    };
    pub use crate::error::{Result, RuntimeError};
    pub use crate::runtime::{AgentRuntime, UserInput};
}
