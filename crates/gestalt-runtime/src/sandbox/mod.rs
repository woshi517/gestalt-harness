#![forbid(unsafe_code)]
//! Two-layer sandbox enforcement: in-process `WorkspacePathGuard` and OS
//! subprocess `SandboxDriver` (`bubblewrap` on Linux, `Seatbelt` on macOS).

pub mod linux_bwrap;
pub mod macos_seatbelt;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use gestalt_kernel::{SandboxPolicy, SideEffectClass};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::{CancellationToken, RuntimeError};

pub use linux_bwrap::LinuxBwrapDriver;
pub use macos_seatbelt::MacOsSeatbeltDriver;

/// Internal harness integrity prefixes always protected from direct regular-tool writes
/// across both `WorkspacePathGuard` and OS `SandboxDriver`s.
pub const INTERNAL_PROTECTED_PREFIXES: &[&str] = &[
    ".gestalt/sessions",
    ".gestalt/blobs",
    ".gestalt/ledger.jsonl",
    ".gestalt/staging",
    ".gestalt/worktrees",
];

/// Safe baseline environment variables forwarded to sandboxed child processes
/// after `env_clear()`.
const SAFE_ENV_ALLOWLIST: &[&str] = &["PATH", "HOME", "TERM", "LANG", "LC_ALL", "TMPDIR"];

/// In-process path guard enforcing workspace confinement, symlink-escape rejection,
/// read-only `protected_paths`, and dynamic `Irreversible` elevation on file overwrite.
#[derive(Debug, Clone)]
pub struct WorkspacePathGuard {
    workspace_root: PathBuf,
    protected_prefixes: BTreeSet<String>,
}

impl WorkspacePathGuard {
    /// Construct a [`WorkspacePathGuard`] anchored at canonical `workspace_root`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Io`] if `workspace_root` cannot be canonicalized.
    pub fn new(
        workspace_root: &Path,
        policy_protected_paths: &BTreeSet<String>,
    ) -> Result<Self, RuntimeError> {
        let canon_root = fs::canonicalize(workspace_root)?;
        let mut protected_prefixes = BTreeSet::new();
        for p in policy_protected_paths {
            let norm = normalize_rel_prefix(p);
            if !norm.is_empty() {
                protected_prefixes.insert(norm);
            }
        }
        for internal in INTERNAL_PROTECTED_PREFIXES {
            protected_prefixes.insert((*internal).to_string());
        }
        Ok(Self {
            workspace_root: canon_root,
            protected_prefixes,
        })
    }

    /// Return the canonical workspace root path.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Return all normalized protected path prefixes.
    #[must_use]
    pub fn protected_prefixes(&self) -> &BTreeSet<String> {
        &self.protected_prefixes
    }

    /// Resolve a candidate path for reading, rejecting `..` or symlink escapes
    /// outside `workspace_root`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxViolation`] if the path escapes `workspace_root`.
    pub fn resolve_read(&self, raw_path: &str) -> Result<PathBuf, RuntimeError> {
        self.resolve_confined(raw_path)
    }

    /// Resolve a candidate path for writing, rejecting `..`/symlink escapes and
    /// any `protected_paths` prefix, and dynamically elevating overwrites to
    /// [`SideEffectClass::Irreversible`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxViolation`] if the path escapes `workspace_root`
    /// or targets a protected path.
    pub fn resolve_write(
        &self,
        raw_path: &str,
    ) -> Result<(PathBuf, SideEffectClass), RuntimeError> {
        let resolved = self.resolve_confined(raw_path)?;
        self.ensure_not_protected(&resolved)?;
        let side_effect = if resolved.exists() {
            SideEffectClass::Irreversible
        } else {
            SideEffectClass::Idempotent
        };
        Ok((resolved, side_effect))
    }

    /// Resolve a candidate path for deletion, rejecting escapes and `protected_paths`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxViolation`] if the path escapes `workspace_root`
    /// or targets a protected path.
    pub fn resolve_delete(&self, raw_path: &str) -> Result<PathBuf, RuntimeError> {
        let resolved = self.resolve_confined(raw_path)?;
        self.ensure_not_protected(&resolved)?;
        Ok(resolved)
    }

