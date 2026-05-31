//! # gestalt-core
//!
//! Core traits, event types, agent loop, and session state for gestalt-harness.
//!
//! ## Ownership Boundary
//!
//! This crate defines the interfaces that all other gestalt crates depend on.
//! It contains **zero** file I/O, **zero** HTTP calls, and **zero** concrete
//! tool or provider implementations.
//!
//! ## Key Exports (Phase 1)
//!
//! - `AgentLoop` — the sacred loop (~200 lines)
//! - `AgentEvent` — the unified event enum
//! - `Message`, `ContentBlock` — transcript types
//! - `Provider`, `Tool`, `PolicyEngine`, `ContextPipeline` — core traits
//! - `Session`, `SessionConfig`, `RunResult` — session state
//! - `HarnessError` — error taxonomy
