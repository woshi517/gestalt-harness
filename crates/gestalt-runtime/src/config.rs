#![forbid(unsafe_code)]
//! Global and Workspace `gestalt.toml` loader, monotonic policy merging, and
//! `(canonical_workspace_path, sha256(gestalt.toml))` trust gate.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use gestalt_kernel::{
    BudgetCap, ExtensionRegistry, KernelError, LoopConfig, PolicyGate, PolicyOverride, Preset,
    SandboxPolicy, sha256_hex,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RuntimeError;
use crate::credentials::CredentialStore;
use crate::sandbox::{DockerSandboxConfig, MicroVmSandboxConfig, SandboxKind};

fn default_ext_timeout_ms() -> u64 {
    5_000
}

/// Out-of-process extension transport protocol in `gestalt.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionProtocol {
    /// Gestalt stdio JSON-RPC 2.0 extension worker.
    #[default]
    Rpc,
    /// Standard stdio Model Context Protocol (MCP) server.
    Mcp,
}

/// `[defaults]` section in `gestalt.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Active default preset name.
    pub preset: Option<String>,
    /// Directory where `SessionTree` JSONL files are stored.
    pub session_dir: Option<String>,
}

/// `[budget]` section in `gestalt.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum number of Steps permitted.
    pub max_steps: Option<u64>,
    /// Maximum total tokens permitted.
    pub max_tokens: Option<u64>,
    /// Maximum cost in USD (converted deterministically to `micro_usd`).
    pub max_cost_usd: Option<f64>,
}

impl BudgetConfig {
    /// Convert this TOML budget config into an integer-unit [`BudgetCap`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ArtifactError`] if `max_cost_usd` is negative, NaN, or infinite.
    pub fn to_budget_cap(&self) -> Result<BudgetCap, RuntimeError> {
        let max_cost_micro_usd = match self.max_cost_usd {
            Some(usd) => {
                if !usd.is_finite() || usd < 0.0 {
                    return Err(RuntimeError::ArtifactError(format!(
                        "invalid max_cost_usd value: {usd}"
                    )));
                }
                Some((usd * 1_000_000.0).round() as u64)
            }
            None => None,
        };
        Ok(BudgetCap {
            max_steps: self.max_steps,
            max_tokens: self.max_tokens,
            max_cost_micro_usd,
        })
    }
}

/// `[sandbox]` section in `gestalt.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Whether outbound network access is allowed in sandboxed subprocesses.
    pub network: Option<bool>,
    /// Workspace-relative paths mounted read-only inside sandboxes.
    pub protected_paths: Option<Vec<String>>,
    /// Maximum subagent spawn depth permitted.
    pub max_spawn_depth: Option<u8>,
    /// Selected sandbox isolation driver (`"os"`, `"docker"`, `"microvm"`).
    pub driver: Option<SandboxKind>,
    /// Optional `[sandbox.docker]` container configuration.
    pub docker: Option<DockerSandboxConfig>,
    /// Optional `[sandbox.microvm]` Firecracker / Apple Virtualization configuration.
    pub microvm: Option<MicroVmSandboxConfig>,
}

/// `[policy]` section in `gestalt.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Pause reasons/actions explicitly auto-approved in `-p` headless mode.
    #[serde(default)]
    pub headless_auto_approve: Vec<String>,
    /// Optional baseline tool allowlist.
    pub allowed_tools: Option<Vec<String>>,
}

