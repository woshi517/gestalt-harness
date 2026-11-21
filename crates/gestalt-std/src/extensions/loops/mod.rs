#![forbid(unsafe_code)]
//! Standard user-space `AgentLoop` implementations (`ChatLoop`, `DecisionRouter`).

pub mod chat;
pub mod decision_router;

use std::collections::BTreeMap;

use gestalt_kernel::{
    ExtensionRegistry, LoopConfig, ModelCapabilities, SideEffectClass, StepResult, ToolSpec,
};
use serde_json::Value;

pub use chat::{ChatLoop, ChatLoopFactory};
pub use decision_router::{DecisionRouter, DecisionRouterFactory};

/// Parse optional `ModelCapabilities` from `LoopConfig::params["model_capabilities"]`
/// with deterministic defaults.
#[must_use]
pub fn parse_model_capabilities(config: &LoopConfig) -> ModelCapabilities {
    config
        .params
        .get("model_capabilities")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(ModelCapabilities {
            max_context_tokens: 200_000,
            max_output_tokens: 8_192,
            supports_prompt_caching: true,
            supports_reasoning: true,
            supports_vision: true,
        })
}

/// Return `true` when `results` represents Phase entry (`None` on Turn start or
/// `Some([StepResult::Switched { .. }])` after `Effect::Switch`).
#[must_use]
pub fn is_phase_entry(results: Option<&[StepResult]>) -> bool {
    match results {
        None => true,
        Some([StepResult::Switched { .. }]) => true,
        Some(_) => false,
    }
}

/// Snapshot Phase-allowed [`ToolSpec`]s from `ExtensionRegistry::tools` in
/// deterministic sorted `BTreeMap` order.
#[must_use]
pub fn capture_allowed_tools(
    config: &LoopConfig,
    registry: &ExtensionRegistry,
) -> BTreeMap<String, ToolSpec> {
    registry
        .tools
        .iter()
        .filter(|(name, _)| {
            config
                .allowed_tools
                .as_ref()
                .is_none_or(|allowed| allowed.iter().any(|a| a == *name))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Look up declared [`SideEffectClass`] for `tool_name`, defaulting to
/// [`SideEffectClass::Irreversible`] for unknown tools so `PolicyGate` fails closed.
#[must_use]
pub fn tool_side_effect(tools: &BTreeMap<String, ToolSpec>, tool_name: &str) -> SideEffectClass {
    tools
        .get(tool_name)
        .map_or(SideEffectClass::Irreversible, |spec| {
            spec.default_side_effect
        })
}

/// Read a string parameter from `LoopConfig::params` or fall back to `default`.
#[must_use]
pub fn param_str<'a>(config: &'a LoopConfig, key: &str, default: &'a str) -> &'a str {
    config
        .params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
}
