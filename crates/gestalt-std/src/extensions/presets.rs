#![forbid(unsafe_code)]
//! Generalized presets (`swe`, `general`, `minimal_shell`, `triage`),
//! declarative [`UISurface`] specs, and [`StdExtension`].

use std::collections::BTreeMap;

use gestalt_kernel::{
    CommandSpec, Extension, ExtensionError, ExtensionRegistry, LoopConfig, PauseReason, Preset,
    UIActionSpec, UIDialogSpec, UIPanelSpec, UIRendererSpec, UISurface,
};
use serde_json::json;

use crate::extensions::{
    ChatLoopFactory, CompactionDecorator, CuratorDecorator, DecisionRouterFactory,
    DecisionStateBuilder, GoalCheckDecorator, SlidingWindowBuilder, StandardBuilder,
};

/// Build the declarative [`UISurface`] specification registered by [`StdExtension`].
#[must_use]
pub fn std_ui_surface() -> UISurface {
    let mut renderers = BTreeMap::new();
    renderers.insert(
        "decision_route".to_string(),
        UIRendererSpec {
            id: "decision_route".to_string(),
            target_kind: "decision_router".to_string(),
            title: "System One Triage Route".to_string(),
            schema: None,
        },
    );

    let mut panels = BTreeMap::new();
    panels.insert(
        "policy_gate".to_string(),
        UIPanelSpec {
            id: "policy_gate".to_string(),
            title: "PolicyGate & Sandbox".to_string(),
            position: "right".to_string(),
            schema: None,
        },
    );
    panels.insert(
        "budget_meter".to_string(),
        UIPanelSpec {
            id: "budget_meter".to_string(),
            title: "Budget Meter".to_string(),
            position: "bottom".to_string(),
            schema: None,
        },
    );

    let mut dialogs = BTreeMap::new();
    dialogs.insert(
        "curator_approval".to_string(),
        UIDialogSpec {
            id: "curator_approval".to_string(),
            pause_reason: PauseReason::Approval,
            title: "Approve Staged Artifact Promotion".to_string(),
            schema: Some(json!({
                "type": "object",
                "required": ["approved"],
                "properties": { "approved": { "type": "boolean" } }
            })),
        },
    );
    dialogs.insert(
        "provider_login".to_string(),
        UIDialogSpec {
            id: "provider_login".to_string(),
            pause_reason: PauseReason::Input,
            title: "Provider Credential Login".to_string(),
            schema: Some(json!({
                "type": "object",
                "required": ["secret"],
                "properties": { "secret": { "type": "string" } }
            })),
        },
    );

    let mut actions = BTreeMap::new();
    actions.insert(
        "compact".to_string(),
        UIActionSpec {
            id: "compact".to_string(),
            label: "Compact Context".to_string(),
            command_or_signal: "/compact".to_string(),
            shortcut: Some("Ctrl+K".to_string()),
        },
    );
    actions.insert(
        "gate".to_string(),
        UIActionSpec {
            id: "gate".to_string(),
            label: "Inspect PolicyGate".to_string(),
            command_or_signal: "/gate".to_string(),
            shortcut: None,
        },
    );
    actions.insert(
        "goal".to_string(),
        UIActionSpec {
            id: "goal".to_string(),
            label: "Set Goal Criteria".to_string(),
            command_or_signal: "/goal".to_string(),
            shortcut: None,
        },
    );

    UISurface {
        renderers,
        panels,
        dialogs,
        actions,
    }
}

/// Built-in default userland [`Extension`] registering standard loops,
/// decorators, builders, presets (`swe`, `general`, `minimal_shell`, `triage`),
/// slash commands (`/goal`, `/compact`, `/gate`, `/login`), and [`UISurface`].
///
/// Uses the exact same public [`Extension`] contract as third-party crates.
#[derive(Debug, Clone, Default)]
pub struct StdExtension;

impl Extension for StdExtension {
    fn name(&self) -> &str {
        "gestalt-std"
    }

