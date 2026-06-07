//! `gestalt-tools` — Built-in tools + `ToolRegistry`
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

// Workspace lint configuration is inherited via Cargo.toml [lints] workspace = true

pub mod builtin_descriptors;
pub mod response_shaping;
mod path;
mod registry;
mod tools;

pub use registry::ToolRegistry;
pub use tools::{
    default_registry, BashInput, BashTool, PatchInput, PatchTool, ReadInput, ReadTool, SearchInput,
    SearchTool, WebFetchInput, WebFetchTool, WriteInput, WriteTool,
};
