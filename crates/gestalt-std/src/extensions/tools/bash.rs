#![forbid(unsafe_code)]
//! Sandboxed shell execution (`bash`) and Turn/Session-scoped background task
//! management (`bash_output`, `bash_kill`).

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use gestalt_runtime::sandbox::build_clean_env;
use gestalt_runtime::{
    BlobStore, CancellationToken, DEFAULT_INLINE_SPILL_BYTES, RuntimeError, SandboxCommand,
    StreamDeltaFn, ToolExecutionContext, ToolHandler, ToolOutcome,
};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const DEFAULT_BASH_TIMEOUT_MS: u64 = 30_000;
const MAX_TASK_BUFFER_BYTES: usize = 256 * 1024;

/// Lifecycle scope of a background `bash` task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskScope {
    /// Automatically reaped when the originating Turn ends (default).
    Turn,
    /// Survives across Turns and is reaped when the Session shuts down.
    Session,
}

impl TaskScope {
    fn from_arg(raw: Option<&str>) -> Self {
        match raw {
            Some("session") => Self::Session,
            _ => Self::Turn,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Session => "session",
        }
    }
}

struct TaskState {
    output_buf: Vec<u8>,
    blob_digest: Option<String>,
    finished: bool,
    exit_code: Option<i32>,
    killed: bool,
    pid: Option<u32>,
}

struct ManagedTask {
    turn_id: String,
    scope: TaskScope,
    cwd_rel: String,
    cancel: CancellationToken,
    state: Arc<StdMutex<TaskState>>,
}

/// Thread-safe registry managing background `bash` process groups and
/// automatic Turn/Session reaping.
#[derive(Default)]
pub struct BackgroundTaskManager {
    seq: AtomicU64,
    tasks: StdMutex<BTreeMap<String, ManagedTask>>,
}

impl std::fmt::Debug for BackgroundTaskManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.tasks.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("BackgroundTaskManager")
            .field("active_tasks", &count)
            .finish()
    }
}

impl Drop for BackgroundTaskManager {
    fn drop(&mut self) {
        let _ = self.reap_all();
    }
}

struct BackgroundSpawnSpec<'a> {
    program: &'a str,
    args: &'a [String],
    cwd: &'a Path,
    cwd_rel: String,
    turn_id: String,
    scope: TaskScope,
    blob_store: BlobStore,
}

impl BackgroundTaskManager {
    /// Construct an empty [`BackgroundTaskManager`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reap all [`TaskScope::Turn`] tasks belonging to `turn_id`.
    pub fn reap_turn(&self, turn_id: &str) -> usize {
        self.reap_where(|t| t.scope == TaskScope::Turn && t.turn_id == turn_id)
    }

    /// Reap all [`TaskScope::Turn`] tasks not belonging to `current_turn_id`.
    pub fn reap_stale_turns(&self, current_turn_id: &str) -> usize {
        self.reap_where(|t| t.scope == TaskScope::Turn && t.turn_id != current_turn_id)
    }

    fn reap_where(&self, pred: impl Fn(&ManagedTask) -> bool) -> usize {
        let mut to_kill = Vec::new();
        if let Ok(mut guard) = self.tasks.lock() {
            let keys: Vec<String> = guard
                .iter()
                .filter(|(_, t)| pred(t))
                .map(|(k, _)| k.clone())
                .collect();
            for k in keys {
                if let Some(task) = guard.remove(&k) {
                    to_kill.push(task);
                }
            }
        }
        let count = to_kill.len();
        for task in to_kill {
            terminate_managed_task(&task);
        }
        count
    }

    /// Reap all remaining background tasks across all scopes.
    pub fn reap_all(&self) -> usize {
        let drained: Vec<ManagedTask> = if let Ok(mut guard) = self.tasks.lock() {
            std::mem::take(&mut *guard).into_values().collect()
        } else {
            Vec::new()
        };
        let count = drained.len();
        for task in drained {
            terminate_managed_task(&task);
        }
        count
    }

