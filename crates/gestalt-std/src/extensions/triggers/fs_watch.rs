#![forbid(unsafe_code)]
//! Workspace-confined debounced filesystem watcher (`/watch`) trigger handler.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use gestalt_runtime::{
    CancellationToken, RuntimeError, TriggerContext, TriggerEvent, TriggerHandler,
    TriggerInstanceConfig, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

fn extract_watch_paths(config: &Value) -> Result<Vec<String>, RuntimeError> {
    if let Some(arr) = config.get("paths").and_then(Value::as_array) {
        let paths: Vec<String> = arr
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
        if !paths.is_empty() {
            return Ok(paths);
        }
    }
    if let Some(single) = config.get("path").and_then(Value::as_str)
        && !single.trim().is_empty()
    {
        return Ok(vec![single.trim().to_string()]);
    }
    Err(RuntimeError::ArtifactError(
        "fs_watch trigger requires 'paths' or 'path'".to_string(),
    ))
}

fn matches_simple_glob(pattern: &str, path: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return path.starts_with(prefix);
    }
    path == pattern || path.ends_with(&format!("/{pattern}"))
}

fn collect_snapshot(
    root: &Path,
    workspace_root: &Path,
    glob_filter: Option<&str>,
    out: &mut BTreeMap<String, (u64, u128)>,
) {
    let Ok(meta) = fs::metadata(root) else {
        return;
    };
    if meta.is_file() {
        let rel = root
            .strip_prefix(workspace_root)
            .unwrap_or(root)
            .to_string_lossy()
            .replace('\\', "/");
        if glob_filter.is_none_or(|g| matches_simple_glob(g, &rel)) {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            out.insert(rel, (meta.len(), mtime));
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == ".git" || name == ".gestalt" || name == "target" {
            continue;
        }
        collect_snapshot(&p, workspace_root, glob_filter, out);
    }
}

/// Workspace-confined debounced filesystem watcher (`/watch`) [`TriggerHandler`].
#[derive(Debug, Clone, Default)]
pub struct FsWatchTriggerHandler;

impl TriggerHandler for FsWatchTriggerHandler {
    fn kind(&self) -> &str {
        "fs_watch"
    }

    fn description(&self) -> &str {
        "Workspace-confined debounced filesystem watcher (/watch)"
    }

    fn validate(&self, config: &Value) -> Result<(), RuntimeError> {
        let _ = extract_watch_paths(config)?;
        Ok(())
    }

    fn start(
        &self,
        instance: TriggerInstanceConfig,
        ctx: TriggerContext,
        event_tx: mpsc::UnboundedSender<TriggerEvent>,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<()>, RuntimeError> {
        let raw_paths = extract_watch_paths(&instance.config)?;
        let mut resolved_roots = Vec::with_capacity(raw_paths.len());
        for raw in &raw_paths {
            resolved_roots.push(ctx.path_guard.resolve_read(raw)?);
        }

        let glob_filter = instance
            .config
            .get("glob_filter")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let poll_ms = instance
            .config
            .get("poll_interval_ms")
            .and_then(Value::as_u64)
            .unwrap_or(150)
            .max(20);
        let debounce_ms = instance
            .config
            .get("debounce_ms")
            .and_then(Value::as_u64)
            .unwrap_or(200);

        Ok(tokio::spawn(async move {
            let mut baseline = BTreeMap::new();
            for root in &resolved_roots {
                collect_snapshot(
                    root,
                    &ctx.workspace_root,
                    glob_filter.as_deref(),
                    &mut baseline,
                );
            }

            let mut pending_changes: Vec<String> = Vec::new();
            let mut last_change_ms: Option<u64> = None;

            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_millis(poll_ms)) => {
                        let mut current = BTreeMap::new();
                        for root in &resolved_roots {
                            collect_snapshot(
                                root,
                                &ctx.workspace_root,
                                glob_filter.as_deref(),
                                &mut current,
                            );
                        }

                        let mut changed_now = Vec::new();
                        for (path, sig) in &current {
                            if baseline.get(path) != Some(sig) {
                                changed_now.push(path.clone());
                            }
                        }
                        for path in baseline.keys() {
                            if !current.contains_key(path) {
                                changed_now.push(path.clone());
                            }
                        }
                        baseline = current;

                        if !changed_now.is_empty() {
                            for p in changed_now {
                                if !pending_changes.contains(&p) {
                                    pending_changes.push(p);
                                }
                            }
                            last_change_ms = Some(current_epoch_ms());
                        }

                        if let Some(changed_at) = last_change_ms
                            && current_epoch_ms().saturating_sub(changed_at) >= debounce_ms
                            && !pending_changes.is_empty()
                        {
                            let files = std::mem::take(&mut pending_changes);
                            last_change_ms = None;
                            let files_joined = files.join(", ");
                            let prompt = match &instance.prompt {
                                Value::String(tmpl) => Value::String(tmpl.replace("{files}", &files_joined)),
                                other => other.clone(),
                            };
                            let ev = TriggerEvent {
                                trigger_id: instance.id.clone(),
                                kind: instance.kind.clone(),
                                prompt,
                                preset: instance.preset.clone(),
                                extra_decorators: instance.extra_decorators.clone(),
                                metadata: json!({ "changed_paths": files }),
                                timestamp_ms: current_epoch_ms(),
                            };
                            if event_tx.send(ev).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_runtime::sandbox::WorkspacePathGuard;
    use gestalt_runtime::{CredentialStore, SharedHttpClient, StreamRedactor};
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn fs_watch_detects_workspace_edits_and_blocks_escapes() {
        let dir = std::env::temp_dir().join(format!("gestalt_fswatch_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let canon = fs::canonicalize(&dir).expect("canon");
        fs::write(canon.join("app.rs"), "fn main() {}\n").expect("write");

        let ctx = TriggerContext {
            workspace_root: canon.clone(),
            path_guard: WorkspacePathGuard::new(&canon, &BTreeSet::new()).expect("guard"),
            credentials: CredentialStore::default(),
            http: SharedHttpClient::new(5).expect("http"),
            redactor: StreamRedactor::default(),
            activity: gestalt_runtime::ActivityTracker::default(),
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        // Escape attempt must fail closed
        let escape_err = FsWatchTriggerHandler.start(
            TriggerInstanceConfig {
                id: "bad".into(),
                kind: "fs_watch".into(),
                enabled: true,
                prompt: json!("bad"),
                preset: None,
                extra_decorators: Vec::new(),
                config: json!({ "paths": ["../../etc/passwd"] }),
            },
            ctx.clone(),
            tx.clone(),
            CancellationToken::new(),
        );
        assert!(matches!(escape_err, Err(RuntimeError::SandboxViolation(_))));

        let cancel = CancellationToken::new();
        let handle = FsWatchTriggerHandler
            .start(
                TriggerInstanceConfig {
                    id: "watch_rs".into(),
                    kind: "fs_watch".into(),
                    enabled: true,
                    prompt: json!("Changed: {files}"),
                    preset: None,
                    extra_decorators: Vec::new(),
                    config: json!({
                        "paths": ["."],
                        "glob_filter": "*.rs",
                        "poll_interval_ms": 25,
                        "debounce_ms": 30,
                    }),
                },
                ctx,
                tx,
                cancel.clone(),
            )
            .expect("start watch");

        tokio::time::sleep(Duration::from_millis(40)).await;
        fs::write(canon.join("app.rs"), "fn main() { println!(\"hi\"); }\n").expect("modify");

        let ev = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("watch event");
        assert_eq!(ev.prompt, json!("Changed: app.rs"));
        assert_eq!(ev.metadata["changed_paths"], json!(["app.rs"]));

        cancel.cancel();
        let _ = handle.await;
        let _ = fs::remove_dir_all(&dir);
    }
}
