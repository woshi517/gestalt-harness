#![forbid(unsafe_code)]
//! Built-in workspace tools (`StdToolsExtension`): `read`, `write`, `edit`,
//! `glob`, `grep`, `bash`, `bash_output`, `bash_kill`.

pub mod bash;
pub mod fs_tools;
pub mod search_tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use gestalt_kernel::{
    Extension, ExtensionError, ExtensionRegistry, PolicyGate, ReplayMode, SessionTree,
    SideEffectClass, ToolSpec, UIRendererSpec,
};
use gestalt_runtime::{
    ArtifactManager, ConfigLoadOptions, ReplayOverlayOptions, RuntimeError, RuntimeRegistry,
    StreamDeltaFn, ToolExecutionContext, ToolHandler, ToolOutcome, WorkspaceTrustStore,
    load_and_merge_config, replay_session_tree_with_overlay,
};
use serde_json::{Value, json};

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

/// Optional self-evolution [`Extension`] registering `replay_verify` without
/// altering [`StdToolsExtension`]'s 8-tool schema surface.
#[derive(Debug, Clone, Default)]
pub struct EvolutionToolsExtension;

impl Extension for EvolutionToolsExtension {
    fn name(&self) -> &str {
        "gestalt-std-evolution-tools"
    }

    fn register(&self, registry: &mut ExtensionRegistry) -> Result<(), ExtensionError> {
        registry.tools.insert(
            "replay_verify".to_string(),
            ToolSpec {
                name: "replay_verify".to_string(),
                description: "Verify a staged Effect::Save artifact against recorded SessionTree traces offline.".to_string(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["stage_id", "session_paths"],
                    "properties": {
                        "stage_id": { "type": "string" },
                        "session_paths": { "type": "array", "items": { "type": "string" } }
                    }
                }),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
        );
        Ok(())
    }
}

/// Read-only tool verifying a staged `Effect::Save` (`stage_id`) against
/// recorded `SessionTree` JSONL traces offline.
#[derive(Debug, Clone, Default)]
pub struct ReplayVerifyTool;

impl ToolHandler for ReplayVerifyTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let stage_id = args
                .get("stage_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RuntimeError::ArtifactError("missing 'stage_id'".to_string()))?;
            let paths = args
                .get("session_paths")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RuntimeError::ArtifactError("missing 'session_paths' array".to_string())
                })?;

            let mgr = ArtifactManager::new(&ctx.workspace_root)?;
            let overlay = mgr.load_staged_overlay(stage_id)?;
            let mut reg = ExtensionRegistry::new();
            #[cfg(feature = "loops")]
            crate::StdExtension.register(&mut reg).map_err(|e| {
                RuntimeError::ArtifactError(format!("failed to register StdExtension: {e}"))
            })?;
            StdToolsExtension.register(&mut reg).map_err(|e| {
                RuntimeError::ArtifactError(format!("failed to register StdToolsExtension: {e}"))
            })?;
            let mut gate = PolicyGate::new(
                gestalt_kernel::BudgetCap::default(),
                None,
                ctx.sandbox_policy.clone(),
            );
            let ws_toml = ctx.workspace_root.join(".gestalt/gestalt.toml");
            if ws_toml.is_file() {
                let trust = WorkspaceTrustStore::default();
                let _ = load_and_merge_config(
                    ConfigLoadOptions {
                        workspace_root: &ctx.workspace_root,
                        global_toml_path: None,
                        workspace_toml_path: Some(&ws_toml),
                        cli_extensions: std::collections::BTreeMap::new(),
                        trust_store: &trust,
                        auto_trust_workspace: true,
                    },
                    &mut gate,
                    &mut reg,
                );
            }

            let mut all_passed = true;
            let mut reports = Vec::with_capacity(paths.len());
            for path_val in paths {
                let raw_path = path_val.as_str().ok_or_else(|| {
                    RuntimeError::ArtifactError("session_paths entries must be strings".to_string())
                })?;
                let resolved = ctx.path_guard.resolve_read(raw_path)?;
                let raw_jsonl = std::fs::read_to_string(&resolved)?;
                let (tree, _) = SessionTree::from_jsonl_str(&raw_jsonl)?;
                let report = replay_session_tree_with_overlay(
                    &tree,
                    ReplayOverlayOptions {
                        registry: &reg,
                        policy_gate: &gate,
                        blob_store: &ctx.blob_store,
                        mode: ReplayMode::Default,
                        ext_opts: None,
                        overlay: Some(&overlay),
                    },
                )
                .await?;
                let passed = report.is_parity();
                if !passed {
                    all_passed = false;
                }
                reports.push(json!({
                    "session_path": raw_path,
                    "passed": passed,
                    "phases_replayed": report.phases_replayed,
                    "divergences": report.divergences,
                }));
            }

            Ok(ToolOutcome {
                output: json!({
                    "stage_id": stage_id,
                    "passed": all_passed,
                    "reports": reports,
                }),
                is_error: false,
            })
        })
    }
}

/// Bind all [`StdToolsExtension`] [`gestalt_runtime::ToolHandler`]s into
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
    runtime
        .tools
        .insert("replay_verify".to_string(), Arc::new(ReplayVerifyTool));
    tasks
}
