#![forbid(unsafe_code)]
//! In-process `.gitignore`-aware workspace search tools (`glob`, `grep`).

use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use gestalt_runtime::{
    RuntimeError, StreamDeltaFn, ToolExecutionContext, ToolHandler, ToolOutcome,
};
use ignore::WalkBuilder;
use regex_lite::Regex;
use serde_json::{Value, json};

const MAX_GLOB_RESULTS: usize = 1_000;
const MAX_GREP_MATCHES: usize = 500;

/// `glob` tool handler (`SideEffectClass::Read`).
#[derive(Debug, Default)]
pub struct GlobTool;

impl ToolHandler for GlobTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'pattern'".to_string()),
                    is_error: true,
                });
            };
            let root_rel = args.get("root").and_then(Value::as_str).unwrap_or(".");
            let root_abs = ctx.path_guard.resolve_read(root_rel)?;
            if !root_abs.exists() {
                return Ok(ToolOutcome {
                    output: Value::String(format!("search root not found: '{root_rel}'")),
                    is_error: true,
                });
            }

            let matcher = match glob_to_regex(pattern) {
                Ok(re) => re,
                Err(msg) => {
                    return Ok(ToolOutcome {
                        output: Value::String(format!("invalid glob pattern '{pattern}': {msg}")),
                        is_error: true,
                    });
                }
            };
            let ws_root = ctx.workspace_root.clone();
            let cancel = ctx.cancel.clone();

            let (matches, truncated) = tokio::task::spawn_blocking(move || {
                let mut results = Vec::new();
                let walker = WalkBuilder::new(&root_abs)
                    .follow_links(false)
                    .hidden(false)
                    .build();
                for entry in walker.flatten() {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    let rel = path
                        .strip_prefix(&ws_root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    if matcher.is_match(&rel) {
                        results.push(rel);
                    }
                }
                results.sort();
                let truncated = results.len() > MAX_GLOB_RESULTS;
                if truncated {
                    results.truncate(MAX_GLOB_RESULTS);
                }
                (results, truncated)
            })
            .await
            .map_err(|e| RuntimeError::ArtifactError(format!("glob task failed: {e}")))?;

            Ok(ToolOutcome {
                output: json!({
                    "pattern": pattern,
                    "matches": matches,
                    "truncated": truncated,
                }),
                is_error: false,
            })
        })
    }
}

/// `grep` tool handler (`SideEffectClass::Read`).
#[derive(Debug, Default)]
pub struct GrepTool;

impl ToolHandler for GrepTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'pattern'".to_string()),
                    is_error: true,
                });
            };
            let target_rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let literal = args
                .get("literal")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let glob_filter = args.get("glob_filter").and_then(Value::as_str);

            let resolved = ctx.path_guard.resolve_read(target_rel)?;
            if !resolved.exists() {
                return Ok(ToolOutcome {
                    output: Value::String(format!("search path not found: '{target_rel}'")),
                    is_error: true,
                });
            }

            let regex_src = if literal {
                regex_lite::escape(pattern)
            } else {
                pattern.to_string()
            };
            let regex = match Regex::new(&regex_src) {
                Ok(r) => r,
                Err(err) => {
                    return Ok(ToolOutcome {
                        output: Value::String(format!("invalid regex '{pattern}': {err}")),
                        is_error: true,
                    });
                }
            };
            let filter_re = match glob_filter {
                Some(g) => match glob_to_regex(g) {
                    Ok(r) => Some(r),
                    Err(msg) => {
                        return Ok(ToolOutcome {
                            output: Value::String(format!("invalid glob_filter '{g}': {msg}")),
                            is_error: true,
                        });
                    }
                },
                None => None,
            };

            let ws_root = ctx.workspace_root.clone();
            let cancel = ctx.cancel.clone();

            let (matches, truncated) = tokio::task::spawn_blocking(move || {
                let mut hits: Vec<(String, usize, String)> = Vec::new();
                if resolved.is_file() {
                    search_single_file(&resolved, &ws_root, &regex, &mut hits);
                } else {
                    let walker = WalkBuilder::new(&resolved)
                        .follow_links(false)
                        .hidden(false)
                        .build();
                    for entry in walker.flatten() {
                        if cancel.is_cancelled() {
                            break;
                        }
                        let p = entry.path();
                        if !p.is_file() {
                            continue;
                        }
                        if let Some(ref f) = filter_re {
                            let rel = p
                                .strip_prefix(&ws_root)
                                .unwrap_or(p)
                                .to_string_lossy()
                                .replace('\\', "/");
                            if !f.is_match(&rel) {
                                continue;
                            }
                        }
                        search_single_file(p, &ws_root, &regex, &mut hits);
                    }
                }
                hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                let truncated = hits.len() > MAX_GREP_MATCHES;
                if truncated {
                    hits.truncate(MAX_GREP_MATCHES);
                }
                let formatted: Vec<Value> = hits
                    .into_iter()
                    .map(|(path, line, text)| {
                        json!({
                            "path": path,
                            "line": line,
                            "text": text,
                        })
                    })
                    .collect();
                (formatted, truncated)
            })
            .await
            .map_err(|e| RuntimeError::ArtifactError(format!("grep task failed: {e}")))?;

            Ok(ToolOutcome {
                output: json!({
                    "pattern": pattern,
                    "matches": matches,
                    "truncated": truncated,
                }),
                is_error: false,
            })
        })
    }
}

