#![forbid(unsafe_code)]
//! Extension-registered slash command handlers (`/goal`, `/compact`, `/gate`, `/login`).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use gestalt_kernel::{KernelRuntime, PauseReason};
use gestalt_runtime::{
    CommandHandler, CredentialRecord, CredentialStore, RuntimeError, current_epoch_ms,
};
use serde_json::{Value, json};

/// `/goal <criteria>` handler returning `goal_check` activation metadata and
/// recording an audit `custom:std:goal_set` node on `SessionTree`.
#[derive(Debug, Default)]
pub struct GoalCommandHandler;

impl CommandHandler for GoalCommandHandler {
    fn execute<'a>(
        &'a self,
        args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let criteria = args.trim().to_string();
        Box::pin(async move {
            if criteria.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /goal <criteria>".to_string(),
                ));
            }
            let node_id = kernel.append_custom_node(
                "std:goal_set".to_string(),
                json!({ "criteria": criteria }),
                current_epoch_ms(),
            )?;
            Ok(json!({
                "command": "goal",
                "node_id": node_id,
                "extra_decorators": ["goal_check"],
                "params_patch": { "goal_criteria": criteria },
            }))
        })
    }
}

/// `/compact` handler appending a `custom:std:compact_requested` marker so
/// `CompactionDecorator` compacts history on the next Step.
#[derive(Debug, Default)]
pub struct CompactCommandHandler;

impl CommandHandler for CompactCommandHandler {
    fn execute<'a>(
        &'a self,
        _args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let node_id = kernel.append_custom_node(
                "std:compact_requested".to_string(),
                json!({ "requested": true }),
                current_epoch_ms(),
            )?;
            Ok(json!({
                "command": "compact",
                "trigger_compaction": true,
                "node_id": node_id,
            }))
        })
    }
}

/// `/gate` handler inspecting active `PolicyGate` rules, `SandboxPolicy`, and
/// `BudgetSnapshot`.
#[derive(Debug, Default)]
pub struct GateCommandHandler;

impl CommandHandler for GateCommandHandler {
    fn execute<'a>(
        &'a self,
        _args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let gate = kernel.policy_gate();
            let snap = kernel.budget_snapshot();
            Ok(json!({
                "command": "gate",
                "baseline_budget": gate.baseline_budget(),
                "budget_snapshot": snap,
                "baseline_allowed_tools": gate.baseline_allowed_tools(),
                "sandbox": gate.sandbox(),
                "rule_ids": gate.rule_ids(),
            }))
        })
    }
}

/// `/login <provider>` handler emitting a `PauseReason::Input` dialog spec so
/// interactive surfaces prompt for a secret without logging it to `SessionTree`.
#[derive(Debug, Clone)]
pub struct LoginCommandHandler {
    credentials: Arc<Mutex<CredentialStore>>,
}

impl LoginCommandHandler {
    /// Construct a [`LoginCommandHandler`] backed by `credentials`.
    #[must_use]
    pub fn new(credentials: Arc<Mutex<CredentialStore>>) -> Self {
        Self { credentials }
    }
}

impl CommandHandler for LoginCommandHandler {
    fn execute<'a>(
        &'a self,
        args: &'a str,
        _kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let provider = args.trim().to_string();
        Box::pin(async move {
            if provider.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /login <provider>".to_string(),
                ));
            }
            let has_existing = self
                .credentials
                .lock()
                .ok()
                .and_then(|c| c.get_record(&provider, None))
                .is_some();

            Ok(json!({
                "command": "login",
                "provider": provider,
                "already_configured": has_existing,
                "pause_request": {
                    "reason": PauseReason::Input,
                    "payload": {
                        "dialog": "provider_login",
                        "provider": provider,
                        "secret": true,
                        "prompt": format!("Enter API key or token for '{provider}'"),
                    }
                }
            }))
        })
    }
}