    fn spawn_background(&self, spec: BackgroundSpawnSpec<'_>) -> Result<String, RuntimeError> {
        self.reap_stale_turns(&spec.turn_id);
        let id_num = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let task_id = format!("bg_{id_num}");

        let mut command = Command::new(spec.program);
        command
            .args(spec.args)
            .current_dir(spec.cwd)
            .env_clear()
            .envs(build_clean_env(&BTreeMap::new()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let mut child = command.spawn()?;
        let pid = child.id();
        let cancel = CancellationToken::new();
        let state = Arc::new(StdMutex::new(TaskState {
            output_buf: Vec::new(),
            blob_digest: None,
            finished: false,
            exit_code: None,
            killed: false,
            pid,
        }));

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let state_out = state.clone();
        let state_err = state.clone();
        let state_wait = state.clone();
        let cancel_wait = cancel.clone();
        let blob_store = spec.blob_store;

        tokio::spawn(async move {
            if let Some(mut pipe) = stdout_pipe {
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    append_capped_output(&state_out, &buf[..n]);
                }
            }
        });
        tokio::spawn(async move {
            if let Some(mut pipe) = stderr_pipe {
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    append_capped_output(&state_err, &buf[..n]);
                }
            }
        });
        tokio::spawn(async move {
            let code = tokio::select! {
                status = child.wait() => {
                    status.ok().and_then(|s| s.code()).unwrap_or(-1)
                }
                () = cancel_wait.cancelled() => {
                    kill_process_group_async(pid, &mut child).await;
                    -1
                }
            };
            if let Ok(mut guard) = state_wait.lock() {
                if guard.output_buf.len() > DEFAULT_INLINE_SPILL_BYTES
                    && let Ok(digest) = blob_store.put_bytes(&guard.output_buf)
                {
                    guard.blob_digest = Some(digest);
                }
                guard.finished = true;
                guard.exit_code = Some(code);
            }
        });

        if let Ok(mut guard) = self.tasks.lock() {
            guard.insert(
                task_id.clone(),
                ManagedTask {
                    turn_id: spec.turn_id,
                    scope: spec.scope,
                    cwd_rel: spec.cwd_rel,
                    cancel,
                    state,
                },
            );
        }
        Ok(task_id)
    }

    /// Read buffered output and status for `task_id`.
    fn inspect(&self, task_id: &str, blob_store: &BlobStore) -> Result<Value, String> {
        let guard = self
            .tasks
            .lock()
            .map_err(|_| "task manager lock poisoned".to_string())?;
        let Some(task) = guard.get(task_id) else {
            return Err(format!("unknown background task '{task_id}'"));
        };
        let st = task
            .state
            .lock()
            .map_err(|_| "task state lock poisoned".to_string())?;
        let text = String::from_utf8_lossy(&st.output_buf).to_string();
        let (output, mut blob_digest) = match blob_store.spill_if_oversized(&text) {
            Ok((preview, d)) => (preview, d),
            Err(_) => (text, None),
        };
        if blob_digest.is_none() {
            blob_digest = st.blob_digest.clone();
        }
        Ok(json!({
            "task_id": task_id,
            "running": !st.finished,
            "exit_code": st.exit_code,
            "killed": st.killed,
            "scope": task.scope.as_str(),
            "cwd": task.cwd_rel,
            "output": output,
            "blob_digest": blob_digest,
        }))
    }

    /// Terminate a background task's process group.
    fn kill(&self, task_id: &str) -> Result<Value, String> {
        let guard = self
            .tasks
            .lock()
            .map_err(|_| "task manager lock poisoned".to_string())?;
        let Some(task) = guard.get(task_id) else {
            return Err(format!("unknown background task '{task_id}'"));
        };
        terminate_managed_task(task);
        Ok(json!({
            "task_id": task_id,
            "killed": true,
        }))
    }
}

