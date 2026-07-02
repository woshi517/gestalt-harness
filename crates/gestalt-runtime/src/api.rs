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
    pub use crate::context::report::{
        load_context_build_report, persist_context_build_report, CapturedContributionV1,
        ContextBuildReportV1, ContextOmissionReportV1, ContextPersistenceDiagnosticV1,
        ContextPressureV1, ContextSourceReportV1, CONTEXT_BUILD_REPORT_SCHEMA_VERSION,
        MAX_CAPTURED_CONTRIBUTIONS_BYTES, MAX_CAPTURED_CONTRIBUTION_BYTES,
    };
    pub use crate::control::contract::*;
    pub use crate::control::{
        ControlHostOptions, InMemoryControlHost, MockControlHost, RuntimeBackedControlHost,
        DEFAULT_CONTROL_QUEUE_CAPACITY, MAX_ARTIFACT_READ_BYTES,
    };
    pub use crate::error::{Result, RuntimeError};
    pub use crate::runtime::{AgentRuntime, UserInput};
    #[cfg(feature = "trace")]
    pub use crate::trace::{
        project_client_event_line, ClientEventPayloadV1, ClientEventRecordV1,
        CLIENT_EVENT_SCHEMA_VERSION,
    };
    pub use gestalt_core::ContextCaptureMode;
}
