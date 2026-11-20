#![forbid(unsafe_code)]
//! Linux `bubblewrap` (`bwrap`) `SandboxDriver` implementation.

use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Command as StdCommand;

use gestalt_kernel::SandboxPolicy;

use crate::sandbox::{
    INTERNAL_PROTECTED_PREFIXES, SandboxCommand, SandboxDriver, SandboxOutput, build_clean_env,
    execute_sandboxed_process, normalize_rel_prefix,
};
use crate::{CancellationToken, RuntimeError};

const SYSTEM_RO_PATHS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc/alternatives",
    "/etc/ssl",
    "/etc/ca-certificates",
];

/// Linux `bubblewrap` (`bwrap`) sandbox driver isolating PID, IPC, UTS, and
/// network namespaces while bind-mounting `protected_paths` read-only.
#[derive(Debug, Clone)]
pub struct LinuxBwrapDriver {
    bwrap_bin: String,
}

impl Default for LinuxBwrapDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxBwrapDriver {
    /// Create a [`LinuxBwrapDriver`] using `bwrap` from `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bwrap_bin: "bwrap".to_string(),
        }
    }

    /// Create a [`LinuxBwrapDriver`] with an explicit `bwrap` binary path.
    #[must_use]
    pub fn with_binary(bwrap_bin: impl Into<String>) -> Self {
        Self {
            bwrap_bin: bwrap_bin.into(),
        }
    }
}

impl SandboxDriver for LinuxBwrapDriver {
    fn name(&self) -> &str {
        "bwrap"
    }