    fn register(&self, registry: &mut ExtensionRegistry) -> Result<(), ExtensionError> {
        registry
            .loops
            .insert("chat".to_string(), Box::new(ChatLoopFactory));
        registry.loops.insert(
            "decision_router".to_string(),
            Box::new(DecisionRouterFactory),
        );

        registry
            .decorators
            .insert("goal_check".to_string(), Box::new(GoalCheckDecorator));
        registry
            .decorators
            .insert("compaction".to_string(), Box::new(CompactionDecorator));
        registry
            .decorators
            .insert("curator".to_string(), Box::new(CuratorDecorator));

        registry
            .builders
            .insert("standard".to_string(), Box::new(StandardBuilder::default()));
        registry.builders.insert(
            "sliding_window".to_string(),
            Box::new(SlidingWindowBuilder::default()),
        );
        registry
            .builders
            .insert("decision_state".to_string(), Box::new(DecisionStateBuilder));

        registry.presets.insert(
            "swe".to_string(),
            Preset {
                base_loop: "chat".to_string(),
                decorators: vec!["compaction".to_string()],
                builder: "standard".to_string(),
                config: LoopConfig {
                    allowed_tools: Some(vec![
                        "read".to_string(),
                        "write".to_string(),
                        "edit".to_string(),
                        "glob".to_string(),
                        "grep".to_string(),
                        "bash".to_string(),
                        "bash_output".to_string(),
                        "bash_kill".to_string(),
                    ]),
                    max_spawn_depth: Some(1),
                    budget: None,
                    params: json!({
                        "compaction_threshold": 0.80,
                        "keep_recent_turns": 2,
                    }),
                },
            },
        );

        registry.presets.insert(
            "general".to_string(),
            Preset {
                base_loop: "chat".to_string(),
                decorators: vec!["compaction".to_string()],
                builder: "standard".to_string(),
                config: LoopConfig {
                    allowed_tools: Some(vec![
                        "read".to_string(),
                        "glob".to_string(),
                        "grep".to_string(),
                    ]),
                    max_spawn_depth: Some(1),
                    budget: None,
                    params: json!({
                        "compaction_threshold": 0.80,
                        "keep_recent_turns": 2,
                    }),
                },
            },
        );

        registry.presets.insert(
            "minimal_shell".to_string(),
            Preset {
                base_loop: "chat".to_string(),
                decorators: Vec::new(),
                builder: "sliding_window".to_string(),
                config: LoopConfig {
                    allowed_tools: Some(vec![
                        "bash".to_string(),
                        "bash_output".to_string(),
                        "bash_kill".to_string(),
                    ]),
                    max_spawn_depth: Some(0),
                    budget: None,
                    params: json!({ "window_steps": 12 }),
                },
            },
        );

        registry.presets.insert(
            "triage".to_string(),
            Preset {
                base_loop: "decision_router".to_string(),
                decorators: Vec::new(),
                builder: "decision_state".to_string(),
                config: LoopConfig {
                    allowed_tools: Some(vec![
                        "read".to_string(),
                        "glob".to_string(),
                        "grep".to_string(),
                        "bash".to_string(),
                    ]),
                    max_spawn_depth: Some(0),
                    budget: None,
                    params: json!({
                        "confidence_threshold": 0.85,
                        "fallback_preset": "swe",
                    }),
                },
            },
        );

        for (name, description, usage) in [
            (
                "goal",
                "Activate goal_check exit criteria for the current session/turn",
                "/goal <criteria>",
            ),
            (
                "compact",
                "Trigger context compaction into a lossless std:compaction node",
                "/compact",
            ),
            (
                "gate",
                "Display active PolicyGate rules, protected_paths, and BudgetSnapshot",
                "/gate",
            ),
            (
                "login",
                "Store provider API key or token into 0600 credentials.toml via Pause(Input)",
                "/login <provider>",
            ),
        ] {
            registry.commands.insert(
                name.to_string(),
                CommandSpec {
                    name: name.to_string(),
                    description: description.to_string(),
                    usage: usage.to_string(),
                    source_extension: None,
                },
            );
        }

        registry.ui.merge(std_ui_surface());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_extension_registers_all_slots_and_resolves_all_presets() {
        let mut reg = ExtensionRegistry::new();
        StdExtension.register(&mut reg).expect("register std");

        for preset_name in ["swe", "general", "minimal_shell", "triage"] {
            let stack = reg
                .resolve_phase_stack(preset_name, &[], None)
                .expect("resolve preset");
            assert!(stack.in_process_loop.is_some());
        }
        assert!(reg.commands.contains_key("goal"));
        assert!(reg.commands.contains_key("compact"));
        assert!(reg.commands.contains_key("gate"));
        assert!(reg.commands.contains_key("login"));
        assert!(reg.ui.renderers.contains_key("decision_route"));
        assert!(reg.ui.dialogs.contains_key("provider_login"));
    }
}
