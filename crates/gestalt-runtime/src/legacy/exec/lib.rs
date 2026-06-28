#![allow(unsafe_code)]
//! `gestalt-exec` — Subprocess execution / sandbox
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

// Workspace lint configuration is inherited via Cargo.toml [lints] workspace = true

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use gestalt_core::{artifact_path, is_audited_local_command, HarnessError, ToolError};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{timeout, timeout_at, Instant as TokioInstant},
};

#[async_trait]
pub trait ExecutionSandbox: Send + Sync {
    async fn run(&self, request: ExecRequest) -> Result<ExecResult, HarnessError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub workspace_root: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub network_policy: NetworkPolicy,
    pub mounts: Vec<SandboxMount>,
    pub artifact_dir: Option<PathBuf>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub truncated: bool,
    pub duration_ms: u64,
}

impl ExecResult {
    #[must_use]
    pub fn combined_text(&self) -> String {
        let stdout = String::from_utf8_lossy(&self.stdout);
        let stderr = String::from_utf8_lossy(&self.stderr);
        format!(
            "exit_code: {}\ntimed_out: {}\ntruncated: {}\nstdout:\n{}\nstderr:\n{}",
            self.exit_code
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            self.timed_out,
            self.truncated,
            stdout,
            stderr
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    None,
    Loopback,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMount {
    pub host_path: PathBuf,
    pub container_path: PathBuf,
    pub read_only: bool,
}

/// Host subprocess execution runner.
///
/// # WARNING
/// This is NOT a security sandbox. It executes processes directly on the host machine
/// under the current user's privileges. It does NOT provide chroot, mount namespace,
/// network confinement, or seccomp isolation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSandbox;

struct ProcessGroupKiller {
    child_id: Option<u32>,
}

impl Drop for ProcessGroupKiller {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child_id {
            unsafe {
                if let Ok(pid) = i32::try_from(pid) {
                    // SAFETY: `pid` comes from a spawned child process. Negating it targets the
                    // child's process group, which we created with `setsid()` in `pre_exec`.
                    // Best-effort cleanup is acceptable in `Drop` because failures cannot be
                    // reported and we only invoke the async-signal-safe `kill(2)` syscall.
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
}

#[async_trait]
impl ExecutionSandbox for NoSandbox {
    async fn run(&self, request: ExecRequest) -> Result<ExecResult, HarnessError> {
        validate_working_dir(&request.working_dir, request.workspace_root.as_deref())?;

        let (program, args) = request
            .command
            .split_first()
            .ok_or_else(|| invalid_input("command must not be empty"))?;

        if request.network_policy == NetworkPolicy::None {
            let cmd_str = request.command.join(" ");
            if !is_audited_local_command(&cmd_str) {
                return Err(HarnessError::Tool(ToolError::NetworkDenied(format!(
                    "Command '{cmd_str}' violates network policy (no network access allowed)"
                ))));
            }
        }

        let started = Instant::now();
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&request.working_dir)
            .env_clear()
            .envs(request.env.iter())
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                // SAFETY: `pre_exec` runs in the child after fork and before exec. We only call
                // the async-signal-safe `setsid(2)` syscall to place the child in its own process
                // group so timeout/drop cleanup can kill the whole subtree.
                libc::setsid();
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(ToolError::ExecutionFailed)?;

        let (mut stdout_file, mut stderr_file, mut stdout_path, mut stderr_path, mut combined_path) =
            (None, None, None, None, None);

        if let (Some(ref art_dir), Some(ref tc_id)) = (&request.artifact_dir, &request.tool_call_id)
        {
            tokio::fs::create_dir_all(art_dir)
                .await
                .map_err(ToolError::ExecutionFailed)?;
            let out_path = artifact_path(art_dir, tc_id, "_stdout.txt");
            let err_path = artifact_path(art_dir, tc_id, "_stderr.txt");
            let comb_path = artifact_path(art_dir, tc_id, ".txt");

            stdout_file = Some(
                tokio::fs::File::create(&out_path)
                    .await
                    .map_err(ToolError::ExecutionFailed)?,
            );
            stderr_file = Some(
                tokio::fs::File::create(&err_path)
                    .await
                    .map_err(ToolError::ExecutionFailed)?,
            );
            stdout_path = Some(out_path);
            stderr_path = Some(err_path);
            combined_path = Some(comb_path);
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| invalid_input("failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| invalid_input("failed to capture stderr"))?;

        let deadline = TokioInstant::now() + request.timeout;
        let stdout_task = tokio::spawn(read_capped(
            stdout,
            request.max_output_bytes,
            deadline,
            stdout_file,
        ));
        let stderr_task = tokio::spawn(read_capped(
            stderr,
            request.max_output_bytes,
            deadline,
            stderr_file,
        ));

        let mut killer = ProcessGroupKiller {
            child_id: child.id(),
        };
        let wait_result = timeout(request.timeout, child.wait()).await;
        let (exit_code, timed_out) = if let Ok(status) = wait_result {
            killer.child_id = None;
            (
                Some(status.map_err(ToolError::ExecutionFailed)?.code()),
                false,
            )
        } else {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    if let Ok(pid) = i32::try_from(pid) {
                        // SAFETY: `pid` belongs to the spawned child process group established by
                        // `setsid()` above. On timeout we perform best-effort group termination to
                        // avoid leaving descendant processes behind.
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            killer.child_id = None;
            (None, true)
        };

        let stdout_res = stdout_task
            .await
            .map_err(|err| invalid_input(format!("stdout task failed: {err}")))??;
        let stderr_res = stderr_task
            .await
            .map_err(|err| invalid_input(format!("stderr task failed: {err}")))??;
        let mut truncated = stdout_res.truncated || stderr_res.truncated;
        let mut stdout = stdout_res.bytes;
        let mut stderr = stderr_res.bytes;

        if stdout.len().saturating_add(stderr.len()) > request.max_output_bytes {
            truncate_combined(&mut stdout, &mut stderr, request.max_output_bytes);
            truncated = true;
        }

        if let Some(combined_path) = combined_path {
            if truncated {
                let stdout_bytes = if let Some(path) = stdout_path.as_ref() {
                    tokio::fs::read(path)
                        .await
                        .map_err(ToolError::ExecutionFailed)?
                } else {
                    Vec::new()
                };
                let stderr_bytes = if let Some(path) = stderr_path.as_ref() {
                    tokio::fs::read(path)
                        .await
                        .map_err(ToolError::ExecutionFailed)?
                } else {
                    Vec::new()
                };
                let full_stdout_str = String::from_utf8_lossy(&stdout_bytes);
                let full_stderr_str = String::from_utf8_lossy(&stderr_bytes);
                let combined_text = format!(
                    "exit_code: {}\ntimed_out: {}\ntruncated: {}\nstdout:\n{}\nstderr:\n{}",
                    exit_code
                        .flatten()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string()),
                    timed_out,
                    truncated,
                    full_stdout_str,
                    full_stderr_str
                );
                tokio::fs::write(&combined_path, combined_text.as_bytes())
                    .await
                    .map_err(ToolError::ExecutionFailed)?;
            } else {
                if let Some(path) = stdout_path {
                    let _ = tokio::fs::remove_file(path).await;
                }
                if let Some(path) = stderr_path {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }

        Ok(ExecResult {
            exit_code: exit_code.flatten(),
            stdout,
            stderr,
            timed_out,
            truncated,
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        })
    }
}

#[derive(Debug)]
struct CappedRead {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_capped<R>(
    mut reader: R,
    max_output_bytes: usize,
    deadline: TokioInstant,
    mut artifact_file: Option<tokio::fs::File>,
) -> Result<CappedRead, HarnessError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;

    loop {
        let read = match timeout_at(deadline, reader.read(&mut buffer)).await {
            Ok(read) => read.map_err(ToolError::ExecutionFailed)?,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }

        if let Some(ref mut file) = artifact_file {
            use tokio::io::AsyncWriteExt;
            file.write_all(&buffer[..read])
                .await
                .map_err(ToolError::ExecutionFailed)?;
        }

        if !truncated {
            let remaining = max_output_bytes.saturating_sub(bytes.len());
            if read > remaining {
                bytes.extend_from_slice(&buffer[..remaining]);
                truncated = true;
            } else {
                bytes.extend_from_slice(&buffer[..read]);
            }
        }
    }

    Ok(CappedRead { bytes, truncated })
}

fn truncate_combined(stdout: &mut Vec<u8>, stderr: &mut Vec<u8>, max_output_bytes: usize) {
    if stdout.len() >= max_output_bytes {
        stdout.truncate(max_output_bytes);
        stderr.clear();
        return;
    }

    stderr.truncate(max_output_bytes.saturating_sub(stdout.len()));
}

fn validate_working_dir(
    working_dir: &Path,
    workspace_root: Option<&Path>,
) -> Result<(), ToolError> {
    let canonical_working_dir = working_dir
        .canonicalize()
        .map_err(ToolError::ExecutionFailed)?;
    if let Some(root) = workspace_root {
        let canonical_root = root.canonicalize().map_err(ToolError::ExecutionFailed)?;
        if !canonical_working_dir.starts_with(canonical_root) {
            return Err(ToolError::PathNotAllowed(
                canonical_working_dir.display().to_string(),
            ));
        }
    }

    Ok(())
}

fn invalid_input(reason: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool_name: "exec".to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gestalt-exec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp workspace");
        root
    }

    fn request(root: &Path, command: &str) -> ExecRequest {
        ExecRequest {
            command: vec!["bash".to_string(), "-lc".to_string(), command.to_string()],
            working_dir: root.to_path_buf(),
            workspace_root: Some(root.to_path_buf()),
            env: HashMap::new(),
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
            network_policy: NetworkPolicy::None,
            mounts: Vec::new(),
            artifact_dir: None,
            tool_call_id: None,
        }
    }

    #[tokio::test]
    async fn nosandbox_should_capture_successful_command() {
        let root = temp_workspace("success");
        let result = NoSandbox
            .run(request(&root, "printf hello"))
            .await
            .expect("command succeeds");

        assert_eq!(result.stdout, b"hello");
    }

    #[tokio::test]
    async fn nosandbox_should_normalize_failed_command() {
        let root = temp_workspace("failure");
        let mut request = request(&root, "printf nope >&2; exit 7");
        request.network_policy = NetworkPolicy::Full;
        let result = NoSandbox
            .run(request)
            .await
            .expect("command result returned");

        assert_eq!(result.exit_code, Some(7));
    }

    #[tokio::test]
    async fn nosandbox_should_kill_on_timeout() {
        let root = temp_workspace("timeout");
        let mut request = request(&root, "sleep 5");
        request.timeout = Duration::from_millis(50);

        let result = NoSandbox.run(request).await.expect("timeout is normalized");

        assert!(result.timed_out);
    }

    #[tokio::test]
    async fn nosandbox_should_cap_output() {
        let root = temp_workspace("cap");
        let mut request = request(&root, "printf 123456789");
        request.max_output_bytes = 4;

        let result = NoSandbox.run(request).await.expect("command succeeds");

        assert_eq!(result.stdout, b"1234");
    }

    #[tokio::test]
    async fn nosandbox_should_not_persist_artifacts_when_not_truncated() {
        let root = temp_workspace("artifacts-clean");
        let artifact_dir = root.join("artifacts");
        let mut request = request(&root, "printf hello");
        request.artifact_dir = Some(artifact_dir.clone());
        request.tool_call_id = Some("call-1".to_string());

        let result = NoSandbox.run(request).await.expect("command succeeds");

        assert_eq!(result.stdout, b"hello");
        assert!(
            !artifact_dir.exists()
                || std::fs::read_dir(&artifact_dir)
                    .expect("read artifacts")
                    .next()
                    .is_none()
        );
    }

    #[tokio::test]
    async fn nosandbox_should_sanitize_artifact_filenames() {
        let root = temp_workspace("artifact-sanitize");
        let artifact_dir = root.join("artifacts");
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).expect("create outside dir");
        std::fs::write(outside.join("keep.txt"), "safe").expect("write sentinel");

        let mut request = request(&root, "printf 123456789");
        request.max_output_bytes = 4;
        request.artifact_dir = Some(artifact_dir.clone());
        request.tool_call_id = Some("../../outside/pwn".to_string());

        let result = NoSandbox.run(request).await.expect("command succeeds");

        assert!(result.truncated);
        assert_eq!(
            std::fs::read_to_string(outside.join("keep.txt")).expect("read sentinel"),
            "safe"
        );
        for entry in std::fs::read_dir(&artifact_dir).expect("read artifacts") {
            let path = entry.expect("artifact entry").path();
            assert!(path.starts_with(&artifact_dir));
        }
    }

    #[tokio::test]
    async fn nosandbox_should_reject_shell_tcp_escape_when_network_disabled() {
        let root = temp_workspace("network-deny");
        let result = NoSandbox
            .run(request(&root, "cat /dev/tcp/127.0.0.1/80"))
            .await;

        assert!(matches!(
            result,
            Err(HarnessError::Tool(ToolError::NetworkDenied(_)))
        ));
    }

    #[tokio::test]
    async fn nosandbox_should_pass_only_allowlisted_env() {
        let root = temp_workspace("env");
        let mut request = request(&root, "printf \"%s:%s\" \"$ALLOWED\" \"$SECRET\"");
        request.env.insert("ALLOWED".to_string(), "yes".to_string());
        request.network_policy = NetworkPolicy::Full;

        let result = NoSandbox.run(request).await.expect("command succeeds");

        assert_eq!(result.stdout, b"yes:");
    }

    #[tokio::test]
    async fn nosandbox_should_reject_working_dir_escape() {
        let root = temp_workspace("escape");
        let mut request = request(&root, "pwd");
        request.working_dir = std::env::temp_dir();

        let result = NoSandbox.run(request).await;

        assert!(matches!(
            result,
            Err(HarnessError::Tool(ToolError::PathNotAllowed(_)))
        ));
    }
}