fn append_capped_output(state: &Arc<StdMutex<TaskState>>, chunk: &[u8]) {
    if let Ok(mut guard) = state.lock() {
        let remaining = MAX_TASK_BUFFER_BYTES.saturating_sub(guard.output_buf.len());
        let take = remaining.min(chunk.len());
        if take > 0 {
            guard.output_buf.extend_from_slice(&chunk[..take]);
        }
    }
}

fn terminate_managed_task(task: &ManagedTask) {
    task.cancel.cancel();
    if let Ok(mut st) = task.state.lock() {
        st.killed = true;
        if let Some(pid) = st.pid {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &format!("-{pid}")])
                    .output();
            }
            #[cfg(not(unix))]
            {
                let _ = pid;
            }
        }
    }
}

async fn kill_process_group_async(pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(p) = pid {
        let _ = Command::new("kill")
            .args(["-9", &format!("-{p}")])
            .output()
            .await;
    }
    #[cfg(not(unix))]
    let _ = pid;
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_millis(100), child.wait()).await;
}

/// Sandboxed `bash` tool handler (`SideEffectClass::Irreversible`).
#[derive(Debug, Clone)]
pub struct BashTool {
    tasks: Arc<BackgroundTaskManager>,
}

impl BashTool {
    /// Construct a [`BashTool`] sharing `tasks`.
    #[must_use]
    pub fn new(tasks: Arc<BackgroundTaskManager>) -> Self {
        Self { tasks }
    }
}

impl ToolHandler for BashTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(command_str) = args.get("command").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'command'".to_string()),
                    is_error: true,
                });
            };
            let cwd_arg = args.get("cwd").and_then(Value::as_str).unwrap_or(".");
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_BASH_TIMEOUT_MS);
            let background = args
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let scope = TaskScope::from_arg(args.get("scope").and_then(Value::as_str));

            let resolved_cwd = ctx.path_guard.resolve_read(cwd_arg)?;
            if !resolved_cwd.is_dir() {
                return Ok(ToolOutcome {
                    output: Value::String(format!("cwd is not a directory: '{cwd_arg}'")),
                    is_error: true,
                });
            }
            let rel_cwd = resolved_cwd
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(&resolved_cwd)
                .to_string_lossy()
                .replace('\\', "/");
            let rel_cwd = if rel_cwd.is_empty() {
                ".".to_string()
            } else {
                rel_cwd
            };

            let sb_cmd = SandboxCommand {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), command_str.to_string()],
                cwd: resolved_cwd.clone(),
                env: BTreeMap::new(),
                stdin_bytes: None,
                timeout_ms: Some(timeout_ms),
            };
            let (program, os_args) = ctx.sandbox_driver.build_os_command(
                &sb_cmd,
                &ctx.workspace_root,
                &ctx.sandbox_policy,
            )?;

            if background {
                let task_id = self.tasks.spawn_background(BackgroundSpawnSpec {
                    program: &program,
                    args: &os_args,
                    cwd: &resolved_cwd,
                    cwd_rel: rel_cwd.clone(),
                    turn_id: ctx.turn_id.clone(),
                    scope,
                    blob_store: ctx.blob_store.clone(),
                })?;
                return Ok(ToolOutcome {
                    output: json!({
                        "task_id": task_id,
                        "background": true,
                        "scope": scope.as_str(),
                        "cwd": rel_cwd,
                    }),
                    is_error: false,
                });
            }

            let mut cmd = Command::new(&program);
            cmd.args(&os_args)
                .current_dir(&resolved_cwd)
                .env_clear()
                .envs(build_clean_env(&BTreeMap::new()))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }

            let mut child = cmd.spawn()?;
            let pid = child.id();
            let mut stdout_pipe = child.stdout.take();
            let mut stderr_pipe = child.stderr.take();

            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            let mut out_buf = [0u8; 4096];
            let mut err_buf = [0u8; 4096];
            let mut out_eof = stdout_pipe.is_none();
            let mut err_eof = stderr_pipe.is_none();
            let mut timed_out = false;
            let mut aborted = false;

            let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
            tokio::pin!(deadline);

            let exit_code = loop {
                tokio::select! {
                    biased;
                    () = ctx.cancel.cancelled() => {
                        aborted = true;
                        kill_process_group_async(pid, &mut child).await;
                        break -1;
                    }
                    () = &mut deadline => {
                        timed_out = true;
                        kill_process_group_async(pid, &mut child).await;
                        break -1;
                    }
                    res = async {
                        match stdout_pipe.as_mut() {
                            Some(p) => p.read(&mut out_buf).await,
                            None => std::future::pending().await,
                        }
                    }, if !out_eof => {
                        match res {
                            Ok(0) | Err(_) => out_eof = true,
                            Ok(n) => {
                                stdout_bytes.extend_from_slice(&out_buf[..n]);
                                let chunk = String::from_utf8_lossy(&out_buf[..n]).to_string();
                                on_delta("tool_output", Value::String(chunk));
                            }
                        }
                    }
                    res = async {
                        match stderr_pipe.as_mut() {
                            Some(p) => p.read(&mut err_buf).await,
                            None => std::future::pending().await,
                        }
                    }, if !err_eof => {
                        match res {
                            Ok(0) | Err(_) => err_eof = true,
                            Ok(n) => {
                                stderr_bytes.extend_from_slice(&err_buf[..n]);
                                let chunk = String::from_utf8_lossy(&err_buf[..n]).to_string();
                                on_delta("tool_output", Value::String(chunk));
                            }
                        }
                    }
                    status = child.wait(), if out_eof && err_eof => {
                        break status?.code().unwrap_or(-1);
                    }
                }
            };

            let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
            let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
            let is_error = exit_code != 0 || timed_out || aborted;

            Ok(ToolOutcome {
                output: json!({
                    "exit_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "cwd": rel_cwd,
                    "timed_out": timed_out,
                    "aborted": aborted,
                }),
                is_error,
            })
        })
    }
}