/// `[providers.<id>]` section in `gestalt.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Optional registered `ModelProvider::id()` adapter backing this alias.
    /// Defaults to the `[providers.<id>]` table key when omitted.
    pub adapter: Option<String>,
    /// Optional custom HTTP endpoint URL.
    pub endpoint: Option<String>,
    /// Default model identifier for this provider alias.
    pub model: String,
    /// Input token price in USD per 1M tokens.
    pub input_cost_per_mtok_usd: Option<f64>,
    /// Output token price in USD per 1M tokens.
    pub output_cost_per_mtok_usd: Option<f64>,
    /// Optional explicit environment variable name for the API key.
    pub env_key: Option<String>,
    /// Additional provider-specific settings preserved for adapters.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `[presets.<id>]` section in `gestalt.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetConfig {
    /// Base `AgentLoopFactory` slot ID.
    pub base_loop: String,
    /// Ordered `LoopDecorator` slot IDs.
    #[serde(default)]
    pub decorators: Vec<String>,
    /// Active `ContextBuilder` slot ID.
    pub builder: String,
    /// Optional tool allowlist narrowing this preset.
    pub allowed_tools: Option<Vec<String>>,
    /// Optional spawn depth cap narrowing this preset.
    pub max_spawn_depth: Option<u8>,
    /// Optional tighter budget cap for this preset.
    pub budget: Option<BudgetConfig>,
    /// Opaque user-space parameters.
    #[serde(default)]
    pub params: Value,
}

impl PresetConfig {
    /// Convert this TOML preset definition into a kernel [`Preset`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `budget.max_cost_usd` is invalid.
    pub fn to_preset(&self) -> Result<Preset, RuntimeError> {
        let budget = match &self.budget {
            Some(b) => Some(b.to_budget_cap()?),
            None => None,
        };
        Ok(Preset {
            base_loop: self.base_loop.clone(),
            decorators: self.decorators.clone(),
            builder: self.builder.clone(),
            config: LoopConfig {
                allowed_tools: self.allowed_tools.clone(),
                max_spawn_depth: self.max_spawn_depth,
                budget,
                params: self.params.clone(),
            },
        })
    }
}

/// `[extensions.<id>]` section in `gestalt.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalExtensionConfig {
    /// Executable and arguments to spawn the worker process.
    pub command: Vec<String>,
    /// IPC protocol (`"rpc"` or `"mcp"`).
    #[serde(default)]
    pub protocol: ExtensionProtocol,
    /// Per-call IPC timeout in milliseconds (defaults to `5000`).
    #[serde(default = "default_ext_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_trigger_enabled() -> bool {
    true
}

/// `[triggers.<id>]` section in `gestalt.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Trigger kind (`"interval"`, `"cron_schedule"`, `"fs_watch"`, `"webhook"`).
    pub kind: String,
    /// Whether this trigger starts active at boot (defaults to `true`).
    #[serde(default = "default_trigger_enabled")]
    pub enabled: bool,
    /// Prompt or JSON input dispatched when the trigger fires.
    pub prompt: Value,
    /// Optional target preset or loop ID.
    #[serde(default)]
    pub preset: Option<String>,
    /// Optional extra decorators applied to triggered Turns.
    #[serde(default)]
    pub extra_decorators: Vec<String>,
    /// Kind-specific trigger configuration parameters.
    #[serde(default)]
    pub config: Value,
}

/// Parsed `gestalt.toml` configuration file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GestaltConfig {
    /// Session and preset defaults.
    #[serde(default)]
    pub defaults: DefaultsConfig,
    /// Optional resource and cost ceilings.
    pub budget: Option<BudgetConfig>,
    /// Optional sandbox restrictions.
    pub sandbox: Option<SandboxConfig>,
    /// Policy rules and headless approval settings.
    #[serde(default)]
    pub policy: PolicyConfig,
    /// Provider endpoint, model, and rate table configurations.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Named preset compositions.
    #[serde(default)]
    pub presets: BTreeMap<String, PresetConfig>,
    /// Out-of-process stdio JSON-RPC / MCP extensions.
    #[serde(default)]
    pub extensions: BTreeMap<String, ExternalExtensionConfig>,
    /// Configured event and schedule triggers.
    #[serde(default)]
    pub triggers: BTreeMap<String, TriggerConfig>,
}

impl GestaltConfig {
    /// Parse a [`GestaltConfig`] from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::TomlDe`] on syntax or schema errors.
    pub fn from_toml_str(raw: &str) -> Result<Self, RuntimeError> {
        Ok(toml::from_str(raw)?)
    }

