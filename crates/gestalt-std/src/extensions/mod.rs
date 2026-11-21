#![forbid(unsafe_code)]
//! Standard user-space `Extension` modules: shared wire schemas, loops,
//! decorators, builders, presets, commands, tools, and vendor provider adapters.

#[cfg(feature = "loops")]
pub mod builders;
#[cfg(feature = "loops")]
pub mod commands;
#[cfg(feature = "loops")]
pub mod decorators;
#[cfg(feature = "loops")]
pub mod loops;
#[cfg(feature = "loops")]
pub mod presets;
#[cfg(feature = "providers")]
pub mod providers;
#[cfg(feature = "tools")]
pub mod tools;

use gestalt_kernel::SandboxPolicy;
use gestalt_runtime::estimate_tokens_fallback;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(feature = "loops")]
pub use builders::{
    DecisionStateBuilder, SlidingWindowBuilder, StandardBuilder, discover_tracked_artifact_paths,
};
#[cfg(feature = "loops")]
pub use commands::{apply_login_resolution, register_std_commands};
#[cfg(feature = "loops")]
pub use decorators::{CompactionDecorator, CuratorDecorator, GoalCheckDecorator};
#[cfg(feature = "loops")]
pub use loops::{ChatLoop, ChatLoopFactory, DecisionRouter, DecisionRouterFactory};
#[cfg(feature = "loops")]
pub use presets::{StdExtension, std_ui_surface};
#[cfg(feature = "providers")]
pub use providers::{AnthropicProvider, JevProvider, OpenAiProvider, register_std_providers};
#[cfg(feature = "tools")]
pub use tools::{
    BackgroundTaskManager, BashKillTool, BashOutputTool, BashTool, EditTool, GlobTool, GrepTool,
    ReadTool, StdToolsExtension, WriteTool, register_std_tools,
};

/// Default workspace-relative protected paths registered by `gestalt-std` so
/// regular shell/file tools cannot mutate memory or skill artifacts directly.
pub const DEFAULT_PROTECTED_PATHS: &[&str] =
    &[".gestalt/memories/", ".gestalt/skills/", ".agents/skills/"];

/// Construct a default [`SandboxPolicy`] with network disabled, `max_spawn_depth = 1`,
/// and [`DEFAULT_PROTECTED_PATHS`] enforced read-only.
#[must_use]
pub fn default_std_sandbox_policy() -> SandboxPolicy {
    SandboxPolicy {
        network: false,
        protected_paths: DEFAULT_PROTECTED_PATHS
            .iter()
            .map(|p| (*p).to_string())
            .collect(),
        max_spawn_depth: 1,
    }
}

/// A single tiered system prompt section emitted by [`StandardBuilder`] or
/// [`SlidingWindowBuilder`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTier {
    /// Tier name (`"stable"`, `"context"`, `"volatile"`).
    pub name: String,
    /// Whether this tier is eligible for provider prefix caching.
    pub cacheable: bool,
    /// UTF-8 tier text content.
    pub content: String,
}

/// Role of a message in [`LlmInput::messages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    /// User prompt, steering, or tool-result turn.
    User,
    /// Assistant model output turn.
    Assistant,
    /// Explicit tool-result role (for providers distinguishing tool role).
    Tool,
}

/// Normalized multimodal/tool content block inside [`ChatMessage`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain UTF-8 text block.
    Text {
        /// Text content.
        text: String,
    },
    /// Normalized reasoning / thinking block with optional provider signature.
    Thought {
        /// Reasoning text.
        text: String,
        /// Optional opaque provider signature preserved across multi-turn requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Tool invocation requested by the model.
    ToolUse {
        /// Unique tool call identifier.
        id: String,
        /// Registered tool slot name.
        name: String,
        /// Tool argument JSON payload.
        input: Value,
    },
    /// Tool execution outcome paired 1:1 with a prior [`ContentBlock::ToolUse`].
    ToolResult {
        /// Correlated tool call identifier.
        tool_use_id: String,
        /// Tool output payload (or error message).
        content: Value,
        /// Whether the tool call resulted in an error or denial.
        is_error: bool,
        /// Optional SHA-256 blob digest if output spilled to `.gestalt/blobs/`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blob_digest: Option<String>,
    },
    /// Multimodal media reference backed by `.gestalt/blobs/<sha256>`.
    Media {
        /// IANA MIME type (e.g., `"image/png"`).
        mime_type: String,
        /// Content-addressed SHA-256 blob digest.
        blob_digest: String,
    },
}