fn search_single_file(
    path: &Path,
    ws_root: &Path,
    regex: &Regex,
    hits: &mut Vec<(String, usize, String)>,
) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    if bytes.contains(&0) {
        return;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return;
    };
    let rel = path
        .strip_prefix(ws_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    for (idx, line) in text.lines().enumerate() {
        if regex.is_match(line) {
            hits.push((rel.clone(), idx + 1, line.to_string()));
        }
    }
}

fn glob_to_regex(pattern: &str) -> Result<Regex, String> {
    let norm = pattern.trim_start_matches("./").replace('\\', "/");
    let mut re = String::from("^");
    let chars: Vec<char> = norm.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    if i + 2 < chars.len() && chars[i + 2] == '/' {
                        re.push_str("(?:.*/)?");
                        i += 3;
                    } else {
                        re.push_str(".*");
                        i += 2;
                    }
                } else {
                    re.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                re.push_str("[^/]");
                i += 1;
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(chars[i]);
                i += 1;
            }
            c => {
                re.push(c);
                i += 1;
            }
        }
    }
    re.push('$');
    Regex::new(&re).map_err(|e| e.to_string())
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
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            cwd: root.to_path_buf(),
            turn_id: "t1".into(),
            phase_id: "p1".into(),
            effect_id: "e1".into(),
            sandbox_policy: SandboxPolicy::default(),
            path_guard: WorkspacePathGuard::new(root, &BTreeSet::new()).expect("guard"),
            sandbox_driver: default_sandbox_driver(true),
            blob_store: BlobStore::new(root),
            cancel: CancellationToken::new(),
            event_sink: None,
            redactor: Default::default(),
        }
    }

    #[tokio::test]
    async fn glob_and_grep_respect_gitignore_and_sort_deterministically() {
        let dir = std::env::temp_dir().join(format!("gestalt_search_tools_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).expect("git");
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::create_dir_all(dir.join("target")).expect("target");
        fs::write(dir.join(".gitignore"), "target/\n").expect("gitignore");
        fs::write(dir.join("src/a.rs"), "fn alpha() {}\n").expect("a.rs");
        fs::write(dir.join("src/b.rs"), "fn beta() {}\nfn alpha() {}\n").expect("b.rs");
        fs::write(dir.join("target/ignored.rs"), "fn alpha() {}\n").expect("ignored");

        let canon = fs::canonicalize(&dir).expect("canon");
        let ctx = make_ctx(&canon);
        let noop = |_: &str, _: Value| {};

        let glob_out = GlobTool
            .execute(&json!({ "pattern": "**/*.rs" }), &ctx, &noop)
            .await
            .expect("glob");
        assert_eq!(glob_out.output["matches"], json!(["src/a.rs", "src/b.rs"]));

        let grep_out = GrepTool
            .execute(
                &json!({ "pattern": "fn alpha", "glob_filter": "src/*.rs" }),
                &ctx,
                &noop,
            )
            .await
            .expect("grep");
        let hits = grep_out.output["matches"].as_array().expect("hits");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["path"], "src/a.rs");
        assert_eq!(hits[0]["line"], 1);
        assert_eq!(hits[1]["path"], "src/b.rs");
        assert_eq!(hits[1]["line"], 2);

        let bad_re = GrepTool
            .execute(&json!({ "pattern": "[unclosed" }), &ctx, &noop)
            .await
            .expect("bad regex");
        assert!(bad_re.is_error);

        let _ = fs::remove_dir_all(&dir);
    }
}