/// Persist a resolved `provider_login` dialog response into [`CredentialStore`]
/// (`0600` `credentials.toml`) and return strictly non-secret confirmation JSON.
///
/// # Errors
///
/// Returns [`RuntimeError`] if the resolution lacks a non-empty secret or file
/// persistence fails.
pub fn apply_login_resolution(
    store: &mut CredentialStore,
    provider: &str,
    resolution: &Value,
) -> Result<Value, RuntimeError> {
    if let Ok(record) = serde_json::from_value::<CredentialRecord>(resolution.clone()) {
        store.set_record(provider, record)?;
        return Ok(json!({
            "provider": provider,
            "stored": true,
            "kind": "structured",
        }));
    }

    let secret = resolution
        .get("secret")
        .and_then(Value::as_str)
        .or_else(|| resolution.get("api_key").and_then(Value::as_str))
        .or_else(|| resolution.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            RuntimeError::ArtifactError("login resolution missing non-empty 'secret'".to_string())
        })?;

    store.set_credential(provider, secret.to_string())?;
    Ok(json!({
        "provider": provider,
        "stored": true,
        "kind": "api_key",
    }))
}

/// Handler for `/model [<provider/model>]` — inspect or switch active LLM target.
pub struct ModelCommandHandler;

impl CommandHandler for ModelCommandHandler {
    fn execute<'a>(
        &'a self,
        args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let trimmed = args.trim().to_string();
        Box::pin(async move {
            if trimmed.is_empty() {
                let current_model = kernel
                    .session_tree()
                    .all_nodes()
                    .iter()
                    .rev()
                    .find_map(|n| match n.decode_payload().ok()? {
                        gestalt_kernel::NodePayload::PhaseOpen { config, .. } => config
                            .params
                            .get("model")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        _ => None,
                    });
                let msg = match &current_model {
                    Some(m) => format!("Active model: {m}. Switch via /model <provider/model>."),
                    None => "No explicit model override set (using default). Switch via /model <provider/model>.".to_string(),
                };
                return Ok(json!({
                    "command": "model",
                    "current_model": current_model,
                    "usage": "/model <provider/model>",
                    "message": msg,
                }));
            }
            let audit_id = kernel.append_custom_node(
                "std:model_set".to_string(),
                json!({ "model": trimmed }),
                current_epoch_ms(),
            )?;
            Ok(json!({
                "command": "model",
                "model": trimmed,
                "audit_node_id": audit_id,
                "message": format!("Switched active model to '{trimmed}'."),
                "params_patch": {
                    "model": trimmed,
                },
            }))
        })
    }
}

/// Handler for `/thinking [<off|low|medium|high|tokens>]` — configure extended reasoning budget.
pub struct ThinkingCommandHandler;

impl CommandHandler for ThinkingCommandHandler {
    fn execute<'a>(
        &'a self,
        args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let trimmed = args.trim().to_string();
        Box::pin(async move {
            if trimmed.is_empty() {
                return Ok(json!({
                    "command": "thinking",
                    "usage": "/thinking <off|low|medium|high|tokens>",
                    "message": "Usage: /thinking <off|low|medium|high|tokens>",
                }));
            }
            let budget: Option<u64> = match trimmed.to_ascii_lowercase().as_str() {
                "off" | "none" | "0" => None,
                "low" => Some(1024),
                "medium" | "med" => Some(4096),
                "high" => Some(16_384),
                other => {
                    let parsed = other.parse::<u64>().map_err(|_| {
                        RuntimeError::ArtifactError(
                            "usage: /thinking <off|low|medium|high|tokens>".to_string(),
                        )
                    })?;
                    Some(parsed)
                }
            };
            let audit_id = kernel.append_custom_node(
                "std:thinking_set".to_string(),
                json!({ "reasoning_budget_tokens": budget }),
                current_epoch_ms(),
            )?;
            let msg = match budget {
                Some(tokens) => format!("Extended thinking budget set to {tokens} tokens."),
                None => "Extended thinking disabled.".to_string(),
            };
            Ok(json!({
                "command": "thinking",
                "reasoning_budget_tokens": budget,
                "audit_node_id": audit_id,
                "message": msg,
                "params_patch": {
                    "reasoning_budget_tokens": budget,
                },
            }))
        })
    }
}