    /// Build a [`PolicyOverride`] from this config layer, inheriting omitted
    /// budget/sandbox fields from `current_budget` and `current_sandbox`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `budget.max_cost_usd` is invalid.
    pub fn to_policy_override(
        &self,
        current_budget: &BudgetCap,
        current_sandbox: &SandboxPolicy,
    ) -> Result<PolicyOverride, RuntimeError> {
        let budget = match &self.budget {
            Some(b) => {
                let cap = b.to_budget_cap()?;
                Some(BudgetCap {
                    max_steps: cap.max_steps.or(current_budget.max_steps),
                    max_tokens: cap.max_tokens.or(current_budget.max_tokens),
                    max_cost_micro_usd: cap
                        .max_cost_micro_usd
                        .or(current_budget.max_cost_micro_usd),
                })
            }
            None => None,
        };
        let allowed_tools = self
            .policy
            .allowed_tools
            .as_ref()
            .map(|list| list.iter().cloned().collect::<BTreeSet<String>>());
        let sandbox = self.sandbox.as_ref().map(|sb| SandboxPolicy {
            network: sb.network.unwrap_or(current_sandbox.network),
            protected_paths: sb
                .protected_paths
                .as_ref()
                .map(|paths| paths.iter().cloned().collect())
                .unwrap_or_default(),
            max_spawn_depth: sb
                .max_spawn_depth
                .unwrap_or(current_sandbox.max_spawn_depth),
        });
        Ok(PolicyOverride {
            budget,
            allowed_tools,
            sandbox,
        })
    }
}

/// Persistent workspace trust allowlist stored in
/// `~/.config/gestalt/trusted_workspaces.json`, keyed by
/// `(canonical_workspace_path, sha256(gestalt.toml))`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceTrustStore {
    #[serde(skip)]
    file_path: Option<PathBuf>,
    #[serde(default)]
    trusted: BTreeMap<String, String>,
}

impl WorkspaceTrustStore {
    /// Load the trust store from `~/.config/gestalt/trusted_workspaces.json`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if reading or parsing an existing trust file fails.
    pub fn load_default() -> Result<Self, RuntimeError> {
        let path = CredentialStore::resolve_config_dir().map(|d| d.join("trusted_workspaces.json"));
        match path {
            Some(p) => Self::load_from_path(p),
            None => Ok(Self::default()),
        }
    }

    /// Load a [`WorkspaceTrustStore`] from an explicit JSON file path.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if reading or JSON parsing fails.
    pub fn load_from_path(path: PathBuf) -> Result<Self, RuntimeError> {
        if !path.exists() {
            return Ok(Self {
                file_path: Some(path),
                trusted: BTreeMap::new(),
            });
        }
        let raw = fs::read_to_string(&path)?;
        let trusted: BTreeMap<String, String> = serde_json::from_str(&raw)?;
        Ok(Self {
            file_path: Some(path),
            trusted,
        })
    }

    /// Check whether `workspace_root`'s `.gestalt/gestalt.toml` bytes match a
    /// previously approved SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Io`] if `workspace_root` cannot be canonicalized.
    pub fn is_trusted(
        &self,
        workspace_root: &Path,
        config_bytes: &[u8],
    ) -> Result<bool, RuntimeError> {
        let canon = fs::canonicalize(workspace_root)?;
        let key = canon.to_string_lossy().to_string();
        let digest = sha256_hex(config_bytes);
        Ok(self.trusted.get(&key).is_some_and(|saved| saved == &digest))
    }

    /// Record explicit trust approval for `(canonical_workspace_path, sha256(config_bytes))`
    /// and persist the updated allowlist to disk.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if canonicalization or persistence fails.
    pub fn trust_workspace(
        &mut self,
        workspace_root: &Path,
        config_bytes: &[u8],
    ) -> Result<String, RuntimeError> {
        let canon = fs::canonicalize(workspace_root)?;
        let key = canon.to_string_lossy().to_string();
        let digest = sha256_hex(config_bytes);
        self.trusted.insert(key, digest.clone());
        if let Some(ref path) = self.file_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let encoded = serde_json::to_string_pretty(&self.trusted)?;
            fs::write(path, encoded)?;
        }
        Ok(digest)
    }
}

