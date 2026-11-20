#![forbid(unsafe_code)]
//! macOS `sandbox-exec` (Seatbelt SBPL) `SandboxDriver` implementation.

use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use gestalt_kernel::SandboxPolicy;

use crate::credentials::CredentialStore;
use crate::sandbox::{
    INTERNAL_PROTECTED_PREFIXES, SandboxCommand, SandboxDriver, SandboxOutput,
    execute_sandboxed_process, normalize_rel_prefix,
};
use crate::{CancellationToken, RuntimeError};

const DEFAULT_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// macOS `sandbox-exec` driver generating dynamic Seatbelt SBPL profiles with
/// read-only `protected_paths` overrides and default network isolation.
#[derive(Debug, Clone)]
pub struct MacOsSeatbeltDriver {
    sandbox_exec_bin: String,
}

impl Default for MacOsSeatbeltDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsSeatbeltDriver {
    /// Create a [`MacOsSeatbeltDriver`] using `/usr/bin/sandbox-exec`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sandbox_exec_bin: DEFAULT_SANDBOX_EXEC.to_string(),
        }
    }

    /// Create a [`MacOsSeatbeltDriver`] with an explicit binary path.
    #[must_use]
    pub fn with_binary(sandbox_exec_bin: impl Into<String>) -> Self {
        Self {
            sandbox_exec_bin: sandbox_exec_bin.into(),
        }
    }

    /// Generate a Seatbelt SBPL profile string for `workspace_root` and `policy`.
    #[must_use]
    pub fn generate_sbpl_profile(workspace_root: &Path, policy: &SandboxPolicy) -> String {
        let canon_ws =
            fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
        let ws_escaped = escape_sbpl_str(&canon_ws.to_string_lossy());

        let mut rules = vec![
            "(version 1)".to_string(),
            "(deny default)".to_string(),
            "(allow process*)".to_string(),
            "(allow signal (target self))".to_string(),
            "(allow sysctl-read)".to_string(),
            "(allow mach-lookup)".to_string(),
            "(allow file-read*)".to_string(),
        ];

        if let Some(cfg_dir) = CredentialStore::resolve_config_dir() {
            let cred_path = escape_sbpl_str(&cfg_dir.join("credentials.toml").to_string_lossy());
            rules.push(format!("(deny file-read* (literal \"{cred_path}\"))"));
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.is_empty()
        {
            for sensitive in [".ssh", ".aws", ".gnupg"] {
                let path = escape_sbpl_str(&Path::new(&home).join(sensitive).to_string_lossy());
                rules.push(format!("(deny file-read* (subpath \"{path}\"))"));
            }
        }

        let workspace_ro = policy
            .protected_paths
            .iter()
            .any(|p| normalize_rel_prefix(p) == ".");

        if workspace_ro {
            rules.push(
                "(allow file-write* (subpath \"/tmp\") (subpath \"/private/tmp\") (literal \"/dev/null\"))"
                    .to_string(),
            );
        } else {
            rules.push(format!(
                "(allow file-write* (subpath \"{ws_escaped}\") (subpath \"/tmp\") (subpath \"/private/tmp\") (literal \"/dev/null\"))"
            ));
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
                let prot_full = canon_ws.join(&norm);
                let prot_escaped = escape_sbpl_str(&prot_full.to_string_lossy());
                rules.push(format!("(deny file-write* (subpath \"{prot_escaped}\"))"));
            }
        }

        if policy.network {
            rules.push("(allow network*)".to_string());
        }

        rules.join("\n")
    }
}

impl SandboxDriver for MacOsSeatbeltDriver {
    fn name(&self) -> &str {
        "seatbelt"
    }

    fn is_available(&self) -> bool {
        Path::new(&self.sandbox_exec_bin).exists()
    }

    fn build_os_command(
        &self,
        cmd: &SandboxCommand,
        workspace_root: &Path,
        policy: &SandboxPolicy,
    ) -> Result<(String, Vec<String>), RuntimeError> {
        if !self.is_available() {
            return Err(RuntimeError::SandboxUnavailable {
                driver: "seatbelt".to_string(),
                reason: format!("binary '{}' not found", self.sandbox_exec_bin),
            });
        }
        let profile = Self::generate_sbpl_profile(workspace_root, policy);
        let mut args = vec!["-p".to_string(), profile, cmd.program.clone()];
        args.extend(cmd.args.iter().cloned());
        Ok((self.sandbox_exec_bin.clone(), args))
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

fn escape_sbpl_str(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seatbelt_sbpl_profile_denies_protected_paths_and_network_by_default() {
        let dir = std::env::temp_dir();
        let policy = SandboxPolicy {
            network: false,
            protected_paths: [".gestalt/protected/".to_string()].into_iter().collect(),
            max_spawn_depth: 1,
        };
        let sbpl = MacOsSeatbeltDriver::generate_sbpl_profile(&dir, &policy);
        assert!(sbpl.contains("(deny default)"));
        assert!(sbpl.contains("(allow file-write*"));
        assert!(sbpl.contains("(deny file-write* (subpath"));
        assert!(sbpl.contains(".gestalt/protected"));
        assert!(sbpl.contains(".gestalt/sessions"));
        assert!(!sbpl.contains("(allow network*)"));
    }

    #[test]
    fn missing_seatbelt_binary_fails_closed() {
        let driver = MacOsSeatbeltDriver::with_binary("/nonexistent/sandbox-exec");
        let cmd = SandboxCommand {
            program: "echo".to_string(),
            args: vec!["hi".to_string()],
            cwd: Path::new(".").to_path_buf(),
            env: std::collections::BTreeMap::new(),
            stdin_bytes: None,
            timeout_ms: None,
        };
        let err = driver
            .build_os_command(&cmd, Path::new("."), &SandboxPolicy::default())
            .expect_err("must fail closed");
        assert!(matches!(err, RuntimeError::SandboxUnavailable { .. }));
    }
}
