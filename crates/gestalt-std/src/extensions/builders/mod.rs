#![forbid(unsafe_code)]
//! Pure, deterministic `ContextBuilder` implementations and workspace artifact
//! projection helpers (`AGENTS.md`, `MEMORY.md`, `USER.md`, `SKILL.md`).

pub mod decision_state;
pub mod sliding_window;
pub mod standard;

use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::Path;

use gestalt_kernel::{
    ArtifactStoreView, Effect, LoopConfig, ModelKind, NodePayload, SessionSlice, SteeringKind,
    StepResult,
};
use serde_json::{Value, json};

use crate::extensions::{ChatMessage, ChatRole, ContentBlock, LlmOutput, LlmToolDefinition};

pub use decision_state::DecisionStateBuilder;
pub use sliding_window::SlidingWindowBuilder;
pub use standard::StandardBuilder;

/// Workspace-relative path for Tier 2 repository instructions.
pub const AGENTS_MD_PATH: &str = "AGENTS.md";
/// Inline byte cap for `AGENTS.md` (16 KB).
pub const MAX_AGENTS_MD_BYTES: usize = 16 * 1024;

/// Workspace-relative path for Tier 3 long-term agent memory.
pub const MEMORY_MD_PATH: &str = ".gestalt/memories/MEMORY.md";
/// Inline byte cap for `MEMORY.md` (8 KB).
pub const MAX_MEMORY_MD_BYTES: usize = 8 * 1024;

/// Workspace-relative path for Tier 3 operator/user preferences.
pub const USER_MD_PATH: &str = ".gestalt/memories/USER.md";
/// Inline byte cap for `USER.md` (8 KB).
pub const MAX_USER_MD_BYTES: usize = 8 * 1024;

/// Workspace-relative prefix for progressive-disclosure skill definitions.
pub const SKILLS_PREFIX: &str = ".gestalt/skills/";

/// Parsed YAML frontmatter metadata from a `SKILL.md` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillIndexEntry {
    /// Workspace-relative path to `SKILL.md`.
    pub path: String,
    /// Skill name.
    pub name: String,
    /// Short skill description.
    pub description: String,
    /// Optional trigger phrases.
    pub triggers: Vec<String>,
}

/// Truncate UTF-8 text cleanly at a character boundary so it never exceeds
/// `max_bytes` (minus a deterministic truncation suffix when truncated).
#[must_use]
pub fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> Cow<'_, str> {
    if s.len() <= max_bytes {
        return Cow::Borrowed(s);
    }
    const SUFFIX: &str = "\n... [truncated]";
    let budget = max_bytes.saturating_sub(SUFFIX.len());
    let mut end = budget.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{SUFFIX}", &s[..end]))
}

/// Parse leading YAML frontmatter (`--- ... ---`) from a `SKILL.md` file without
/// adding a YAML parser dependency.
#[must_use]
pub fn parse_skill_frontmatter(path: &str, content: &str) -> Option<SkillIndexEntry> {
    let trimmed = content.trim_start_matches('\u{feff}');
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))?;
    let end_pos = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    let yaml_block = &rest[..end_pos];

    let mut name = None;
    let mut description = None;
    let mut triggers = Vec::new();
    let mut in_triggers_list = false;

    for raw_line in yaml_block.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if in_triggers_list && let Some(item) = line.strip_prefix("- ") {
            let clean = item.trim().trim_matches('"').trim_matches('\'');
            if !clean.is_empty() {
                triggers.push(clean.to_string());
            }
            continue;
        }
        in_triggers_list = false;

        if let Some((key, val)) = line.split_once(':') {
            let k = key.trim();
            let v = val.trim();
            match k {
                "name" => {
                    name = Some(v.trim_matches('"').trim_matches('\'').to_string());
                }
                "description" => {
                    description = Some(v.trim_matches('"').trim_matches('\'').to_string());
                }
                "triggers" => {
                    if v.starts_with('[') && v.ends_with(']') {
                        let inner = &v[1..v.len() - 1];
                        for item in inner.split(',') {
                            let clean = item.trim().trim_matches('"').trim_matches('\'');
                            if !clean.is_empty() {
                                triggers.push(clean.to_string());
                            }
                        }
                    } else if v.is_empty() {
                        in_triggers_list = true;
                    }
                }
                _ => {}
            }
        }
    }

    let skill_name = name.filter(|n| !n.is_empty())?;
    Some(SkillIndexEntry {
        path: path.to_string(),
        name: skill_name,
        description: description.unwrap_or_default(),
        triggers,
    })
}