    /// Check whether a canonical or resolved path inside `workspace_root` falls
    /// under any protected prefix.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxViolation`] if the path is protected.
    pub fn ensure_not_protected(&self, resolved_path: &Path) -> Result<(), RuntimeError> {
        let rel = resolved_path
            .strip_prefix(&self.workspace_root)
            .map_err(|_| {
                RuntimeError::SandboxViolation(format!(
                    "path '{}' escapes workspace root '{}'",
                    resolved_path.display(),
                    self.workspace_root.display()
                ))
            })?;
        let rel_str = normalize_rel_prefix(&rel.to_string_lossy());
        for prefix in &self.protected_prefixes {
            if rel_str == *prefix || rel_str.starts_with(&format!("{prefix}/")) {
                return Err(RuntimeError::SandboxViolation(format!(
                    "write to protected path '{rel_str}' is denied (matches '{prefix}')"
                )));
            }
        }
        Ok(())
    }

    fn resolve_confined(&self, raw_path: &str) -> Result<PathBuf, RuntimeError> {
        if raw_path.contains('\0') {
            return Err(RuntimeError::SandboxViolation(
                "null byte in path".to_string(),
            ));
        }

        let input = Path::new(raw_path);
        let joined = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.workspace_root.join(input)
        };

        // Walk upward to the deepest existing ancestor (checking symlink_metadata so
        // dangling symlinks do not bypass confinement).
        let mut tail_segments = Vec::new();
        let mut cursor = joined.as_path();
        loop {
            match fs::symlink_metadata(cursor) {
                Ok(_) => {
                    let canon_ancestor = fs::canonicalize(cursor).map_err(|err| {
                        RuntimeError::SandboxViolation(format!(
                            "failed to canonicalize '{}': {err}",
                            cursor.display()
                        ))
                    })?;
                    if !canon_ancestor.starts_with(&self.workspace_root) {
                        return Err(RuntimeError::SandboxViolation(format!(
                            "path '{}' escapes workspace root '{}'",
                            raw_path,
                            self.workspace_root.display()
                        )));
                    }
                    let mut final_path = canon_ancestor;
                    for seg in tail_segments.into_iter().rev() {
                        final_path.push(seg);
                    }
                    if !final_path.starts_with(&self.workspace_root) {
                        return Err(RuntimeError::SandboxViolation(format!(
                            "resolved path '{}' escapes workspace root",
                            final_path.display()
                        )));
                    }
                    return Ok(final_path);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let Some(name) = cursor.file_name() else {
                        return Err(RuntimeError::SandboxViolation(format!(
                            "path '{raw_path}' has no existing ancestor inside workspace"
                        )));
                    };
                    if name == ".." {
                        return Err(RuntimeError::SandboxViolation(format!(
                            "parent traversal '..' rejected in '{raw_path}'"
                        )));
                    }
                    tail_segments.push(name.to_os_string());
                    let Some(parent) = cursor.parent() else {
                        return Err(RuntimeError::SandboxViolation(format!(
                            "path '{raw_path}' escapes workspace root"
                        )));
                    };
                    cursor = parent;
                }
                Err(err) => {
                    return Err(RuntimeError::SandboxViolation(format!(
                        "unable to inspect '{}': {err}",
                        cursor.display()
                    )));
                }
            }
        }
    }
}

/// Command specification passed to a [`SandboxDriver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCommand {
    /// Executable program name or path.
    pub program: String,
    /// Arguments passed to `program`.
    pub args: Vec<String>,
    /// Working directory inside the sandbox.
    pub cwd: PathBuf,
    /// Explicit environment variables added after `env_clear()`.
    pub env: BTreeMap<String, String>,
    /// Optional stdin payload written and closed immediately after spawn.
    pub stdin_bytes: Option<Vec<u8>>,
    /// Optional execution timeout in milliseconds.
    pub timeout_ms: Option<u64>,
}

