#![forbid(unsafe_code)]
//! Generalized presets (`swe`, `general`, `minimal_shell`, `triage`),
//! declarative [`UISurface`] specs, and [`StdExtension`].

use std::collections::BTreeMap;

use gestalt_kernel::{
    CommandSpec, Extension, ExtensionError, ExtensionRegistry, LoopConfig, PauseReason, Preset,
    TriggerSpec, UIActionSpec, UIDialogSpec, UIPanelSpec, UIRendererSpec, UISurface,
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
    renderers.insert(
        "trigger_fired".to_string(),
        UIRendererSpec {
            id: "trigger_fired".to_string(),
            target_kind: "trigger".to_string(),
            title: "Trigger Activated".to_string(),
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
    panels.insert(
        "triggers_inspector".to_string(),
        UIPanelSpec {
            id: "triggers_inspector".to_string(),
            title: "Active Triggers".to_string(),
            position: "right".to_string(),
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
            "general".to_string(),
            Preset {
                base_loop: "chat".to_string(),
                decorators: vec!["compaction".to_string()],
                builder: "standard".to_string(),
                config: LoopConfig {
                    allowed_tools: None,
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
                        "fallback_preset": "general",
                    }),
                },
            },
        );

        for (name, description, usage) in [
            (
                "model",
                "Inspect or switch the active provider/model for the current session",
                "/model [<provider/model>]",
            ),
            (
                "thinking",
                "Configure or toggle extended reasoning token budget",
                "/thinking [<off|low|medium|high|tokens>]",
            ),
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
            (
                "logout",
                "Remove stored credentials for a provider from credentials.toml",
                "/logout <provider>",
            ),
            (
                "skills",
                "List discovered global & workspace Agent Skills or load one via /skill:<name>",
                "/skills",
            ),
            (
                "loop",
                "Create or manage a local interval trigger",
                "/loop <interval> <prompt> | list | stop <id>",
            ),
            (
                "schedule",
                "Create or manage a 5-field cron or cloud schedule trigger",
                "/schedule \"<cron>\" <prompt> | list | stop <id>",
            ),
            (
                "watch",
                "Watch workspace paths for changes and trigger a Turn",
                "/watch <path> <prompt> | stop <id>",
            ),
            (
                "webhook",
                "Start or stop a bounded localhost HTTP webhook trigger",
                "/webhook <addr> <prompt> | stop <id>",
            ),
            (
                "triggers",
                "List, pause, resume, stop, or manually fire active triggers",
                "/triggers [list|pause <id>|resume <id>|stop <id>|fire <id>]",
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

        for (id, kind, description, schema) in [
            (
                "interval",
                "interval",
                "Local fixed-interval timer trigger (/loop)",
                json!({ "type": "object", "properties": { "interval_ms": { "type": "integer" }, "interval": { "type": "string" } } }),
            ),
            (
                "cron_schedule",
                "cron_schedule",
                "5-field POSIX cron and optional cloud schedule trigger (/schedule)",
                json!({ "type": "object", "required": ["expression"], "properties": { "expression": { "type": "string" } } }),
            ),
            (
                "fs_watch",
                "fs_watch",
                "Workspace-confined debounced filesystem watcher (/watch)",
                json!({ "type": "object", "required": ["paths"], "properties": { "paths": { "type": "array" } } }),
            ),
            (
                "webhook",
                "webhook",
                "Bounded localhost HTTP/1.1 webhook listener (/webhook)",
                json!({ "type": "object", "properties": { "bind_addr": { "type": "string" }, "path": { "type": "string" } } }),
            ),
        ] {
            registry.triggers.insert(
                id.to_string(),
                TriggerSpec {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    description: description.to_string(),
                    config_schema: schema,
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

        for preset_name in ["general", "triage"] {
            let stack = reg
                .resolve_phase_stack(preset_name, &[], None)
                .expect("resolve preset");
            assert!(stack.in_process_loop.is_some());
        }
        assert!(!reg.presets.contains_key("swe"));
        assert!(!reg.presets.contains_key("minimal_shell"));
        for cmd in [
            "model", "thinking", "goal", "compact", "gate", "login", "logout", "skills",
        ] {
            assert!(reg.commands.contains_key(cmd), "missing command {cmd}");
        }
        assert!(reg.ui.renderers.contains_key("decision_route"));
        assert!(reg.ui.dialogs.contains_key("provider_login"));
    }
}
