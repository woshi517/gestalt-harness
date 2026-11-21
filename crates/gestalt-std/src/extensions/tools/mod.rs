#![forbid(unsafe_code)]
//! Built-in workspace tools (`StdToolsExtension`): `read`, `write`, `edit`,
//! `glob`, `grep`, `bash`, `bash_output`, `bash_kill`.

pub mod bash;
pub mod fs_tools;
pub mod search_tools;

use std::sync::Arc;

use gestalt_kernel::{
    Extension, ExtensionError, ExtensionRegistry, SideEffectClass, ToolSpec, UIRendererSpec,
};
use gestalt_runtime::RuntimeRegistry;
use serde_json::json;

pub use bash::{BackgroundTaskManager, BashKillTool, BashOutputTool, BashTool};
pub use fs_tools::{EditTool, ReadTool, WriteTool};
pub use search_tools::{GlobTool, GrepTool};

/// Built-in [`Extension`] registering all 8 core workspace tools and their
/// declarative `UISurface` renderers (`diff_preview`, `bash_output`).
#[derive(Debug, Clone, Default)]
pub struct StdToolsExtension;

impl Extension for StdToolsExtension {
    fn name(&self) -> &str {
        "gestalt-std-tools"
    }

    fn register(&self, registry: &mut ExtensionRegistry) -> Result<(), ExtensionError> {
        let specs = [
            ToolSpec {
                name: "read".to_string(),
                description: "Read a UTF-8 workspace file (or .gestalt/blobs/<sha256>) with optional line range or byte offset.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": { "type": "string" },
                        "start_line": { "type": "integer" },
                        "end_line": { "type": "integer" },
                        "byte_offset": { "type": "integer" }
                    }
                }),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
            ToolSpec {
                name: "write".to_string(),
                description: "Create or overwrite a file inside the workspace (protected_paths denied).".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" },
                        "overwrite": { "type": "boolean" }
                    }
                }),
                default_side_effect: SideEffectClass::Idempotent,
                source_extension: None,
            },
            ToolSpec {
                name: "edit".to_string(),
                description: "Apply a targeted unique-match replacement to a workspace file and emit a structured diff preview.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["path", "target", "replacement"],
                    "properties": {
                        "path": { "type": "string" },
                        "target": { "type": "string" },
                        "replacement": { "type": "string" }
                    }
                }),
                default_side_effect: SideEffectClass::Idempotent,
                source_extension: None,
            },
            ToolSpec {
                name: "glob".to_string(),
                description: "Find workspace file paths matching a glob pattern (respecting .gitignore).".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["pattern"],
                    "properties": {
                        "pattern": { "type": "string" },
                        "root": { "type": "string" }
                    }
                }),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
            ToolSpec {
                name: "grep".to_string(),
                description: "Search workspace files for literal or regex matches with 1-based line numbers.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["pattern"],
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string" },
                        "glob_filter": { "type": "string" },
                        "literal": { "type": "boolean" }
                    }
                }),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
            ToolSpec {
                name: "bash".to_string(),
                description: "Execute a shell command inside the OS sandbox with explicit cwd, timeout, and optional background handle.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": { "type": "string" },
                        "cwd": { "type": "string" },
                        "timeout_ms": { "type": "integer" },
                        "background": { "type": "boolean" },
                        "scope": { "type": "string" }
                    }
                }),
                default_side_effect: SideEffectClass::Irreversible,
                source_extension: None,
            },
            ToolSpec {
                name: "bash_output".to_string(),
                description: "Inspect buffered stdout/stderr and status of a background bash task.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["task_id"],
                    "properties": {
                        "task_id": { "type": "string" }
                    }
                }),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
            ToolSpec {
                name: "bash_kill".to_string(),
                description: "Terminate a running background bash task process group by task_id.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["task_id"],
                    "properties": {
                        "task_id": { "type": "string" }
                    }
                }),
                default_side_effect: SideEffectClass::Irreversible,
                source_extension: None,
            },
        ];

        for spec in specs {
            registry.tools.insert(spec.name.clone(), spec);
        }

        registry.ui.renderers.insert(
            "diff_preview".to_string(),
            UIRendererSpec {
                id: "diff_preview".to_string(),
                target_kind: "edit".to_string(),
                title: "File Diff Preview".to_string(),
                schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "additions": { "type": "integer" },
                        "deletions": { "type": "integer" },
                        "unified_diff": { "type": "string" }
                    }
                })),
            },
        );
        registry.ui.renderers.insert(
            "bash_output".to_string(),
            UIRendererSpec {
                id: "bash_output".to_string(),
                target_kind: "bash".to_string(),
                title: "Shell Execution Output".to_string(),
                schema: None,
            },
        );
        Ok(())
    }
}

/// Bind all 8 [`StdToolsExtension`] [`gestalt_runtime::ToolHandler`]s into
/// [`RuntimeRegistry`] and return the shared [`BackgroundTaskManager`] for
/// Turn/Session reaping.
pub fn register_std_tools(runtime: &mut RuntimeRegistry) -> Arc<BackgroundTaskManager> {
    let tasks = Arc::new(BackgroundTaskManager::new());
    runtime.tools.insert("read".to_string(), Arc::new(ReadTool));
    runtime
        .tools
        .insert("write".to_string(), Arc::new(WriteTool));
    runtime.tools.insert("edit".to_string(), Arc::new(EditTool));
    runtime.tools.insert("glob".to_string(), Arc::new(GlobTool));
    runtime.tools.insert("grep".to_string(), Arc::new(GrepTool));
    runtime
        .tools
        .insert("bash".to_string(), Arc::new(BashTool::new(tasks.clone())));
    runtime.tools.insert(
        "bash_output".to_string(),
        Arc::new(BashOutputTool::new(tasks.clone())),
    );
    runtime.tools.insert(
        "bash_kill".to_string(),
        Arc::new(BashKillTool::new(tasks.clone())),
    );
    tasks
}
