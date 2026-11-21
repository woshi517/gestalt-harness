#![forbid(unsafe_code)]
//! In-process workspace file tools (`read`, `write`, `edit`) guarded by
//! `WorkspacePathGuard` and emitting structured `ToolUi` diff previews.

use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use gestalt_kernel::SideEffectClass;
use gestalt_runtime::{
    RuntimeError, StreamDeltaFn, ToolExecutionContext, ToolHandler, ToolOutcome,
};
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};

const MAX_READ_LINES: usize = 800;
const MAX_READ_BYTES: usize = 28 * 1024;
static WRITE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// `read` tool handler (`SideEffectClass::Read`).
#[derive(Debug, Default)]
pub struct ReadTool;

impl ToolHandler for ReadTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'path'".to_string()),
                    is_error: true,
                });
            };
            let start_line = args
                .get("start_line")
                .and_then(Value::as_u64)
                .map_or(1, |n| n.max(1) as usize);
            let end_line = args
                .get("end_line")
                .and_then(Value::as_u64)
                .map(|n| n as usize);
            let byte_offset = args
                .get("byte_offset")
                .and_then(Value::as_u64)
                .map(|n| n as usize);

            let resolved = ctx.path_guard.resolve_read(path)?;
            if !resolved.is_file() {
                return Ok(ToolOutcome {
                    output: Value::String(format!("file not found: '{path}'")),
                    is_error: true,
                });
            }
            let bytes = fs::read(&resolved)?;
            let text = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => {
                    return Ok(ToolOutcome {
                        output: Value::String(format!("file '{path}' is not valid UTF-8")),
                        is_error: true,
                    });
                }
            };

            if let Some(offset) = byte_offset {
                let mut start = offset.min(text.len());
                while start < text.len() && !text.is_char_boundary(start) {
                    start += 1;
                }
                let mut end = (start + MAX_READ_BYTES).min(text.len());
                while end > start && !text.is_char_boundary(end) {
                    end -= 1;
                }
                let truncated = end < text.len();
                return Ok(ToolOutcome {
                    output: json!({
                        "path": path,
                        "byte_offset": start,
                        "next_byte_offset": truncated.then_some(end),
                        "truncated": truncated,
                        "content": &text[start..end],
                    }),
                    is_error: false,
                });
            }

            let all_lines: Vec<&str> = text.lines().collect();
            let total_lines = all_lines.len();
            if total_lines == 0 {
                return Ok(ToolOutcome {
                    output: json!({
                        "path": path,
                        "start_line": 1,
                        "end_line": 0,
                        "total_lines": 0,
                        "truncated": false,
                        "content": "",
                    }),
                    is_error: false,
                });
            }

            let start_idx = start_line.saturating_sub(1).min(total_lines);
            let max_end = (start_idx + MAX_READ_LINES).min(total_lines);
            let end_idx = end_line
                .map_or(max_end, |e| e.min(total_lines))
                .min(max_end)
                .max(start_idx);

            let mut out = String::new();
            let mut actual_end = start_idx;
            let mut truncated = end_idx < end_line.unwrap_or(total_lines).min(total_lines);
            for (offset, line) in all_lines[start_idx..end_idx].iter().enumerate() {
                let line_no = start_idx + offset + 1;
                let formatted = format!("{line_no}: {line}\n");
                if out.len() + formatted.len() > MAX_READ_BYTES && !out.is_empty() {
                    truncated = true;
                    break;
                }
                out.push_str(&formatted);
                actual_end = line_no;
            }

            Ok(ToolOutcome {
                output: json!({
                    "path": path,
                    "start_line": start_line,
                    "end_line": actual_end,
                    "total_lines": total_lines,
                    "truncated": truncated,
                    "content": out,
                }),
                is_error: false,
            })
        })
    }
}

/// `write` tool handler (`Idempotent` on new file, `Irreversible` on overwrite).
#[derive(Debug, Default)]
pub struct WriteTool;

impl ToolHandler for WriteTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'path'".to_string()),
                    is_error: true,
                });
            };
            let Some(content) = args.get("content").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'content'".to_string()),
                    is_error: true,
                });
            };
            let overwrite = args
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let (resolved, side_effect) = ctx.path_guard.resolve_write(path)?;
            if side_effect == SideEffectClass::Irreversible && !overwrite {
                return Ok(ToolOutcome {
                    output: Value::String(format!(
                        "file '{path}' already exists; pass overwrite: true to replace it"
                    )),
                    is_error: true,
                });
            }

            if let Some(parent) = resolved.parent() {
                fs::create_dir_all(parent)?;
                // Re-verify parent confinement after creation.
                ctx.path_guard.ensure_not_protected(&resolved)?;
            }
            atomic_write(&resolved, content.as_bytes())?;

            Ok(ToolOutcome {
                output: json!({
                    "path": path,
                    "bytes_written": content.len(),
                    "side_effect": side_effect,
                }),
                is_error: false,
            })
        })
    }
}

/// `edit` tool handler (`Idempotent`) applying unique exact-match replacements
/// and emitting structured `RunEvent::ToolUi { renderer: "diff_preview", .. }`.
#[derive(Debug, Default)]
pub struct EditTool;