/// Normalized chat message in [`LlmInput`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role.
    pub role: ChatRole,
    /// Ordered content blocks.
    pub content: Vec<ContentBlock>,
}

/// Tool schema definition passed to generative LLM providers inside [`LlmInput`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmToolDefinition {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for parameters.
    pub parameters_schema: Value,
}

/// Normalized generative LLM input JSON placed in `Effect::Model { kind: Llm, input, .. }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmInput {
    /// Ordered system prompt tiers (`stable` -> `context` -> `volatile`).
    #[serde(default)]
    pub system_tiers: Vec<PromptTier>,
    /// Chronological conversation messages.
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// Lexicographically sorted active tool definitions.
    #[serde(default)]
    pub tools: Vec<LlmToolDefinition>,
    /// Optional output token limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Optional extended reasoning token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_budget_tokens: Option<u64>,
}

/// Normalized reasoning block returned inside [`LlmOutput`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThoughtBlock {
    /// Reasoning text.
    pub text: String,
    /// Optional provider signature for multi-turn verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Normalized tool invocation parsed from an LLM response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Tool call identifier.
    pub id: String,
    /// Target tool slot name.
    pub name: String,
    /// Parsed argument JSON value (or `{ "raw": "..." }` if malformed).
    pub args: Value,
}

/// Normalized generative LLM output JSON stored in `StepResult::Model { kind: Llm, output, .. }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmOutput {
    /// Assembled assistant text response.
    #[serde(default)]
    pub text: String,
    /// Normalized reasoning blocks emitted during generation.
    #[serde(default)]
    pub reasoning: Vec<ThoughtBlock>,
    /// Tool calls requested by the model.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRequest>,
    /// Optional stop reason (e.g., `"end_turn"`, `"tool_use"`, `"max_tokens"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// Discrete candidate choice passed to a `ModelKind::Decision` model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionChoice {
    /// Choice identifier.
    pub id: String,
    /// Human-readable description of this route/action.
    pub description: String,
}

/// Normalized System One decision request JSON (`Effect::Model { kind: Decision, input, .. }`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecisionInput {
    /// Compact structured state snapshot.
    pub state: Value,
    /// Candidate discrete choices.
    #[serde(default)]
    pub choices: Vec<DecisionChoice>,
    /// Optional classification rubric.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<String>,
}

/// Normalized System One decision response JSON (`StepResult::Model { kind: Decision, output, .. }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionOutput {
    /// Selected choice ID.
    pub choice: String,
    /// Calibrated confidence score in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Model-reported or measured latency in milliseconds.
    #[serde(default)]
    pub latency_ms: u64,
}

/// Deterministic fallback token count estimator for arbitrary JSON payloads (`chars / 4`).
#[must_use]
pub fn estimate_json_tokens(value: &Value) -> u64 {
    let text = serde_json::to_string(value).unwrap_or_default();
    estimate_tokens_fallback(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_schemas_round_trip_losslessly() {
        let input = LlmInput {
            system_tiers: vec![PromptTier {
                name: "stable".into(),
                cacheable: true,
                content: "You are Gestalt.".into(),
            }],
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![
                    ContentBlock::Text {
                        text: "Inspect image".into(),
                    },
                    ContentBlock::Media {
                        mime_type: "image/png".into(),
                        blob_digest: "ab".repeat(32),
                    },
                ],
            }],
            tools: vec![LlmToolDefinition {
                name: "read".into(),
                description: "Read file".into(),
                parameters_schema: json!({"type": "object"}),
            }],
            max_output_tokens: Some(4096),
            reasoning_budget_tokens: Some(1024),
        };
        let encoded = serde_json::to_value(&input).expect("ser llm input");
        let decoded: LlmInput = serde_json::from_value(encoded).expect("de llm input");
        assert_eq!(input, decoded);

        let dec_in = DecisionInput {
            state: json!({"prompt": "run tests"}),
            choices: vec![DecisionChoice {
                id: "run_tests".into(),
                description: "Run cargo test".into(),
            }],
            rubric: Some("Pick run_tests if user asks to test".into()),
        };
        let dec_json = serde_json::to_value(&dec_in).expect("ser dec input");
        let dec_back: DecisionInput = serde_json::from_value(dec_json).expect("de dec input");
        assert_eq!(dec_in, dec_back);
    }

    #[test]
    fn default_protected_paths_include_memories_and_skills() {
        let pol = default_std_sandbox_policy();
        assert!(!pol.network);
        assert!(pol.protected_paths.contains(".gestalt/memories/"));
        assert!(pol.protected_paths.contains(".gestalt/skills/"));
    }
}