/// Handler for `/logout <provider>` — remove stored credentials from `CredentialStore`.
pub struct LogoutCommandHandler {
    credentials: Arc<Mutex<CredentialStore>>,
}

impl LogoutCommandHandler {
    /// Construct a [`LogoutCommandHandler`] bound to `credentials`.
    #[must_use]
    pub fn new(credentials: Arc<Mutex<CredentialStore>>) -> Self {
        Self { credentials }
    }
}

impl CommandHandler for LogoutCommandHandler {
    fn execute<'a>(
        &'a self,
        args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let provider = args.trim().to_string();
        Box::pin(async move {
            if provider.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /logout <provider>".to_string(),
                ));
            }
            let removed = {
                let mut guard = self
                    .credentials
                    .lock()
                    .map_err(|_| RuntimeError::ArtifactError("credential lock poisoned".into()))?;
                guard.remove_credential(&provider)?
            };
            let audit_id = kernel.append_custom_node(
                "std:logout".to_string(),
                json!({ "provider": provider, "removed": removed }),
                current_epoch_ms(),
            )?;
            let msg = if removed {
                format!("Removed stored credentials for '{provider}'.")
            } else {
                format!("No stored credentials found for '{provider}'.")
            };
            Ok(json!({
                "command": "logout",
                "provider": provider,
                "removed": removed,
                "audit_node_id": audit_id,
                "message": msg,
            }))
        })
    }
}

/// Handler for `/skills` and `/skill:<name> [args]`.
pub struct SkillsCommandHandler;

