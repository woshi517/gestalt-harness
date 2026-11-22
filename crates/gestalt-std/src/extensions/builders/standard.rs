#![forbid(unsafe_code)]
//! 3-tier cache-stable [`ContextBuilder`] (`stable` -> `context` -> `volatile`).

use std::collections::BTreeMap;

use gestalt_kernel::{ArtifactStoreView, ContextBuilder, ModelCapabilities, SessionSlice};
use serde_json::Value;

use crate::extensions::builders::{
    AGENTS_MD_PATH, MAX_AGENTS_MD_BYTES, MAX_MEMORY_MD_BYTES, MAX_USER_MD_BYTES, MEMORY_MD_PATH,
    USER_MD_PATH, collect_skill_index, inspect_slice_metadata, project_messages_from_slice,
    resolve_sorted_tool_defs, truncate_utf8_bytes,
};
use crate::extensions::{LlmInput, LlmToolDefinition, PromptTier};

const DEFAULT_STABLE_SYSTEM_PROMPT: &str = "You are Gestalt, a terminal-native software engineering and systems agent. \
Use registered tools carefully, follow workspace conventions, and verify changes before concluding.";

/// Deterministic 3-tier cache-stable [`ContextBuilder`] (`"standard"`).
///
/// Assembles:
/// - **Tier 1 (`stable`):** System instructions + lexicographically sorted tool schemas.
/// - **Tier 2 (`context`):** `<workspace>/AGENTS.md` (16 KB cap).
/// - **Tier 3 (`volatile`):** `.gestalt/memories/MEMORY.md` (8 KB cap),
///   `USER.md` (8 KB cap), and `.gestalt/skills/*/SKILL.md` YAML frontmatter index.
/// - **Messages + Ephemeral Slot:** Compaction-aware conversation history with
///   steering and handoff notes placed at the tail so Tier 1–2 prefixes remain cache-stable.
#[derive(Debug, Clone)]
pub struct StandardBuilder {
    system_prompt: String,
    default_tools: BTreeMap<String, LlmToolDefinition>,
}

impl Default for StandardBuilder {
    fn default() -> Self {
        Self::new(DEFAULT_STABLE_SYSTEM_PROMPT.to_string(), BTreeMap::new())
    }
}

impl StandardBuilder {
    /// Construct a [`StandardBuilder`] with custom stable instructions and tool definitions.
    #[must_use]
    pub fn new(system_prompt: String, default_tools: BTreeMap<String, LlmToolDefinition>) -> Self {
        Self {
            system_prompt,
            default_tools,
        }
    }
}

impl ContextBuilder for StandardBuilder {
    fn id(&self) -> &str {
        "standard"
    }

    fn digest(&self) -> &str {
        "std_standard_builder_v1"
    }

    fn build(
        &self,
        session: &SessionSlice,
        artifacts: &ArtifactStoreView,
        caps: &ModelCapabilities,
    ) -> Value {
        let meta = inspect_slice_metadata(session);
        let cfg_ref = meta.active_config.as_ref();

        let stable_text = cfg_ref
            .and_then(|c| c.params.get("system_prompt"))
            .and_then(Value::as_str)
            .unwrap_or(&self.system_prompt)
            .to_string();

        let mut system_tiers = vec![PromptTier {
            name: "stable".to_string(),
            cacheable: caps.supports_prompt_caching,
            content: stable_text,
        }];

        if let Some(agents) = artifacts.get(AGENTS_MD_PATH) {
            let capped = truncate_utf8_bytes(&agents.content, MAX_AGENTS_MD_BYTES);
            system_tiers.push(PromptTier {
                name: "context".to_string(),
                cacheable: caps.supports_prompt_caching,
                content: format!("# Workspace AGENTS.md\n{capped}"),
            });
        }

        let mut volatile_sections = Vec::new();
        if let Some(mem) = artifacts.get(MEMORY_MD_PATH) {
            let capped = truncate_utf8_bytes(&mem.content, MAX_MEMORY_MD_BYTES);
            volatile_sections.push(format!("# MEMORY.md\n{capped}"));
        }
        if let Some(user) = artifacts.get(USER_MD_PATH) {
            let capped = truncate_utf8_bytes(&user.content, MAX_USER_MD_BYTES);
            volatile_sections.push(format!("# USER.md\n{capped}"));
        }
        let skills = collect_skill_index(artifacts);
        if !skills.is_empty() {
            let mut lines = vec!["# Available Skills (load full SKILL.md via read)".to_string()];
            for sk in skills {
                let trig = if sk.triggers.is_empty() {
                    String::new()
                } else {
                    format!(" [triggers: {}]", sk.triggers.join(", "))
                };
                lines.push(format!(
                    "- **{}** (`{}`): {}{}",
                    sk.name, sk.path, sk.description, trig
                ));
            }
            volatile_sections.push(lines.join("\n"));
        }
        if !volatile_sections.is_empty() {
            system_tiers.push(PromptTier {
                name: "volatile".to_string(),
                cacheable: false,
                content: volatile_sections.join("\n\n"),
            });
        }

        let tools = resolve_sorted_tool_defs(&self.default_tools, cfg_ref);
        let messages = project_messages_from_slice(session, &meta);

        let max_output_tokens = (caps.max_output_tokens > 0).then_some(caps.max_output_tokens);
        let reasoning_budget_tokens = if caps.supports_reasoning {
            cfg_ref
                .and_then(|c| c.params.get("reasoning_budget_tokens"))
                .and_then(Value::as_u64)
        } else {
            None
        };

        let input = LlmInput {
            system_tiers,
            messages,
            tools,
            max_output_tokens,
            reasoning_budget_tokens,
        };
        serde_json::to_value(input).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        ArtifactEntry, Effect, EffectEnvelope, LoopConfig, NodePayload, SESSION_SCHEMA_VERSION,
        SessionTree, SideEffectClass, SteeringKind, SteeringSource, StepResult, sha256_hex,
    };
    use serde_json::json;