/// Collect sorted [`SkillIndexEntry`] metadata from `ArtifactStoreView` without
/// loading full `SKILL.md` bodies into the prompt.
#[must_use]
pub fn collect_skill_index(artifacts: &ArtifactStoreView) -> Vec<SkillIndexEntry> {
    let mut entries = Vec::new();
    for (path, art) in artifacts.entries() {
        if path.starts_with(SKILLS_PREFIX)
            && path.ends_with("/SKILL.md")
            && let Some(parsed) = parse_skill_frontmatter(path, &art.content)
        {
            entries.push(parsed);
        }
    }
    entries
}

/// Discover existing workspace-relative artifact paths (`AGENTS.md`, `MEMORY.md`,
/// `USER.md`, and `.gestalt/skills/*/SKILL.md`) for Turn-start `BlobStore` snapshotting.
#[must_use]
pub fn discover_tracked_artifact_paths(workspace_root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    for fixed in [AGENTS_MD_PATH, MEMORY_MD_PATH, USER_MD_PATH] {
        if workspace_root.join(fixed).is_file() {
            paths.push(fixed.to_string());
        }
    }
    let skills_dir = workspace_root.join(".gestalt").join("skills");
    if let Ok(read_dir) = fs::read_dir(&skills_dir) {
        let mut skill_names = Vec::new();
        for entry in read_dir.flatten() {
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                let skill_file = entry.path().join("SKILL.md");
                if skill_file.is_file() {
                    skill_names.push(format!("{SKILLS_PREFIX}{name}/SKILL.md"));
                }
            }
        }
        skill_names.sort();
        paths.extend(skill_names);
    }
    paths
}

pub(crate) struct SliceMetadata {
    pub active_config: Option<LoopConfig>,
    pub active_handoff: Value,
    pub latest_compaction: Option<(String, String)>,
}