/// Merged runtime configuration produced by [`load_and_merge_config`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRuntimeConfig {
    /// Active default preset name.
    pub default_preset: String,
    /// Relative or absolute session directory path.
    pub session_dir: String,
    /// Pause reasons auto-approved in `-p` headless mode.
    pub headless_auto_approve: Vec<String>,
    /// Effective sandbox driver tier (`os`, `docker`, `microvm`).
    pub sandbox_driver: SandboxKind,
    /// Optional Docker sandbox configuration.
    pub docker_sandbox: Option<DockerSandboxConfig>,
    /// Optional Micro-VM sandbox configuration.
    pub microvm_sandbox: Option<MicroVmSandboxConfig>,
    /// Merged provider configurations keyed by provider/alias ID.
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Merged preset configurations keyed by preset ID.
    pub presets: BTreeMap<String, Preset>,
    /// Merged external IPC/MCP extension definitions keyed by extension name.
    pub extensions: BTreeMap<String, ExternalExtensionConfig>,
    /// Merged trigger definitions keyed by trigger ID.
    pub triggers: BTreeMap<String, TriggerConfig>,
}

/// Input options for [`load_and_merge_config`].
pub struct ConfigLoadOptions<'a> {
    /// Workspace root directory.
    pub workspace_root: &'a Path,
    /// Optional global `~/.config/gestalt/gestalt.toml` path.
    pub global_toml_path: Option<&'a Path>,
    /// Optional workspace `<workspace>/.gestalt/gestalt.toml` path.
    pub workspace_toml_path: Option<&'a Path>,
    /// Ad-hoc CLI `--ext` definitions overriding earlier tiers.
    pub cli_extensions: BTreeMap<String, ExternalExtensionConfig>,
    /// Persistent workspace trust allowlist.
    pub trust_store: &'a WorkspaceTrustStore,
    /// `true` when `--trust-workspace` is explicitly passed in headless CI.
    pub auto_trust_workspace: bool,
}

/// Discover and merge Global (`~/.config/gestalt/gestalt.toml`), Workspace
/// (`<workspace_root>/.gestalt/gestalt.toml`), and CLI `--ext` configurations
/// with monotonic `PolicyGate` narrowing and workspace trust enforcement.
///
/// # Errors
///
/// Returns [`RuntimeError::UntrustedWorkspace`] if a workspace config exists
/// without matching trust approval, or [`RuntimeError::Kernel`] if any tier
/// attempts to relax a baseline policy restriction.
pub fn load_and_merge_config(
    opts: ConfigLoadOptions<'_>,
    policy_gate: &mut PolicyGate,
    registry: &mut ExtensionRegistry,
) -> Result<ResolvedRuntimeConfig, RuntimeError> {
    let mut resolved = ResolvedRuntimeConfig {
        default_preset: "general".to_string(),
        session_dir: ".gestalt/sessions".to_string(),
        headless_auto_approve: Vec::new(),
        sandbox_driver: SandboxKind::Os,
        docker_sandbox: None,
        microvm_sandbox: None,
        providers: BTreeMap::new(),
        presets: registry.presets.clone(),
        extensions: BTreeMap::new(),
        triggers: BTreeMap::new(),
    };

    if let Some(global_path) = opts.global_toml_path
        && global_path.exists()
    {
        let raw = fs::read_to_string(global_path)?;
        let global_cfg = GestaltConfig::from_toml_str(&raw)?;
        apply_tier(&global_cfg, &mut resolved, policy_gate, registry)?;
    }

    if let Some(ws_path) = opts.workspace_toml_path
        && ws_path.exists()
    {
        let raw_bytes = fs::read(ws_path)?;
        if !opts.auto_trust_workspace
            && !opts
                .trust_store
                .is_trusted(opts.workspace_root, &raw_bytes)?
        {
            let canon = fs::canonicalize(opts.workspace_root)
                .unwrap_or_else(|_| opts.workspace_root.to_path_buf());
            return Err(RuntimeError::UntrustedWorkspace {
                workspace_root: canon.to_string_lossy().to_string(),
                config_digest: sha256_hex(&raw_bytes),
            });
        }
        let raw_str = String::from_utf8(raw_bytes).map_err(|err| {
            RuntimeError::ArtifactError(format!("workspace gestalt.toml is not UTF-8: {err}"))
        })?;
        let ws_cfg = GestaltConfig::from_toml_str(&raw_str)?;
        apply_tier(&ws_cfg, &mut resolved, policy_gate, registry)?;
    }

    resolved.extensions.extend(opts.cli_extensions);
    Ok(resolved)
}

