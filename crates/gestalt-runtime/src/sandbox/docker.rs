#![forbid(unsafe_code)]
//! Container (`docker`) `SandboxDriver` implementation with read-only
//! `protected_paths`, network isolation, and `<= 250 ms` abort cleanup.

use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gestalt_kernel::SandboxPolicy;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::sandbox::{
    SandboxCommand, SandboxDriver, SandboxOutput, build_clean_env, execute_sandboxed_process,
    normalize_rel_prefix,
};
use crate::{CancellationToken, RuntimeError};

fn default_docker_binary() -> String {
    "docker".to_string()
}

fn default_docker_image() -> String {
    "alpine:3.20".to_string()
}

/// Configuration for [`DockerSandboxDriver`] (`[sandbox.docker]` in `gestalt.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerSandboxConfig {
    /// OCI/Docker CLI binary (defaults to `"docker"`).
    #[serde(default = "default_docker_binary")]
    pub binary: String,
    /// Container image used to execute sandboxed commands (defaults to `"alpine:3.20"`).
    #[serde(default = "default_docker_image")]
    pub image: String,
    /// Optional memory limit passed via `--memory` (e.g., `"512m"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
    /// Optional CPU quota passed via `--cpus` (e.g., `"1.0"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_limit: Option<String>,
    /// Additional hardened `docker run` flags.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Default for DockerSandboxConfig {
    fn default() -> Self {
        Self {
            binary: default_docker_binary(),
            image: default_docker_image(),
            memory_limit: None,
            cpu_limit: None,
            extra_args: Vec::new(),
        }
    }
}

/// Container-isolated [`SandboxDriver`] running commands inside ephemeral `docker run`
/// containers with `--cap-drop ALL`, `no-new-privileges`, `--network none` by default,
/// and read-only overlays for all `protected_paths`.
#[derive(Debug, Clone)]
pub struct DockerSandboxDriver {
    config: DockerSandboxConfig,
    container_seq: Arc<AtomicU64>,
}

impl Default for DockerSandboxDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerSandboxDriver {
    /// Construct a [`DockerSandboxDriver`] with default `docker` binary and `alpine:3.20` image.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(DockerSandboxConfig::default())
    }

    /// Construct a [`DockerSandboxDriver`] from an explicit [`DockerSandboxConfig`].
    #[must_use]
    pub fn with_config(config: DockerSandboxConfig) -> Self {
        Self {
            config,
            container_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Construct a [`DockerSandboxDriver`] with an explicit binary path (useful in tests).
    #[must_use]
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self::with_config(DockerSandboxConfig {
            binary: binary.into(),
            ..DockerSandboxConfig::default()
        })
    }

    /// Return the active [`DockerSandboxConfig`].
    #[must_use]
    pub fn config(&self) -> &DockerSandboxConfig {
        &self.config
    }

    /// Build the `docker run` argument vector for `cmd`, `workspace_root`, and `policy`
    /// without checking binary availability (useful for cross-platform profile tests).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Io`] if `workspace_root` cannot be canonicalized.
    pub fn build_docker_args(
        &self,
        cmd: &SandboxCommand,
        workspace_root: &Path,
        policy: &SandboxPolicy,
        container_name: Option<&str>,
    ) -> Result<Vec<String>, RuntimeError> {
        let canon_ws = fs::canonicalize(workspace_root)?;
        let ws_str = canon_ws.to_string_lossy().to_string();
        let canon_cwd = fs::canonicalize(&cmd.cwd).unwrap_or_else(|_| canon_ws.clone());
        let cwd_str = if canon_cwd.starts_with(&canon_ws) {
            canon_cwd.to_string_lossy().to_string()
        } else {
            ws_str.clone()
        };

        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--init".to_string(),
            "--security-opt".to_string(),
            "no-new-privileges".to_string(),
            "--cap-drop".to_string(),
            "ALL".to_string(),
            "--pids-limit".to_string(),
            "256".to_string(),
        ];

        if let Some(name) = container_name {
            args.push("--name".to_string());
            args.push(name.to_string());
        }
        if cmd.stdin_bytes.is_some() {
            args.push("-i".to_string());
        }
        if let Some(ref mem) = self.config.memory_limit {
            args.push("--memory".to_string());
            args.push(mem.clone());
        }
        if let Some(ref cpus) = self.config.cpu_limit {
            args.push("--cpus".to_string());
            args.push(cpus.clone());
        }

        args.push("--network".to_string());
        args.push(if policy.network { "bridge" } else { "none" }.to_string());

        let workspace_ro = policy
            .protected_paths
            .iter()
            .any(|p| normalize_rel_prefix(p) == ".");
        args.push("-v".to_string());
        args.push(format!(
            "{ws_str}:{ws_str}:{}",
            if workspace_ro { "ro" } else { "rw" }
        ));

        if !workspace_ro {
            for prot_path in super::collect_protected_paths(&canon_ws, policy) {
                let p_str = prot_path.to_string_lossy().to_string();
                if prot_path.exists() {
                    args.push("-v".to_string());
                    args.push(format!("{p_str}:{p_str}:ro"));
                } else {
                    args.push("--mount".to_string());
                    args.push(format!("type=tmpfs,destination={p_str},tmpfs-mode=0555"));
                }
            }
        }

        args.push("-w".to_string());
        args.push(cwd_str);

        for (k, v) in build_clean_env(&cmd.env) {
            args.push("-e".to_string());
            args.push(format!("{k}={v}"));
        }

        args.extend(self.config.extra_args.iter().cloned());
        args.push(self.config.image.clone());
        args.push(cmd.program.clone());
        args.extend(cmd.args.iter().cloned());

        Ok(args)
    }
}