pub(crate) fn inspect_slice_metadata(session: &SessionSlice) -> SliceMetadata {
    let mut active_config = None;
    let mut active_handoff = Value::Null;
    let mut latest_compaction = None;

    for node in session.nodes() {
        if let Ok(payload) = node.decode_payload() {
            match payload {
                NodePayload::PhaseOpen { config, .. } => {
                    active_config = Some(config);
                }
                NodePayload::PhaseSeal { handoff, .. } => {
                    active_handoff = handoff;
                }
                NodePayload::Custom { namespace, data }
                    if namespace == "std:compaction" || namespace == "custom:std:compaction" =>
                {
                    if let (Some(up_to), Some(summary)) = (
                        data.get("compacted_up_to_node_id").and_then(Value::as_str),
                        data.get("summary").and_then(Value::as_str),
                    ) {
                        latest_compaction = Some((up_to.to_string(), summary.to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    SliceMetadata {
        active_config,
        active_handoff,
        latest_compaction,
    }
}

/// Resolve sorted [`LlmToolDefinition`]s by combining static defaults with
/// `PhaseOpen.config.params["tool_specs"]`, filtered by `allowed_tools`.
pub(crate) fn resolve_sorted_tool_defs(
    defaults: &BTreeMap<String, LlmToolDefinition>,
    config: Option<&LoopConfig>,
) -> Vec<LlmToolDefinition> {
    let mut merged = defaults.clone();
    if let Some(cfg) = config
        && let Some(specs_val) = cfg.params.get("tool_specs")
        && let Ok(extra) = serde_json::from_value::<Vec<LlmToolDefinition>>(specs_val.clone())
    {
        for spec in extra {
            merged.insert(spec.name.clone(), spec);
        }
    }

    let allowed = config.and_then(|c| c.allowed_tools.as_ref());
    merged
        .into_values()
        .filter(|t| allowed.is_none_or(|list| list.iter().any(|a| a == &t.name)))
        .collect()
}

pub(crate) fn project_messages_from_slice(
    session: &SessionSlice,
    meta: &SliceMetadata,
) -> Vec<ChatMessage> {
    let nodes = session.nodes();
    let start_idx = match &meta.latest_compaction {
        Some((up_to_id, _)) => nodes
            .iter()
            .position(|n| n.id == *up_to_id)
            .map_or(0, |pos| pos + 1),
        None => 0,
    };

    let mut messages = Vec::new();
    if let Some((_, summary)) = &meta.latest_compaction {
        messages.push(ChatMessage {
            role: ChatRole::User,
            content: vec![ContentBlock::Text {
                text: format!("[Compacted earlier session history]\n{summary}"),
            }],
        });
    }

    let mut pending_tool_call_ids: VecDeque<String> = VecDeque::new();
    let mut effect_to_tool_use_id: BTreeMap<String, String> = BTreeMap::new();
    let mut pending_tool_results: Vec<ContentBlock> = Vec::new();
    let mut ephemeral_notes: Vec<String> = Vec::new();

    if !meta.active_handoff.is_null() {
        ephemeral_notes.push(format!("[Phase handoff]: {}", meta.active_handoff));
    }

    let flush_tool_results = |msgs: &mut Vec<ChatMessage>, results: &mut Vec<ContentBlock>| {
        if !results.is_empty() {
            msgs.push(ChatMessage {
                role: ChatRole::User,
                content: std::mem::take(results),
            });
        }
    };

    for node in &nodes[start_idx..] {
        let Ok(payload) = node.decode_payload() else {
            continue;
        };
        match payload {
            NodePayload::TurnStart { prompt, .. } => {
                flush_tool_results(&mut messages, &mut pending_tool_results);
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    content: prompt_value_to_blocks(&prompt),
                });
            }
            NodePayload::Effect { envelope, .. } => {
                if matches!(envelope.effect, Effect::Tool { .. }) {
                    let call_id = pending_tool_call_ids
                        .pop_front()
                        .unwrap_or_else(|| envelope.effect_id.clone());
                    effect_to_tool_use_id.insert(envelope.effect_id, call_id);
                }
            }
            NodePayload::StepResult(sr) => match sr {
                StepResult::Model {
                    kind: ModelKind::Llm,
                    output,
                    ..
                } => {
                    flush_tool_results(&mut messages, &mut pending_tool_results);
                    pending_tool_call_ids.clear();
                    if let Ok(parsed) = serde_json::from_value::<LlmOutput>(output.clone()) {
                        let mut blocks = Vec::new();
                        for th in parsed.reasoning {
                            blocks.push(ContentBlock::Thought {
                                text: th.text,
                                signature: th.signature,
                            });
                        }
                        if !parsed.text.is_empty() {
                            blocks.push(ContentBlock::Text { text: parsed.text });
                        }
                        for tc in parsed.tool_calls {
                            pending_tool_call_ids.push_back(tc.id.clone());
                            blocks.push(ContentBlock::ToolUse {
                                id: tc.id,
                                name: tc.name,
                                input: tc.args,
                            });
                        }
                        if !blocks.is_empty() {
                            messages.push(ChatMessage {
                                role: ChatRole::Assistant,
                                content: blocks,
                            });
                        }
                    } else if let Some(text) = output.as_str() {
                        messages.push(ChatMessage {
                            role: ChatRole::Assistant,
                            content: vec![ContentBlock::Text {
                                text: text.to_string(),
                            }],
                        });
                    }
                }
                StepResult::Tool {
                    effect_id,
                    output,
                    is_error,
                    blob_digest,
                } => {
                    if let Some(tool_use_id) = effect_to_tool_use_id.remove(&effect_id) {
                        pending_tool_results.push(ContentBlock::ToolResult {
                            tool_use_id,
                            content: output,
                            is_error,
                            blob_digest,
                        });
                    }
                }
                StepResult::Denied { effect_id, reason } => {
                    if let Some(tool_use_id) = effect_to_tool_use_id.remove(&effect_id) {
                        pending_tool_results.push(ContentBlock::ToolResult {
                            tool_use_id,
                            content: json!({ "denied": reason }),
                            is_error: true,
                            blob_digest: None,
                        });
                    }
                }
                StepResult::Aborted { effect_id, reason } => {
                    if let Some(tool_use_id) = effect_to_tool_use_id.remove(&effect_id) {
                        pending_tool_results.push(ContentBlock::ToolResult {
                            tool_use_id,
                            content: json!({ "aborted": reason }),
                            is_error: true,
                            blob_digest: None,
                        });
                    }
                }
                StepResult::Error {
                    effect_id, message, ..
                } => {
                    if let Some(tool_use_id) = effect_to_tool_use_id.remove(&effect_id) {
                        pending_tool_results.push(ContentBlock::ToolResult {
                            tool_use_id,
                            content: json!({ "error": message }),
                            is_error: true,
                            blob_digest: None,
                        });
                    }
                }
                _ => {}
            },
            NodePayload::Steering {
                kind: SteeringKind::Steer | SteeringKind::UrgentSteer,
                content,
                ..
            } => {
                let text = match content {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                ephemeral_notes.push(format!("[Steering]: {text}"));
            }
            NodePayload::Custom { namespace, data }
                if namespace == "std:goal_feedback" || namespace == "custom:std:goal_feedback" =>
            {
                if let Some(fb) = data.get("feedback").and_then(Value::as_str) {
                    ephemeral_notes.push(format!("[Goal check feedback]: {fb}"));
                }
            }
            _ => {}
        }
    }

    flush_tool_results(&mut messages, &mut pending_tool_results);

    if !ephemeral_notes.is_empty() {
        let note_text = ephemeral_notes.join("\n");
        let can_append_to_last_user = messages.last().is_some_and(|m| {
            m.role == ChatRole::User
                && !m
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        });
        if can_append_to_last_user {
            if let Some(last) = messages.last_mut() {
                last.content.push(ContentBlock::Text { text: note_text });
            }
        } else {
            messages.push(ChatMessage {
                role: ChatRole::User,
                content: vec![ContentBlock::Text { text: note_text }],
            });
        }
    }

    messages
}

pub(crate) fn prompt_value_to_blocks(prompt: &Value) -> Vec<ContentBlock> {
    if let Some(s) = prompt.as_str() {
        return vec![ContentBlock::Text {
            text: s.to_string(),
        }];
    }
    if let Ok(blocks) = serde_json::from_value::<Vec<ContentBlock>>(prompt.clone()) {
        return blocks;
    }
    vec![ContentBlock::Text {
        text: prompt.to_string(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_utf8_respects_multibyte_boundaries() {
        let s = "🦀".repeat(5_000); // 20,000 bytes
        let truncated = truncate_utf8_bytes(&s, MAX_AGENTS_MD_BYTES);
        assert!(truncated.len() <= MAX_AGENTS_MD_BYTES);
        assert!(truncated.ends_with("... [truncated]"));
    }

    #[test]
    fn parse_skill_frontmatter_ignores_body() {
        let content = "---\nname: rust-debug\ndescription: Debug Rust builds\ntriggers: [cargo, clippy]\n---\n\n# Full Skill Body\nVery long instructions...";
        let entry = parse_skill_frontmatter(".gestalt/skills/rust-debug/SKILL.md", content)
            .expect("frontmatter");
        assert_eq!(entry.name, "rust-debug");
        assert_eq!(entry.description, "Debug Rust builds");
        assert_eq!(entry.triggers, vec!["cargo", "clippy"]);
    }
}