fn apply_tier(
    cfg: &GestaltConfig,
    resolved: &mut ResolvedRuntimeConfig,
    policy_gate: &mut PolicyGate,
    registry: &mut ExtensionRegistry,
) -> Result<(), RuntimeError> {
    let override_cfg =
        cfg.to_policy_override(&policy_gate.baseline_budget(), policy_gate.sandbox())?;
    policy_gate.apply_monotonic_override(&override_cfg)?;

    if let Some(ref sb) = cfg.sandbox {
        if let Some(driver) = sb.driver {
            if driver.isolation_rank() < resolved.sandbox_driver.isolation_rank() {
                return Err(RuntimeError::Kernel(
                    KernelError::PolicyRelaxationForbidden(format!(
                        "sandbox driver cannot be downgraded from '{:?}' to '{:?}'",
                        resolved.sandbox_driver, driver
                    )),
                ));
            }
            resolved.sandbox_driver = driver;
        }
        if let Some(ref docker) = sb.docker {
            resolved.docker_sandbox = Some(docker.clone());
        }
        if let Some(ref microvm) = sb.microvm {
            resolved.microvm_sandbox = Some(microvm.clone());
        }
    }

    if let Some(ref preset) = cfg.defaults.preset {
        resolved.default_preset = preset.clone();
    }
    if let Some(ref session_dir) = cfg.defaults.session_dir {
        resolved.session_dir = session_dir.clone();
    }
    if !cfg.policy.headless_auto_approve.is_empty() {
        resolved.headless_auto_approve = cfg.policy.headless_auto_approve.clone();
    }
    resolved.providers.extend(cfg.providers.clone());
    for (id, preset_cfg) in &cfg.presets {
        let preset = preset_cfg.to_preset()?;
        resolved.presets.insert(id.clone(), preset.clone());
        registry.presets.insert(id.clone(), preset);
    }
    resolved.extensions.extend(cfg.extensions.clone());
    resolved.triggers.extend(cfg.triggers.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::KernelError;

    #[test]
    fn parses_reference_gestalt_toml_schema() {
        let raw = r#"
[defaults]
preset = "swe"
session_dir = ".gestalt/sessions"

[budget]
max_steps = 100
max_tokens = 250000
max_cost_usd = 5.0

[sandbox]
network = false
protected_paths = [".gestalt/memories/", ".gestalt/skills/"]

[policy]
headless_auto_approve = []

[providers.mock_llm]
adapter = "mock_adapter"
model = "mock-sonnet"
input_cost_per_mtok_usd = 3.0
output_cost_per_mtok_usd = 15.0

[presets.custom_swe]
base_loop = "chat"
decorators = ["compaction"]
builder = "standard"
allowed_tools = ["read", "write", "edit"]
max_spawn_depth = 1

[extensions.my_router]
command = ["python3", "./ext/router.py"]
protocol = "rpc"
timeout_ms = 5000
"#;
        let cfg = GestaltConfig::from_toml_str(raw).expect("parse reference toml");
        let cap = cfg
            .budget
            .as_ref()
            .expect("budget")
            .to_budget_cap()
            .expect("budget cap");
        assert_eq!(cap.max_cost_micro_usd, Some(5_000_000));
        assert_eq!(
            cfg.providers["mock_llm"].adapter.as_deref(),
            Some("mock_adapter")
        );
        assert_eq!(cfg.extensions["my_router"].protocol, ExtensionProtocol::Rpc);
        assert_eq!(cfg.presets["custom_swe"].decorators, vec!["compaction"]);
    }

    #[test]
    fn monotonic_merge_and_workspace_trust_gate_enforced() {
        let dir = std::env::temp_dir().join(format!("gestalt_cfg_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let canon_dir = fs::canonicalize(&dir).expect("canon");

        let global_toml = canon_dir.join("global.toml");
        fs::write(
            &global_toml,
            "[budget]\nmax_steps = 50\nmax_cost_usd = 5.0\n\n[sandbox]\nnetwork = false\nprotected_paths = [\".gestalt/protected/\"]\n",
        )
        .expect("write global");

        let ws_toml = canon_dir.join("workspace.toml");
        fs::write(
            &ws_toml,
            "[budget]\nmax_cost_usd = 2.0\n\n[sandbox]\nprotected_paths = [\".gestalt/extra/\"]\n",
        )
        .expect("write ws");

        let trust_path = canon_dir.join("trusted.json");
        let mut trust = WorkspaceTrustStore::load_from_path(trust_path).expect("trust store");
        let mut gate = PolicyGate::default();
        let mut reg = ExtensionRegistry::new();

        let err = load_and_merge_config(
            ConfigLoadOptions {
                workspace_root: &canon_dir,
                global_toml_path: Some(&global_toml),
                workspace_toml_path: Some(&ws_toml),
                cli_extensions: BTreeMap::new(),
                trust_store: &trust,
                auto_trust_workspace: false,
            },
            &mut gate,
            &mut reg,
        )
        .expect_err("must reject untrusted workspace");
        assert!(matches!(err, RuntimeError::UntrustedWorkspace { .. }));

        let ws_bytes = fs::read(&ws_toml).expect("read ws");
        trust
            .trust_workspace(&canon_dir, &ws_bytes)
            .expect("trust ws");

        let mut gate_ok = PolicyGate::default();
        let mut reg_ok = ExtensionRegistry::new();
        load_and_merge_config(
            ConfigLoadOptions {
                workspace_root: &canon_dir,
                global_toml_path: Some(&global_toml),
                workspace_toml_path: Some(&ws_toml),
                cli_extensions: BTreeMap::new(),
                trust_store: &trust,
                auto_trust_workspace: false,
            },
            &mut gate_ok,
            &mut reg_ok,
        )
        .expect("trusted merge succeeds");
        assert_eq!(
            gate_ok.baseline_budget().max_cost_micro_usd,
            Some(2_000_000)
        );
        assert!(
            gate_ok
                .sandbox()
                .protected_paths
                .contains(".gestalt/extra/")
        );

        fs::write(&ws_toml, "[sandbox]\nnetwork = true\n").expect("write relaxed ws");
        let relaxed_bytes = fs::read(&ws_toml).expect("read relaxed");
        trust
            .trust_workspace(&canon_dir, &relaxed_bytes)
            .expect("trust relaxed");
        let mut gate_relax = PolicyGate::default();
        let mut reg_relax = ExtensionRegistry::new();
        let relax_err = load_and_merge_config(
            ConfigLoadOptions {
                workspace_root: &canon_dir,
                global_toml_path: Some(&global_toml),
                workspace_toml_path: Some(&ws_toml),
                cli_extensions: BTreeMap::new(),
                trust_store: &trust,
                auto_trust_workspace: false,
            },
            &mut gate_relax,
            &mut reg_relax,
        )
        .expect_err("must reject network relaxation");
        assert!(matches!(
            relax_err,
            RuntimeError::Kernel(KernelError::PolicyRelaxationForbidden(_))
        ));

        let _ = fs::remove_dir_all(&dir);
    }
}