impl SandboxDriver for DockerSandboxDriver {
    fn name(&self) -> &str {
        "docker"
    }

    fn is_available(&self) -> bool {
        StdCommand::new(&self.config.binary)
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
                driver: "docker".to_string(),
                reason: format!(
                    "binary '{}' not found or failed --version",
                    self.config.binary
                ),
            });
        }
        let args = self.build_docker_args(cmd, workspace_root, policy, None)?;
        Ok((self.config.binary.clone(), args))
    }

    fn run<'a>(
        &'a self,
        cmd: &'a SandboxCommand,
        workspace_root: &'a Path,
        policy: &'a SandboxPolicy,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SandboxOutput, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if !self.is_available() {
                return Err(RuntimeError::SandboxUnavailable {
                    driver: "docker".to_string(),
                    reason: format!(
                        "binary '{}' not found or failed --version",
                        self.config.binary
                    ),
                });
            }
            let seq = self.container_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let container_name = format!("gestalt-sb-{}-{seq}", std::process::id());
            let args =
                self.build_docker_args(cmd, workspace_root, policy, Some(&container_name))?;

            let out = execute_sandboxed_process(
                &self.config.binary,
                &args,
                workspace_root,
                &std::collections::BTreeMap::new(),
                cmd.stdin_bytes.as_deref(),
                cmd.timeout_ms,
                cancel,
            )
            .await?;

            if out.aborted || out.timed_out {
                let bin = self.config.binary.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(
                        Duration::from_millis(100),
                        Command::new(&bin)
                            .args(["rm", "-f", &container_name])
                            .output(),
                    )
                    .await;
                });
            }

            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn docker_args_enforce_protected_paths_network_isolation_and_clean_env() {
        let dir = std::env::temp_dir().join(format!("gestalt_docker_args_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".gestalt/memories")).expect("mkdir");

        let driver = DockerSandboxDriver::new();
        let mut explicit_env = BTreeMap::new();
        explicit_env.insert("SAFE_VAR".to_string(), "1".to_string());
        let cmd = SandboxCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "echo hi".to_string()],
            cwd: dir.clone(),
            env: explicit_env,
            stdin_bytes: None,
            timeout_ms: Some(1000),
        };
        let policy = SandboxPolicy {
            network: false,
            protected_paths: [".gestalt/memories/".to_string()].into_iter().collect(),
            max_spawn_depth: 1,
        };

        let args = driver
            .build_docker_args(&cmd, &dir, &policy, Some("gestalt-sb-test"))
            .expect("build docker args");
        let joined = args.join(" ");

        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--network none"));
        assert!(joined.contains("--name gestalt-sb-test"));
        assert!(joined.contains(".gestalt/memories:ro"));
        assert!(joined.contains("type=tmpfs,destination="));
        assert!(joined.contains("tmpfs-mode=0555"));
        assert!(joined.contains("-e SAFE_VAR=1"));
        assert!(!joined.contains("ANTHROPIC_API_KEY"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_docker_binary_fails_closed() {
        let driver = DockerSandboxDriver::with_binary("/nonexistent/docker_missing_bin");
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