/// `bash_output` tool handler (`SideEffectClass::Read`).
#[derive(Debug, Clone)]
pub struct BashOutputTool {
    tasks: Arc<BackgroundTaskManager>,
}

impl BashOutputTool {
    /// Construct a [`BashOutputTool`] sharing `tasks`.
    #[must_use]
    pub fn new(tasks: Arc<BackgroundTaskManager>) -> Self {
        Self { tasks }
    }
}

impl ToolHandler for BashOutputTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(task_id) = args.get("task_id").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'task_id'".to_string()),
                    is_error: true,
                });
            };
            match self.tasks.inspect(task_id, &ctx.blob_store) {
                Ok(out) => Ok(ToolOutcome {
                    output: out,
                    is_error: false,
                }),
                Err(msg) => Ok(ToolOutcome {
                    output: Value::String(msg),
                    is_error: true,
                }),
            }
        })
    }
}

/// `bash_kill` tool handler (`SideEffectClass::Irreversible`).
#[derive(Debug, Clone)]
pub struct BashKillTool {
    tasks: Arc<BackgroundTaskManager>,
}

impl BashKillTool {
    /// Construct a [`BashKillTool`] sharing `tasks`.
    #[must_use]
    pub fn new(tasks: Arc<BackgroundTaskManager>) -> Self {
        Self { tasks }
    }
}

