//! `gestalt-tools` — Built-in tools + `ToolRegistry`
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

// Workspace lint configuration is inherited via Cargo.toml [lints] workspace = true

mod backends;
pub mod builtin_descriptors;
mod path;
mod registry;
pub mod response_shaping;
mod tools;

pub use registry::ToolRegistry;
pub use tools::{
    default_registry, BashInput, BashTool, FindFilesInput, FindFilesTool, PatchInput, PatchTool,
    ReadInput, ReadTool, SearchInput, SearchTool, WebFetchInput, WebFetchTool, WriteInput,
    WriteTool,
};
