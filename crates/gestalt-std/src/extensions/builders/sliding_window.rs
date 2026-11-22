#![forbid(unsafe_code)]
//! Lean sliding-window [`ContextBuilder`] for `minimal_shell`.

use std::collections::BTreeMap;

use gestalt_kernel::{ArtifactStoreView, ContextBuilder, ModelCapabilities, SessionSlice};
use serde_json::Value;

use crate::extensions::builders::{
    inspect_slice_metadata, project_messages_from_slice, resolve_sorted_tool_defs,
};
use crate::extensions::{ChatRole, ContentBlock, LlmInput, LlmToolDefinition, PromptTier};

const DEFAULT_SHELL_SYSTEM_PROMPT: &str = "You are Gestalt running in minimal_shell mode. Execute concise shell commands to solve the task.";

const DEFAULT_WINDOW_STEPS: usize = 12;

/// Lean [`ContextBuilder`] (`"sliding_window"`) omitting `MEMORY.md` / `SKILL.md`
/// and retaining the initial prompt plus the most recent `window_steps` messages
/// without splitting tool-call/result pairs.
#[derive(Debug, Clone)]
pub struct SlidingWindowBuilder {
    system_prompt: String,
    default_tools: BTreeMap<String, LlmToolDefinition>,
}

impl Default for SlidingWindowBuilder {
    fn default() -> Self {
        Self::new(DEFAULT_SHELL_SYSTEM_PROMPT.to_string(), BTreeMap::new())
    }
}

impl SlidingWindowBuilder {
    /// Construct a [`SlidingWindowBuilder`] with custom instructions and tool definitions.
    #[must_use]
    pub fn new(system_prompt: String, default_tools: BTreeMap<String, LlmToolDefinition>) -> Self {
        Self {
            system_prompt,
            default_tools,
        }
    }
}

impl ContextBuilder for SlidingWindowBuilder {
    fn id(&self) -> &str {
        "sliding_window"
    }

    fn digest(&self) -> &str {
        "std_sliding_window_builder_v1"
    }

    fn build(
        &self,
        session: &SessionSlice,
        _artifacts: &ArtifactStoreView,
        caps: &ModelCapabilities,
    ) -> Value {
        let meta = inspect_slice_metadata(session);
        let cfg_ref = meta.active_config.as_ref();
        let window_steps = cfg_ref
            .and_then(|c| c.params.get("window_steps"))
            .and_then(Value::as_u64)
            .map_or(DEFAULT_WINDOW_STEPS, |n| n as usize)
            .max(1);

        let all_messages = project_messages_from_slice(session, &meta);
        let messages = if all_messages.len() <= window_steps + 1 {
            all_messages
        } else {
            let mut kept = vec![all_messages[0].clone()];
            let mut start = all_messages.len().saturating_sub(window_steps);
            // Never slice right onto a User message containing ToolResult without its preceding Assistant ToolUse.
            while start > 1 && start < all_messages.len() {
                let is_tool_result_msg = all_messages[start].role == ChatRole::User
                    && all_messages[start]
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
                if is_tool_result_msg {
                    start -= 1;
                } else {
                    break;
                }
            }
            kept.extend(all_messages[start..].iter().cloned());
            kept
        };

        let input = LlmInput {
            system_tiers: vec![PromptTier {
                name: "stable".to_string(),
                cacheable: caps.supports_prompt_caching,
                content: self.system_prompt.clone(),
            }],
            messages,
            tools: resolve_sorted_tool_defs(&self.default_tools, cfg_ref),
            max_output_tokens: (caps.max_output_tokens > 0).then_some(caps.max_output_tokens),
            reasoning_budget_tokens: None,
        };
        serde_json::to_value(input).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        ArtifactEntry, LoopConfig, NodePayload, SESSION_SCHEMA_VERSION, SessionTree, sha256_hex,
    };
    use serde_json::json;

    #[test]
    fn sliding_window_omits_memory_and_caps_messages() {
        let builder = SlidingWindowBuilder::default();
        let mut tree = SessionTree::new();
        tree.append(
            "n0".into(),
            None,
            None,
            1,
            &NodePayload::SessionOpen {
                schema_version: SESSION_SCHEMA_VERSION,
                workspace_root: "/repo".into(),
                created_at_ms: 1,
            },
        )
        .expect("n0");
        tree.append(
            "n1".into(),
            Some("t0".into()),
            Some("p0".into()),
            2,
            &NodePayload::PhaseOpen {
                phase_id: "p0".into(),
                preset_id: Some("general".into()),
                base_loop_id: "chat".into(),
                loop_digest: "d1".into(),
                decorator_digests: vec![],
                builder_id: "sliding_window".into(),
                builder_digest: builder.digest().into(),
                builder_layer_digests: vec![],
                config: LoopConfig {
                    params: json!({ "window_steps": 2 }),
                    ..LoopConfig::default()
                },
            },
        )
        .expect("n1");
        for i in 0..5 {
            tree.append(
                format!("turn_{i}"),
                Some(format!("t{i}")),
                Some("p0".into()),
                10 + i as u64,
                &NodePayload::TurnStart {
                    turn_id: format!("t{i}"),
                    prompt: json!(format!("prompt {i}")),
                    artifact_snapshots: BTreeMap::new(),
                },
            )
            .expect("turn");
        }

        let mut entries = BTreeMap::new();
        entries.insert(
            ".gestalt/memories/MEMORY.md".into(),
            ArtifactEntry {
                path: ".gestalt/memories/MEMORY.md".into(),
                content: "SHOULD_NOT_APPEAR".into(),
                blob_digest: sha256_hex(b"SHOULD_NOT_APPEAR"),
            },
        );
        let art = ArtifactStoreView::from_entries(entries);
        let val = builder.build(
            &tree.slice_to(None).expect("slice"),
            &art,
            &ModelCapabilities::default(),
        );
        let parsed: LlmInput = serde_json::from_value(val).expect("parse");
        assert_eq!(parsed.system_tiers.len(), 1);
        assert!(!parsed.system_tiers[0].content.contains("SHOULD_NOT_APPEAR"));
        assert_eq!(parsed.messages.len(), 3);
    }
}