    #[test]
    fn standard_builder_preserves_tier1_and_tier2_byte_prefix_across_turns() {
        let mut tools = BTreeMap::new();
        tools.insert(
            "read".to_string(),
            LlmToolDefinition {
                name: "read".into(),
                description: "Read file".into(),
                parameters_schema: json!({"type": "object"}),
            },
        );
        let builder = StandardBuilder::new("Base system".into(), tools);
        let caps = ModelCapabilities {
            max_context_tokens: 200_000,
            max_output_tokens: 8_192,
            supports_prompt_caching: true,
            supports_reasoning: true,
            supports_vision: false,
        };

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
            Some("t1".into()),
            None,
            2,
            &NodePayload::TurnStart {
                turn_id: "t1".into(),
                prompt: json!("hello"),
                artifact_snapshots: BTreeMap::new(),
            },
        )
        .expect("n1");
        tree.append(
            "n2".into(),
            Some("t1".into()),
            Some("p1".into()),
            3,
            &NodePayload::PhaseOpen {
                phase_id: "p1".into(),
                preset_id: Some("general".into()),
                base_loop_id: "chat".into(),
                loop_digest: "d1".into(),
                decorator_digests: vec![],
                builder_id: "standard".into(),
                builder_digest: builder.digest().into(),
                builder_layer_digests: vec![],
                config: LoopConfig::default(),
            },
        )
        .expect("n2");

        let mut entries = BTreeMap::new();
        entries.insert(
            AGENTS_MD_PATH.into(),
            ArtifactEntry {
                path: AGENTS_MD_PATH.into(),
                content: "Always run cargo test.".into(),
                blob_digest: sha256_hex(b"Always run cargo test."),
            },
        );
        entries.insert(
            MEMORY_MD_PATH.into(),
            ArtifactEntry {
                path: MEMORY_MD_PATH.into(),
                content: "Memory v1".into(),
                blob_digest: sha256_hex(b"Memory v1"),
            },
        );
        let art1 = ArtifactStoreView::from_entries(entries.clone());
        let out1: LlmInput =
            serde_json::from_value(builder.build(&tree.slice_to(None).expect("s1"), &art1, &caps))
                .expect("de1");

        // Turn 2: modify MEMORY.md, add a SKILL.md, add a mid-run steering signal and aborted tool
        tree.append(
            "n3".into(),
            Some("t1".into()),
            Some("p1".into()),
            4,
            &NodePayload::Effect {
                envelope: EffectEnvelope {
                    effect_id: "e1".into(),
                    effect: Effect::Tool {
                        name: "read".into(),
                        args: json!({"path": "a.rs"}),
                        side_effect: SideEffectClass::Read,
                    },
                },
                state_token: None,
            },
        )
        .expect("n3");
        tree.append(
            "n4".into(),
            Some("t1".into()),
            Some("p1".into()),
            5,
            &NodePayload::StepResult(StepResult::Aborted {
                effect_id: "e1".into(),
                reason: "cancel".into(),
            }),
        )
        .expect("n4");
        tree.append(
            "n5".into(),
            Some("t1".into()),
            Some("p1".into()),
            6,
            &NodePayload::Steering {
                id: "st1".into(),
                kind: SteeringKind::Steer,
                content: json!("use rg instead"),
                source: SteeringSource::User,
            },
        )
        .expect("n5");

        entries.insert(
            MEMORY_MD_PATH.into(),
            ArtifactEntry {
                path: MEMORY_MD_PATH.into(),
                content: "Memory v2 changed".into(),
                blob_digest: sha256_hex(b"Memory v2 changed"),
            },
        );
        entries.insert(
            ".gestalt/skills/triage/SKILL.md".into(),
            ArtifactEntry {
                path: ".gestalt/skills/triage/SKILL.md".into(),
                content: "---\nname: triage\ndescription: Fast triage\n---\nSECRET BODY TEXT"
                    .into(),
                blob_digest: sha256_hex(b"skill"),
            },
        );
        let art2 = ArtifactStoreView::from_entries(entries);
        let out2: LlmInput =
            serde_json::from_value(builder.build(&tree.slice_to(None).expect("s2"), &art2, &caps))
                .expect("de2");

        // Tier 1 (stable), Tier 2 (context), and tools MUST be byte-identical!
        assert_eq!(out1.system_tiers[0], out2.system_tiers[0]);
        assert_eq!(out1.system_tiers[1], out2.system_tiers[1]);
        assert_eq!(out1.tools, out2.tools);

        // Tier 3 contains skill index only, never the body
        assert!(out2.system_tiers[2].content.contains("triage"));
        assert!(!out2.system_tiers[2].content.contains("SECRET BODY TEXT"));

        // Orphan tool call was paired with aborted tool result and steering landed in messages
        let serialized_msgs = serde_json::to_string(&out2.messages).expect("msgs");
        assert!(serialized_msgs.contains("use rg instead"));
        assert!(serialized_msgs.contains("aborted"));
    }
}