/// Completed execution output returned by a [`SandboxDriver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxOutput {
    /// Child process exit code (`-1` if terminated by signal/timeout/abort).
    pub exit_code: i32,
    /// Captured stdout bytes.
    pub stdout: Vec<u8>,
    /// Captured stderr bytes.
    pub stderr: Vec<u8>,
    /// `true` if the command exceeded `timeout_ms`.
    pub timed_out: bool,
    /// `true` if cancelled via [`CancellationToken`].
    pub aborted: bool,
}

/// OS-level subprocess sandbox driver (`bubblewrap` on Linux, `Seatbelt` on macOS).
pub trait SandboxDriver: Send + Sync {
    /// Human-readable driver identifier (`"bwrap"`, `"seatbelt"`, `"unsafe_none"`).
    fn name(&self) -> &str;

    /// Return `true` if the required OS sandbox binary is present on this host.
    fn is_available(&self) -> bool;

    /// Build the OS-level command and argument list without executing it
    /// (enables cross-platform unit testing of sandbox profiles/flags).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxUnavailable`] or [`RuntimeError::SandboxViolation`].
    fn build_os_command(
        &self,
        cmd: &SandboxCommand,
        workspace_root: &Path,
        policy: &SandboxPolicy,
    ) -> Result<(String, Vec<String>), RuntimeError>;

    /// Execute `cmd` inside the OS sandbox, respecting `policy` and `cancel`.
    fn run<'a>(
        &'a self,
        cmd: &'a SandboxCommand,
        workspace_root: &'a Path,
        policy: &'a SandboxPolicy,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SandboxOutput, RuntimeError>> + Send + 'a>>;
}

/// Select the default [`SandboxDriver`] for the current host OS.
///
/// Fails closed with [`UnavailableSandboxDriver`] if the OS sandbox binary is
/// missing unless `unsafe_no_sandbox` is explicitly `true`.
#[must_use]
pub fn default_sandbox_driver(unsafe_no_sandbox: bool) -> Arc<dyn SandboxDriver> {
    #[cfg(target_os = "linux")]
    {
        let bwrap = LinuxBwrapDriver::new();
        if bwrap.is_available() {
            return Arc::new(bwrap);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let seatbelt = MacOsSeatbeltDriver::new();
        if seatbelt.is_available() {
            return Arc::new(seatbelt);
        }
    }
    if unsafe_no_sandbox {
        Arc::new(UnsandboxedFallbackDriver)
    } else {
        Arc::new(UnavailableSandboxDriver)
    }
}

/// Fail-closed placeholder driver returned when no OS sandbox binary is available.
#[derive(Debug, Default)]
pub struct UnavailableSandboxDriver;

impl SandboxDriver for UnavailableSandboxDriver {
    fn name(&self) -> &str {
        "unavailable"
    }

    fn is_available(&self) -> bool {
        false
    }

    fn build_os_command(
        &self,
        _cmd: &SandboxCommand,
        _workspace_root: &Path,
        _policy: &SandboxPolicy,
    ) -> Result<(String, Vec<String>), RuntimeError> {
        Err(RuntimeError::SandboxUnavailable {
            driver: "os_sandbox".to_string(),
            reason: "no supported OS sandbox binary found (bwrap/sandbox-exec); pass --unsafe-no-sandbox to override".to_string(),
        })
    }

    fn run<'a>(
        &'a self,
        cmd: &'a SandboxCommand,
        workspace_root: &'a Path,
        policy: &'a SandboxPolicy,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SandboxOutput, RuntimeError>> + Send + 'a>> {
        let err = self
            .build_os_command(cmd, workspace_root, policy)
            .expect_err("unavailable always errors");
        Box::pin(async move { Err(err) })
    }
}

/// Explicit opt-in unsandboxed driver used only when `--unsafe-no-sandbox` is passed.
#[derive(Debug, Default)]
pub struct UnsandboxedFallbackDriver;

impl SandboxDriver for UnsandboxedFallbackDriver {
    fn name(&self) -> &str {
        "unsafe_none"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn build_os_command(
        &self,
        cmd: &SandboxCommand,
        _workspace_root: &Path,
        _policy: &SandboxPolicy,
    ) -> Result<(String, Vec<String>), RuntimeError> {
        Ok((cmd.program.clone(), cmd.args.clone()))
    }

    fn run<'a>(
        &'a self,
        cmd: &'a SandboxCommand,
        _workspace_root: &'a Path,
        _policy: &'a SandboxPolicy,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SandboxOutput, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            tracing::warn!(
                program = %cmd.program,
                "executing subprocess WITHOUT OS sandbox (--unsafe-no-sandbox active)"
            );
            execute_sandboxed_process(
                &cmd.program,
                &cmd.args,
                &cmd.cwd,
                &cmd.env,
                cmd.stdin_bytes.as_deref(),
                cmd.timeout_ms,
                cancel,
            )
            .await
        })
    }
}

