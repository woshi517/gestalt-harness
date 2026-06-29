//! `gestalt-tools` — Built-in tools + `ToolRegistry`
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

// Workspace lint configuration is inherited via Cargo.toml [lints] workspace = true

pub mod backends;
pub mod builtin_descriptors;
pub mod path;
pub mod registry;
pub mod tools;

pub use registry::ToolRegistry;
pub use tools::{
    default_registry, parse_patch, BashInput, BashTool, FindFilesInput, FindFilesTool, PatchInput,
    PatchOperation, PatchTool, ReadInput, ReadTool, SearchInput, SearchReplace, SearchTool,
    WebFetchInput, WebFetchTool, WriteInput, WriteTool,
};