    fn is_available(&self) -> bool {
        StdCommand::new(&self.bwrap_bin)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    fn build_os_command(
        &self,
        cmd: &SandboxCommand,
        workspace_root: &Path,
        policy: &SandboxPolicy,
    ) -> Result<(String, Vec<String>), RuntimeError> {
        if !self.is_available() {
            return Err(RuntimeError::SandboxUnavailable {
                driver: "bwrap".to_string(),
                reason: format!("binary '{}' not found or failed --version", self.bwrap_bin),
            });
        }

        let canon_ws = fs::canonicalize(workspace_root)?;
        let ws_str = canon_ws.to_string_lossy().to_string();
        let canon_cwd = fs::canonicalize(&cmd.cwd).unwrap_or_else(|_| canon_ws.clone());
        let cwd_str = if canon_cwd.starts_with(&canon_ws) {
            canon_cwd.to_string_lossy().to_string()
        } else {
            ws_str.clone()
        };

        let mut args = vec![
            "--die-with-parent".to_string(),
            "--unshare-pid".to_string(),
            "--unshare-ipc".to_string(),
            "--unshare-uts".to_string(),
        ];

        if !policy.network {
            args.push("--unshare-net".to_string());
        }

        for sys_path in SYSTEM_RO_PATHS {
            if Path::new(sys_path).exists() {
                args.push("--ro-bind".to_string());
                args.push((*sys_path).to_string());
                args.push((*sys_path).to_string());
            }
        }
        if policy.network && Path::new("/etc/resolv.conf").exists() {
            args.push("--ro-bind".to_string());
            args.push("/etc/resolv.conf".to_string());
            args.push("/etc/resolv.conf".to_string());
        }

        let workspace_ro = policy
            .protected_paths
            .iter()
            .any(|p| normalize_rel_prefix(p) == ".");

        args.extend([
            "--dev".to_string(),
            "/dev".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            if workspace_ro {
                "--ro-bind".to_string()
            } else {
                "--bind".to_string()
            },
            ws_str.clone(),
            ws_str,
        ]);

        // Mount configured protected_paths AND internal harness integrity paths
        // read-only over the workspace bind mount. If a protected directory does
        // not exist yet on disk, shadow it with a read-only tmpfs so the child
        // cannot `mkdir` and write into it.
        if !workspace_ro {
            let mut all_protected = std::collections::BTreeSet::new();
            for raw_prot in &policy.protected_paths {
                let norm = normalize_rel_prefix(raw_prot);
                if !norm.is_empty() && norm != "." {
                    all_protected.insert(norm);
                }
            }
            for internal in INTERNAL_PROTECTED_PREFIXES {
                all_protected.insert((*internal).to_string());
            }

            for norm in all_protected {
                let prot_path = canon_ws.join(&norm);
                let p_str = prot_path.to_string_lossy().to_string();
                if prot_path.exists() {
                    args.push("--ro-bind".to_string());
                    args.push(p_str.clone());
                    args.push(p_str);
                } else {
                    args.push("--tmpfs".to_string());
                    args.push(p_str.clone());
                    args.push("--remount-ro".to_string());
                    args.push(p_str);
                }
            }
        }

        args.push("--chdir".to_string());
        args.push(cwd_str);
        args.push("--clearenv".to_string());

        for (k, v) in build_clean_env(&cmd.env) {
            args.push("--setenv".to_string());
            args.push(k);
            args.push(v);
        }

        args.push("--".to_string());
        args.push(cmd.program.clone());
        args.extend(cmd.args.iter().cloned());

        Ok((self.bwrap_bin.clone(), args))
    }

    fn run<'a>(
        &'a self,
        cmd: &'a SandboxCommand,
        workspace_root: &'a Path,
        policy: &'a SandboxPolicy,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SandboxOutput, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let (program, args) = self.build_os_command(cmd, workspace_root, policy)?;
            execute_sandboxed_process(
                &program,
                &args,
                workspace_root,
                &std::collections::BTreeMap::new(),
                cmd.stdin_bytes.as_deref(),
                cmd.timeout_ms,
                cancel,
            )
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bwrap_enforces_protected_paths_env_clear_and_fast_abort() {
        let driver = LinuxBwrapDriver::new();
        if !driver.is_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("gestalt_bwrap_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".gestalt/protected")).expect("mkdir");
        fs::write(dir.join(".gestalt/protected/data.txt"), "original").expect("write");

        let policy = SandboxPolicy {
            network: false,
            protected_paths: [".gestalt/protected/".to_string()].into_iter().collect(),
            max_spawn_depth: 1,
        };

        // 1. Attempt write to protected path must fail
        let write_cmd = SandboxCommand {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo pwned > .gestalt/protected/data.txt".to_string(),
            ],
            cwd: dir.clone(),
            env: BTreeMap::new(),
            stdin_bytes: None,
            timeout_ms: Some(3_000),
        };
        let out = driver
            .run(&write_cmd, &dir, &policy, CancellationToken::new())
            .await
            .expect("bwrap run");
        assert_ne!(out.exit_code, 0);
        let content = fs::read_to_string(dir.join(".gestalt/protected/data.txt")).expect("read");
        assert_eq!(content, "original");

        // 2. Host secret in env must not leak into sandbox
        let env_cmd = SandboxCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "env".to_string()],
            cwd: dir.clone(),
            env: BTreeMap::new(),
            stdin_bytes: None,
            timeout_ms: Some(3_000),
        };
        let env_out = driver
            .run(&env_cmd, &dir, &policy, CancellationToken::new())
            .await
            .expect("bwrap env");
        let env_str = String::from_utf8_lossy(&env_out.stdout);
        assert!(!env_str.contains("MOCK_API_KEY"));
        assert!(!env_str.contains("HOST_SECRET_TOKEN"));

        // 3. Cancellation must terminate within <= 250 ms
        let sleep_cmd = SandboxCommand {
            program: "sleep".to_string(),
            args: vec!["10".to_string()],
            cwd: dir.clone(),
            env: BTreeMap::new(),
            stdin_bytes: None,
            timeout_ms: None,
        };
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let started = std::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });
        let aborted = driver
            .run(&sleep_cmd, &dir, &policy, cancel)
            .await
            .expect("bwrap abort");
        assert!(aborted.aborted);
        assert!(started.elapsed() <= std::time::Duration::from_millis(250));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_bwrap_binary_fails_closed() {
        let driver = LinuxBwrapDriver::with_binary("/nonexistent/bwrap_missing_bin");
        let cmd = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["hi".to_string()],
            cwd: Path::new(".").to_path_buf(),
            env: BTreeMap::new(),
            stdin_bytes: None,
            timeout_ms: None,
        };
        let err = driver
            .build_os_command(&cmd, Path::new("."), &SandboxPolicy::default())
            .expect_err("must fail closed");
        assert!(matches!(err, RuntimeError::SandboxUnavailable { .. }));
    }
}
