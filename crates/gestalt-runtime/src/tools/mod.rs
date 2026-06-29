//! `gestalt-tools` — Built-in tools + `ToolRegistry`
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

// Workspace lint configuration is inherited via Cargo.toml [lints] workspace = true

pub mod backends;
mod bash;
pub mod builtin_descriptors;
mod builtins;
mod common;
mod find_files;
mod patch;
pub mod path;
mod read;
pub mod registry;
mod search;
#[cfg(test)]
mod test_support;
mod web_fetch;
mod write;

pub use bash::{BashInput, BashTool};
pub use builtins::default_registry;
pub use find_files::{FindFilesInput, FindFilesTool};
pub use patch::{parse_patch, PatchInput, PatchOperation, PatchTool, SearchReplace};
pub use read::{ReadInput, ReadTool};
pub use registry::ToolRegistry;
pub use search::{SearchInput, SearchTool};
pub use web_fetch::{WebFetchInput, WebFetchTool};
pub use write::{WriteInput, WriteTool};
