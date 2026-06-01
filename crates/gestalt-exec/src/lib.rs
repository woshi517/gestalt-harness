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
use gestalt_core::{HarnessError, ToolError};
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

#[derive(Debug, Clone, Copy, Default)]
pub struct NoSandbox;

#[async_trait]
impl ExecutionSandbox for NoSandbox {
    async fn run(&self, request: ExecRequest) -> Result<ExecResult, HarnessError> {
        validate_working_dir(&request.working_dir, request.workspace_root.as_deref())?;

        let (program, args) = request
            .command
            .split_first()
            .ok_or_else(|| invalid_input("command must not be empty"))?;

        let started = Instant::now();
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&request.working_dir)
            .env_clear()
            .envs(request.env.iter())
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(ToolError::ExecutionFailed)?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| invalid_input("failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| invalid_input("failed to capture stderr"))?;

        let deadline = TokioInstant::now() + request.timeout;
        let stdout_task = tokio::spawn(read_capped(stdout, request.max_output_bytes, deadline));
        let stderr_task = tokio::spawn(read_capped(stderr, request.max_output_bytes, deadline));

        let wait_result = timeout(request.timeout, child.wait()).await;
        let (exit_code, timed_out) = if let Ok(status) = wait_result {
            (
                Some(status.map_err(ToolError::ExecutionFailed)?.code()),
                false,
            )
        } else {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (None, true)
        };

        let stdout = stdout_task
            .await
            .map_err(|err| invalid_input(format!("stdout task failed: {err}")))??;
        let stderr = stderr_task
            .await
            .map_err(|err| invalid_input(format!("stderr task failed: {err}")))??;
        let mut truncated = stdout.truncated || stderr.truncated;
        let mut stdout = stdout.bytes;
        let mut stderr = stderr.bytes;

        if stdout.len().saturating_add(stderr.len()) > request.max_output_bytes {
            truncate_combined(&mut stdout, &mut stderr, request.max_output_bytes);
            truncated = true;
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

        let remaining = max_output_bytes.saturating_sub(bytes.len());
        if read > remaining {
            bytes.extend_from_slice(&buffer[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
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
        let result = NoSandbox
            .run(request(&root, "printf nope >&2; exit 7"))
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
    async fn nosandbox_should_pass_only_allowlisted_env() {
        let root = temp_workspace("env");
        let mut request = request(&root, "printf \"%s:%s\" \"$ALLOWED\" \"$SECRET\"");
        request.env.insert("ALLOWED".to_string(), "yes".to_string());

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
