#![forbid(unsafe_code)]
//! Micro-VM `SandboxDriver` supporting `Firecracker` on Linux and Apple
//! `Virtualization.framework` (`apple_vz`) on macOS.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gestalt_kernel::SandboxPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::sandbox::{
    SandboxCommand, SandboxDriver, SandboxOutput, build_clean_env, execute_sandboxed_process,
    normalize_rel_prefix,
};
use crate::{CancellationToken, RuntimeError};

/// Host hypervisor backend used by [`MicroVmSandboxDriver`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicroVmBackend {
    /// Linux KVM Firecracker micro-VM (`firecracker` / `jailer`).
    #[cfg_attr(not(target_os = "macos"), default)]
    Firecracker,
    /// macOS Apple `Virtualization.framework` runner (`gestalt-vz-runner` / `vfkit`).
    #[cfg_attr(target_os = "macos", default)]
    AppleVz,
}

fn default_microvm_binary() -> String {
    #[cfg(target_os = "macos")]
    {
        "gestalt-vz-runner".to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "firecracker".to_string()
    }
}

fn default_vcpu_count() -> u8 {
    1
}

fn default_mem_mib() -> u32 {
    256
}

/// Configuration for [`MicroVmSandboxDriver`] (`[sandbox.microvm]` in `gestalt.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmSandboxConfig {
    /// Micro-VM hypervisor backend (`"firecracker"` on Linux, `"apple_vz"` on macOS).
    #[serde(default)]
    pub backend: MicroVmBackend,
    /// VMM binary name or path.
    #[serde(default = "default_microvm_binary")]
    pub binary: String,
    /// Path to uncompressed guest Linux kernel image (`vmlinux` / `Image`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_image: Option<PathBuf>,
    /// Path to read-only guest root filesystem image (`ext4` / `squashfs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rootfs_image: Option<PathBuf>,
    /// Number of virtual CPUs allocated to the micro-VM (default: `1`).
    #[serde(default = "default_vcpu_count")]
    pub vcpu_count: u8,
    /// Guest RAM allocation in MiB (default: `256`).
    #[serde(default = "default_mem_mib")]
    pub mem_mib: u32,
}

impl Default for MicroVmSandboxConfig {
    fn default() -> Self {
        Self {
            backend: MicroVmBackend::default(),
            binary: default_microvm_binary(),
            kernel_image: None,
            rootfs_image: None,
            vcpu_count: default_vcpu_count(),
            mem_mib: default_mem_mib(),
        }
    }
}

/// Hardware-virtualized micro-VM [`SandboxDriver`] (`Firecracker` on Linux,
/// Apple `Virtualization.framework` on macOS).
#[derive(Debug, Clone)]
pub struct MicroVmSandboxDriver {
    config: MicroVmSandboxConfig,
    vm_seq: Arc<AtomicU64>,
}