/// Build the safe sanitized environment map (`env_clear()` + allowlist + explicit vars).
#[must_use]
pub fn build_clean_env(explicit: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in SAFE_ENV_ALLOWLIST {
        if let Ok(val) = std::env::var(key) {
            env.insert((*key).to_string(), val);
        }
    }
    for (k, v) in explicit {
        env.insert(k.clone(), v.clone());
    }
    env
}

/// Spawn and supervise a child process with `env_clear()`, process-group isolation,
/// optional stdin/timeout, and immediate `CancellationToken` termination.
///
/// # Errors
///
/// Returns [`RuntimeError::Io`] if spawning or reading pipes fails.
pub async fn execute_sandboxed_process(
    program: &str,
    args: &[String],
    cwd: &Path,
    explicit_env: &BTreeMap<String, String>,
    stdin_bytes: Option<&[u8]>,
    timeout_ms: Option<u64>,
    cancel: CancellationToken,
) -> Result<SandboxOutput, RuntimeError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(build_clean_env(explicit_env))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin_bytes.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

    #[cfg(unix)]
    {
        command.process_group(0);
    }

    let mut child = command.spawn()?;
    if let Some(input) = stdin_bytes
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(input).await;
        let _ = stdin.shutdown().await;
    }

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stdout_pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stderr_pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    });

    let mut timed_out = false;
    let mut aborted = false;

    let timeout_fut = async {
        match timeout_ms {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => std::future::pending().await,
        }
    };

    let exit_code = tokio::select! {
        status = child.wait() => {
            status?.code().unwrap_or(-1)
        }
        () = cancel.cancelled() => {
            aborted = true;
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(100), child.wait()).await;
            -1
        }
        () = timeout_fut => {
            timed_out = true;
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(100), child.wait()).await;
            -1
        }
    };

    let stdout = tokio::time::timeout(Duration::from_millis(100), stdout_task)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let stderr = tokio::time::timeout(Duration::from_millis(100), stderr_task)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();

    Ok(SandboxOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
        aborted,
    })
}

/// Normalize a workspace-relative prefix by stripping leading `./` and trailing `/`.
#[must_use]
pub fn normalize_rel_prefix(raw: &str) -> String {
    raw.trim_start_matches("./")
        .trim_end_matches('/')
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_guard_blocks_escapes_and_protected_paths() {
        let dir = std::env::temp_dir().join(format!("gestalt_guard_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".gestalt/protected/demo")).expect("mkdir");
        fs::write(dir.join("existing.txt"), "hi").expect("write");

        let protected = [".gestalt/protected/".to_string()].into_iter().collect();
        let guard = WorkspacePathGuard::new(&dir, &protected).expect("guard");

        let (new_path, new_class) = guard.resolve_write("new_file.txt").expect("new write");
        assert!(new_path.ends_with("new_file.txt"));
        assert_eq!(new_class, SideEffectClass::Idempotent);

        let (_, existing_class) = guard.resolve_write("existing.txt").expect("overwrite");
        assert_eq!(existing_class, SideEffectClass::Irreversible);

        assert!(matches!(
            guard.resolve_write(".gestalt/protected/demo/note.txt"),
            Err(RuntimeError::SandboxViolation(_))
        ));
        assert!(matches!(
            guard.resolve_read("../../etc/passwd"),
            Err(RuntimeError::SandboxViolation(_))
        ));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.join("evil_link")).expect("symlink");
            assert!(matches!(
                guard.resolve_read("evil_link"),
                Err(RuntimeError::SandboxViolation(_))
            ));
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