impl ToolHandler for BashKillTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        _ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(task_id) = args.get("task_id").and_then(Value::as_str) else {
                return Ok(ToolOutcome {
                    output: Value::String("validation error: missing 'task_id'".to_string()),
                    is_error: true,
                });
            };
            match self.tasks.kill(task_id) {
                Ok(out) => Ok(ToolOutcome {
                    output: out,
                    is_error: false,
                }),
                Err(msg) => Ok(ToolOutcome {
                    output: Value::String(msg),
                    is_error: true,
                }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::SandboxPolicy;
    use gestalt_runtime::{WorkspacePathGuard, default_sandbox_driver};
    use std::collections::BTreeSet;
    use std::fs;

    fn make_ctx(root: &Path, turn_id: &str, protected: BTreeSet<String>) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            cwd: root.to_path_buf(),
            turn_id: turn_id.to_string(),
            phase_id: "p1".to_string(),
            effect_id: "e1".to_string(),
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
    async fn bash_foreground_cancel_and_background_reaping() {
        let dir = std::env::temp_dir().join(format!("gestalt_bash_tools_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).expect("mkdir");
        fs::create_dir_all(dir.join(".gestalt/memories")).expect("mem");
        fs::write(dir.join(".gestalt/memories/MEMORY.md"), "keep").expect("write");
        let canon = fs::canonicalize(&dir).expect("canon");

        let mgr = Arc::new(BackgroundTaskManager::new());
        let bash = BashTool::new(mgr.clone());
        let bash_out = BashOutputTool::new(mgr.clone());
        let bash_kill = BashKillTool::new(mgr.clone());

        let protected: BTreeSet<String> = [".gestalt/memories/".to_string()].into_iter().collect();
        let ctx = make_ctx(&canon, "t1", protected.clone());
        let noop = |_: &str, _: Value| {};

        // 1. Foreground execution in subdirectory records relative cwd
        let fg = bash
            .execute(
                &json!({ "command": "pwd && echo hi", "cwd": "sub" }),
                &ctx,
                &noop,
            )
            .await
            .expect("fg");
        assert!(!fg.is_error);
        assert_eq!(fg.output["cwd"], "sub");
        assert!(fg.output["stdout"].as_str().expect("s").contains("hi"));

        // 2. Cancellation aborts <= 250 ms
        let cancel_ctx = make_ctx(&canon, "t1", protected.clone());
        let cancel_handle = cancel_ctx.cancel.clone();
        let started = std::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            cancel_handle.cancel();
        });
        let aborted = bash
            .execute(&json!({ "command": "sleep 10" }), &cancel_ctx, &noop)
            .await
            .expect("cancel");
        assert_eq!(aborted.output["aborted"], true);
        assert!(started.elapsed() <= Duration::from_millis(250));

        // 3. Background spawn + output + kill + Turn reaping
        let bg = bash
            .execute(
                &json!({ "command": "echo bg_ready && sleep 10", "background": true }),
                &ctx,
                &noop,
            )
            .await
            .expect("bg");
        let task_id = bg.output["task_id"].as_str().expect("task_id").to_string();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let out = bash_out
            .execute(&json!({ "task_id": task_id }), &ctx, &noop)
            .await
            .expect("bg_out");
        assert!(
            out.output["output"]
                .as_str()
                .expect("s")
                .contains("bg_ready")
        );
        let killed = bash_kill
            .execute(&json!({ "task_id": task_id }), &ctx, &noop)
            .await
            .expect("bg_kill");
        assert_eq!(killed.output["killed"], true);

        // Turn reaping removes t1 tasks
        assert_eq!(mgr.reap_turn("t1"), 1);

        // Protected path write inside OS sandbox fails
        let mut prot_ctx = make_ctx(&canon, "t2", protected);
        prot_ctx.sandbox_driver = default_sandbox_driver(false);
        if prot_ctx.sandbox_driver.is_available() {
            let hack = bash
                .execute(
                    &json!({ "command": "echo pwned > .gestalt/memories/MEMORY.md" }),
                    &prot_ctx,
                    &noop,
                )
                .await
                .expect("sandboxed bash");
            assert!(hack.is_error);
            assert_eq!(
                fs::read_to_string(canon.join(".gestalt/memories/MEMORY.md")).expect("read"),
                "keep"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