impl ToolHandler for EditTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'path'".to_string()),
                    is_error: true,
                });
            };
            let Some(target) = args.get("target").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'target'".to_string()),
                    is_error: true,
                });
            };
            let Some(replacement) = args.get("replacement").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'replacement'".to_string()),
                    is_error: true,
                });
            };

            if target.is_empty() {
                return Ok(ToolOutcome {
                    output: Value::String("edit target must not be empty".to_string()),
                    is_error: true,
                });
            }

            let (resolved, _) = ctx.path_guard.resolve_write(path)?;
            if !resolved.is_file() {
                return Ok(ToolOutcome {
                    output: Value::String(format!("file not found: '{path}'")),
                    is_error: true,
                });
            }

            let before = fs::read_to_string(&resolved)?;
            let match_count = before.matches(target).count();
            if match_count == 0 {
                return Ok(ToolOutcome {
                    output: Value::String(format!("target text not found in '{path}'")),
                    is_error: true,
                });
            }
            if match_count > 1 {
                return Ok(ToolOutcome {
                    output: Value::String(format!(
                        "target text matched {match_count} locations in '{path}'; provide more context for a unique match"
                    )),
                    is_error: true,
                });
            }

            let after = before.replacen(target, replacement, 1);
            let diff = TextDiff::from_lines(&before, &after);
            let mut additions: u64 = 0;
            let mut deletions: u64 = 0;
            for change in diff.iter_all_changes() {
                match change.tag() {
                    ChangeTag::Insert => additions += 1,
                    ChangeTag::Delete => deletions += 1,
                    ChangeTag::Equal => {}
                }
            }
            let unified_diff = diff
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{path}"), &format!("b/{path}"))
                .to_string();

            atomic_write(&resolved, after.as_bytes())?;

            let diff_payload = json!({
                "path": path,
                "additions": additions,
                "deletions": deletions,
                "unified_diff": unified_diff,
            });
            ctx.emit_tool_ui("diff_preview", diff_payload.clone());

            Ok(ToolOutcome {
                output: diff_payload,
                is_error: false,
            })
        })
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let seq = WRITE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!(".{file_name}.tmp.{}.{seq}", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::SandboxPolicy;
    use gestalt_runtime::{
        BlobStore, CancellationToken, WorkspacePathGuard, default_sandbox_driver,
    };
    use std::collections::BTreeSet;

    fn make_ctx(root: &Path) -> ToolExecutionContext {
        let protected: BTreeSet<String> = [".gestalt/memories/".to_string()].into_iter().collect();
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            cwd: root.to_path_buf(),
            turn_id: "t1".into(),
            phase_id: "p1".into(),
            effect_id: "e1".into(),
            sandbox_policy: SandboxPolicy {
                network: false,
                protected_paths: protected.clone(),
                max_spawn_depth: 1,
            },
            path_guard: WorkspacePathGuard::new(root, &protected).expect("guard"),
            sandbox_driver: default_sandbox_driver(true),
            blob_store: BlobStore::new(root),
            cancel: CancellationToken::new(),
            event_sink: None,
            redactor: Default::default(),
        }
    }

    #[tokio::test]
    async fn read_write_edit_and_protected_path_enforcement() {
        let dir = std::env::temp_dir().join(format!("gestalt_fs_tools_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".gestalt/memories")).expect("mkdir");
        let canon = fs::canonicalize(&dir).expect("canon");
        let ctx = make_ctx(&canon);
        let noop = |_: &str, _: Value| {};

        // Write new file
        let w1 = WriteTool
            .execute(
                &json!({ "path": "src/main.rs", "content": "fn main() {\n    println!(\"v1\");\n}\n" }),
                &ctx,
                &noop,
            )
            .await
            .expect("write");
        assert!(!w1.is_error);

        // Existing file without overwrite: true returns recoverable error
        let w2 = WriteTool
            .execute(
                &json!({ "path": "src/main.rs", "content": "clobber" }),
                &ctx,
                &noop,
            )
            .await
            .expect("write existing");
        assert!(w2.is_error);

        // Read line range
        let r1 = ReadTool
            .execute(
                &json!({ "path": "src/main.rs", "start_line": 2, "end_line": 2 }),
                &ctx,
                &noop,
            )
            .await
            .expect("read");
        assert!(!r1.is_error);
        assert!(r1.output["content"].as_str().expect("s").contains("v1"));

        // Edit file uniquely
        let e1 = EditTool
            .execute(
                &json!({ "path": "src/main.rs", "target": "v1", "replacement": "v2" }),
                &ctx,
                &noop,
            )
            .await
            .expect("edit");
        assert!(!e1.is_error);
        assert_eq!(e1.output["additions"], 1);
        assert_eq!(e1.output["deletions"], 1);

        // Protected path write fails closed with SandboxViolation
        let prot_err = WriteTool
            .execute(
                &json!({ "path": ".gestalt/memories/MEMORY.md", "content": "hack" }),
                &ctx,
                &noop,
            )
            .await
            .expect_err("must deny protected path");
        assert!(matches!(prot_err, RuntimeError::SandboxViolation(_)));

        let _ = fs::remove_dir_all(&dir);
    }
}