impl Default for MicroVmSandboxDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MicroVmSandboxDriver {
    /// Construct a [`MicroVmSandboxDriver`] with host-OS defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(MicroVmSandboxConfig::default())
    }

    /// Construct a [`MicroVmSandboxDriver`] from an explicit [`MicroVmSandboxConfig`].
    #[must_use]
    pub fn with_config(config: MicroVmSandboxConfig) -> Self {
        Self {
            config,
            vm_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Return the active [`MicroVmSandboxConfig`].
    #[must_use]
    pub fn config(&self) -> &MicroVmSandboxConfig {
        &self.config
    }

    fn require_guest_assets(&self) -> Result<(&Path, &Path), RuntimeError> {
        let kernel = self
            .config
            .kernel_image
            .as_deref()
            .filter(|p| p.is_file())
            .ok_or_else(|| RuntimeError::SandboxUnavailable {
                driver: "microvm".to_string(),
                reason: "microvm kernel_image is missing or not a file".to_string(),
            })?;
        let rootfs = self
            .config
            .rootfs_image
            .as_deref()
            .filter(|p| p.is_file())
            .ok_or_else(|| RuntimeError::SandboxUnavailable {
                driver: "microvm".to_string(),
                reason: "microvm rootfs_image is missing or not a file".to_string(),
            })?;
        Ok((kernel, rootfs))
    }

    /// Generate a deterministic Firecracker micro-VM JSON configuration for
    /// `cmd`, `workspace_root`, and `policy`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxUnavailable`] if guest assets are missing
    /// or [`RuntimeError::Io`] if `workspace_root` cannot be canonicalized.
    pub fn generate_firecracker_config(
        &self,
        cmd: &SandboxCommand,
        workspace_root: &Path,
        policy: &SandboxPolicy,
    ) -> Result<Value, RuntimeError> {
        let (kernel, rootfs) = self.require_guest_assets()?;
        let canon_ws = fs::canonicalize(workspace_root)?;
        let ws_str = canon_ws.to_string_lossy().to_string();
        let canon_cwd = fs::canonicalize(&cmd.cwd).unwrap_or_else(|_| canon_ws.clone());
        let cwd_str = if canon_cwd.starts_with(&canon_ws) {
            canon_cwd.to_string_lossy().to_string()
        } else {
            ws_str.clone()
        };

        let workspace_ro = policy
            .protected_paths
            .iter()
            .any(|p| normalize_rel_prefix(p) == ".");
        let protected_mounts: Vec<Value> = super::collect_protected_paths(&canon_ws, policy)
            .into_iter()
            .map(|p| {
                json!({
                    "path": p.to_string_lossy(),
                    "read_only": true,
                })
            })
            .collect();

        let clean_env = build_clean_env(&cmd.env);
        let net_ifaces = if policy.network {
            vec![json!({
                "iface_id": "eth0",
                "guest_mac": "AA:FC:00:00:00:01",
                "host_dev_name": "tap0",
            })]
        } else {
            Vec::new()
        };

        Ok(json!({
            "boot-source": {
                "kernel_image_path": kernel.to_string_lossy(),
                "boot_args": "console=ttyS0 reboot=k panic=1 pci=off",
            },
            "drives": [
                {
                    "drive_id": "rootfs",
                    "path_on_host": rootfs.to_string_lossy(),
                    "is_root_device": true,
                    "is_read_only": true,
                }
            ],
            "machine-config": {
                "vcpu_count": self.config.vcpu_count.max(1),
                "mem_size_mib": self.config.mem_mib.max(64),
            },
            "network-interfaces": net_ifaces,
            "gestalt_guest_exec": {
                "workspace_root": ws_str,
                "workspace_read_only": workspace_ro,
                "protected_mounts": protected_mounts,
                "cwd": cwd_str,
                "env": clean_env,
                "program": cmd.program,
                "args": cmd.args,
            }
        }))
    }

    /// Build deterministic Apple `Virtualization.framework` (`apple_vz`) runner
    /// CLI arguments for `cmd`, `workspace_root`, and `policy`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SandboxUnavailable`] if guest assets are missing
    /// or [`RuntimeError::Io`] if `workspace_root` cannot be canonicalized.
    pub fn build_apple_vz_args(
        &self,
        cmd: &SandboxCommand,
        workspace_root: &Path,
        policy: &SandboxPolicy,
    ) -> Result<Vec<String>, RuntimeError> {
        let (kernel, rootfs) = self.require_guest_assets()?;
        let canon_ws = fs::canonicalize(workspace_root)?;
        let ws_str = canon_ws.to_string_lossy().to_string();
        let canon_cwd = fs::canonicalize(&cmd.cwd).unwrap_or_else(|_| canon_ws.clone());
        let cwd_str = if canon_cwd.starts_with(&canon_ws) {
            canon_cwd.to_string_lossy().to_string()
        } else {
            ws_str.clone()
        };

        let mut args = vec![
            "--kernel".to_string(),
            kernel.to_string_lossy().to_string(),
            "--rootfs".to_string(),
            rootfs.to_string_lossy().to_string(),
            "--cpus".to_string(),
            self.config.vcpu_count.max(1).to_string(),
            "--memory-mib".to_string(),
            self.config.mem_mib.max(64).to_string(),
            "--network".to_string(),
            if policy.network { "nat" } else { "none" }.to_string(),
        ];

        let workspace_ro = policy
            .protected_paths
            .iter()
            .any(|p| normalize_rel_prefix(p) == ".");
        args.push(if workspace_ro {
            "--virtiofs-ro".to_string()
        } else {
            "--virtiofs-rw".to_string()
        });
        args.push(format!("{ws_str}:{ws_str}"));

        if !workspace_ro {
            for prot_path in super::collect_protected_paths(&canon_ws, policy) {
                let p_str = prot_path.to_string_lossy();
                args.push("--virtiofs-ro".to_string());
                args.push(format!("{p_str}:{p_str}"));
            }
        }

        args.push("--workdir".to_string());
        args.push(cwd_str);
        args.push("--clearenv".to_string());

        for (k, v) in build_clean_env(&cmd.env) {
            args.push("--env".to_string());
            args.push(format!("{k}={v}"));
        }

        args.push("--".to_string());
        args.push(cmd.program.clone());
        args.extend(cmd.args.iter().cloned());
        Ok(args)
    }
}

impl SandboxDriver for MicroVmSandboxDriver {
    fn name(&self) -> &str {
        "microvm"
    }

    fn is_available(&self) -> bool {
        if self.require_guest_assets().is_err() {
            return false;
        }
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
                driver: "microvm".to_string(),
                reason: format!(
                    "micro-VM backend '{:?}' binary '{}' or guest kernel/rootfs unavailable",
                    self.config.backend, self.config.binary
                ),
            });
        }
        match self.config.backend {
            MicroVmBackend::Firecracker => {
                let _ = self.generate_firecracker_config(cmd, workspace_root, policy)?;
                Ok((
                    self.config.binary.clone(),
                    vec![
                        "--no-api".to_string(),
                        "--config-file".to_string(),
                        "<ephemeral-firecracker.json>".to_string(),
                    ],
                ))
            }
            MicroVmBackend::AppleVz => {
                let args = self.build_apple_vz_args(cmd, workspace_root, policy)?;
                Ok((self.config.binary.clone(), args))
            }
        }
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
                    driver: "microvm".to_string(),
                    reason: format!(
                        "micro-VM backend '{:?}' binary '{}' or guest kernel/rootfs unavailable",
                        self.config.backend, self.config.binary
                    ),
                });
            }

            match self.config.backend {
                MicroVmBackend::Firecracker => {
                    let cfg_json = self.generate_firecracker_config(cmd, workspace_root, policy)?;
                    let seq = self.vm_seq.fetch_add(1, Ordering::Relaxed) + 1;
                    let cfg_path = std::env::temp_dir()
                        .join(format!("gestalt_fc_{}_{seq}.json", std::process::id()));
                    let encoded = serde_json::to_vec_pretty(&cfg_json)?;
                    fs::write(&cfg_path, encoded)?;
                    let args = vec![
                        "--no-api".to_string(),
                        "--config-file".to_string(),
                        cfg_path.to_string_lossy().to_string(),
                    ];
                    let res = execute_sandboxed_process(
                        &self.config.binary,
                        &args,
                        workspace_root,
                        &std::collections::BTreeMap::new(),
                        cmd.stdin_bytes.as_deref(),
                        cmd.timeout_ms,
                        cancel,
                    )
                    .await;
                    let _ = fs::remove_file(&cfg_path);
                    res
                }
                MicroVmBackend::AppleVz => {
                    let args = self.build_apple_vz_args(cmd, workspace_root, policy)?;
                    execute_sandboxed_process(
                        &self.config.binary,
                        &args,
                        workspace_root,
                        &std::collections::BTreeMap::new(),
                        cmd.stdin_bytes.as_deref(),
                        cmd.timeout_ms,
                        cancel,
                    )
                    .await
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn firecracker_and_apple_vz_configs_enforce_readonly_protected_paths_and_no_net() {
        let dir = std::env::temp_dir().join(format!("gestalt_microvm_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let kernel = dir.join("vmlinux");
        let rootfs = dir.join("rootfs.ext4");
        fs::write(&kernel, "kernel").expect("kernel");
        fs::write(&rootfs, "rootfs").expect("rootfs");

        let policy = SandboxPolicy {
            network: false,
            protected_paths: [".gestalt/memories/".to_string()].into_iter().collect(),
            max_spawn_depth: 1,
        };
        let mut env = BTreeMap::new();
        env.insert("SAFE_VAR".to_string(), "ok".to_string());
        let cmd = SandboxCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "echo hi".to_string()],
            cwd: dir.clone(),
            env,
            stdin_bytes: None,
            timeout_ms: Some(1000),
        };

        let fc_driver = MicroVmSandboxDriver::with_config(MicroVmSandboxConfig {
            backend: MicroVmBackend::Firecracker,
            binary: "firecracker".to_string(),
            kernel_image: Some(kernel.clone()),
            rootfs_image: Some(rootfs.clone()),
            vcpu_count: 2,
            mem_mib: 512,
        });
        let fc_cfg = fc_driver
            .generate_firecracker_config(&cmd, &dir, &policy)
            .expect("fc config");
        assert_eq!(fc_cfg["network-interfaces"], json!([]));
        assert_eq!(fc_cfg["drives"][0]["is_read_only"], true);
        let prot_str = fc_cfg["gestalt_guest_exec"]["protected_mounts"].to_string();
        assert!(prot_str.contains(".gestalt/memories"));
        assert!(prot_str.contains(".gestalt/sessions"));
        assert_eq!(fc_cfg["gestalt_guest_exec"]["env"]["SAFE_VAR"], "ok");

        let vz_driver = MicroVmSandboxDriver::with_config(MicroVmSandboxConfig {
            backend: MicroVmBackend::AppleVz,
            binary: "gestalt-vz-runner".to_string(),
            kernel_image: Some(kernel),
            rootfs_image: Some(rootfs),
            vcpu_count: 2,
            mem_mib: 512,
        });
        let vz_args = vz_driver
            .build_apple_vz_args(&cmd, &dir, &policy)
            .expect("vz args");
        let vz_joined = vz_args.join(" ");
        assert!(vz_joined.contains("--network none"));
        assert!(vz_joined.contains("--virtiofs-ro"));
        assert!(vz_joined.contains(".gestalt/memories"));
        assert!(vz_joined.contains("--env SAFE_VAR=ok"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_microvm_assets_fail_closed() {
        let driver = MicroVmSandboxDriver::new();
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
