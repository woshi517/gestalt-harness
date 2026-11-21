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

/// Register all 4 `gestalt-std` slash command handlers into [`gestalt_runtime::RuntimeRegistry`].
pub fn register_std_commands(
    runtime: &mut gestalt_runtime::RuntimeRegistry,
    credentials: Arc<Mutex<CredentialStore>>,
) {
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
        Arc::new(LoginCommandHandler::new(credentials)),
    );
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