impl CommandHandler for SkillsCommandHandler {
    fn execute<'a>(
        &'a self,
        args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let trimmed = args.trim().to_string();
        Box::pin(async move {
            let ws_root = kernel
                .session_tree()
                .all_nodes()
                .iter()
                .find_map(|n| match n.decode_payload().ok()? {
                    gestalt_kernel::NodePayload::SessionOpen { workspace_root, .. } => {
                        Some(std::path::PathBuf::from(workspace_root))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| std::path::PathBuf::from("."));

            let skill_paths = crate::extensions::builders::discover_skill_paths(&ws_root);
            let mut entries = std::collections::BTreeMap::new();
            for path in skill_paths {
                let full_path = if std::path::Path::new(&path).is_absolute() {
                    std::path::PathBuf::from(&path)
                } else {
                    ws_root.join(&path)
                };
                if let Ok(content) = std::fs::read_to_string(&full_path)
                    && let Some(parsed) =
                        crate::extensions::builders::parse_skill_frontmatter(&path, &content)
                {
                    entries.insert(parsed.name.clone(), (parsed, content));
                }
            }

            let trimmed = trimmed.as_str();
            if let Some(target_rest) = trimmed.strip_prefix("load:") {
                let (skill_name, extra_args) = match target_rest.split_once(char::is_whitespace) {
                    Some((n, a)) => (n.trim(), a.trim()),
                    None => (target_rest.trim(), ""),
                };
                let Some((meta, body)) = entries.get(skill_name) else {
                    return Err(RuntimeError::ArtifactError(format!(
                        "unknown skill '{skill_name}'"
                    )));
                };
                let steering = if extra_args.is_empty() {
                    format!("Apply skill '{}' ({}):\n\n{}", meta.name, meta.path, body)
                } else {
                    format!(
                        "Apply skill '{}' ({}) for task '{}':\n\n{}",
                        meta.name, meta.path, extra_args, body
                    )
                };
                return Ok(json!({
                    "command": "skills",
                    "loaded_skill": meta.name,
                    "path": meta.path,
                    "steering": steering,
                    "message": format!("Loaded skill '{}' ({}).", meta.name, meta.path),
                }));
            }

            let summary_lines: Vec<String> = entries
                .values()
                .map(|(meta, _)| {
                    if meta.description.is_empty() {
                        format!("/skill:{} ({})", meta.name, meta.path)
                    } else {
                        format!("/skill:{} - {}", meta.name, meta.description)
                    }
                })
                .collect();
            let msg = if summary_lines.is_empty() {
                "No skills discovered in ~/.agents/skills, ~/.gestalt/skills, .agents/skills, or .gestalt/skills.".to_string()
            } else {
                format!(
                    "Skills ({}): {}",
                    summary_lines.len(),
                    summary_lines.join(" | ")
                )
            };
            let list: Vec<Value> = entries
                .into_values()
                .map(|(meta, _)| {
                    json!({
                        "name": meta.name,
                        "path": meta.path,
                        "description": meta.description,
                        "triggers": meta.triggers,
                        "disable_model_invocation": meta.disable_model_invocation,
                    })
                })
                .collect();
            Ok(json!({
                "command": "skills",
                "count": list.len(),
                "skills": list,
                "message": msg,
            }))
        })
    }
}

/// Register all `gestalt-std` slash command handlers into [`gestalt_runtime::RuntimeRegistry`].
pub fn register_std_commands(
    runtime: &mut gestalt_runtime::RuntimeRegistry,
    credentials: Arc<Mutex<CredentialStore>>,
) {
    runtime
        .commands
        .insert("model".to_string(), Arc::new(ModelCommandHandler));
    runtime
        .commands
        .insert("thinking".to_string(), Arc::new(ThinkingCommandHandler));
    runtime
        .commands
        .insert("goal".to_string(), Arc::new(GoalCommandHandler));
    runtime
        .commands
        .insert("compact".to_string(), Arc::new(CompactCommandHandler));
    runtime
        .commands
        .insert("gate".to_string(), Arc::new(GateCommandHandler));
    runtime.commands.insert(
        "login".to_string(),
        Arc::new(LoginCommandHandler::new(credentials.clone())),
    );
    runtime.commands.insert(
        "logout".to_string(),
        Arc::new(LogoutCommandHandler::new(credentials)),
    );
    runtime
        .commands
        .insert("skills".to_string(), Arc::new(SkillsCommandHandler));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::StdExtension;
    use gestalt_kernel::{BudgetCap, Extension, ExtensionRegistry, PolicyGate, SandboxPolicy};
    use std::fs;

    #[tokio::test]
    async fn std_commands_gate_goal_compact_and_login_work() {
        let dir = std::env::temp_dir().join(format!("gestalt_cmd_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        let mut reg = ExtensionRegistry::new();
        StdExtension.register(&mut reg).expect("reg");
        let gate = PolicyGate::new(
            BudgetCap {
                max_steps: Some(10),
                max_tokens: Some(5_000),
                max_cost_micro_usd: Some(1_000_000),
            },
            None,
            SandboxPolicy::default(),
        );
        let mut kernel =
            KernelRuntime::new("r_cmd".into(), "/repo".into(), 10, reg, gate).expect("kernel");

        let gate_out = GateCommandHandler
            .execute("", &mut kernel)
            .await
            .expect("gate");
        assert_eq!(gate_out["baseline_budget"]["max_steps"], 10);

        let goal_out = GoalCommandHandler
            .execute("cargo test passes", &mut kernel)
            .await
            .expect("goal");
        assert_eq!(
            goal_out["params_patch"]["goal_criteria"],
            "cargo test passes"
        );

        let compact_out = CompactCommandHandler
            .execute("", &mut kernel)
            .await
            .expect("compact");
        assert_eq!(compact_out["trigger_compaction"], true);

        let cred_path = dir.join("credentials.toml");
        let mut store = CredentialStore::load_from_path(cred_path.clone()).expect("store");
        let res = apply_login_resolution(
            &mut store,
            "mock_provider",
            &json!({ "secret": "sk-mock-super-secret-value" }),
        )
        .expect("apply login");
        assert_eq!(res["stored"], true);
        assert!(!res.to_string().contains("sk-mock-super-secret-value"));

        let reloaded = CredentialStore::load_from_path(cred_path).expect("reload 0600");
        assert_eq!(
            reloaded.get("mock_provider", None).as_deref(),
            Some("sk-mock-super-secret-value")
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
