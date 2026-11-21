#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Optional batteries-included standard loops, decorators, builders, presets,
//! tools, providers, and surfaces for Gestalt.
//!
//! `gestalt-std` dogfoods the exact same public [`gestalt_kernel::Extension`],
//! [`gestalt_runtime::RuntimeRegistry`], and [`gestalt_runtime::ProviderRegistry`]
//! contracts as external user-space crates, with zero private kernel backdoors.

pub mod extensions;
#[cfg(feature = "surfaces")]
pub mod surfaces;

pub use extensions::{
    ChatMessage, ChatRole, ContentBlock, DEFAULT_PROTECTED_PATHS, DecisionChoice, DecisionInput,
    DecisionOutput, LlmInput, LlmOutput, LlmToolDefinition, PromptTier, ThoughtBlock,
    ToolCallRequest, default_std_sandbox_policy, estimate_json_tokens,
};

#[cfg(feature = "loops")]
pub use extensions::{
    ChatLoop, ChatLoopFactory, CompactionDecorator, CuratorDecorator, DecisionRouter,
    DecisionRouterFactory, DecisionStateBuilder, GoalCheckDecorator, SlidingWindowBuilder,
    StandardBuilder, StdExtension, apply_login_resolution, discover_tracked_artifact_paths,
    register_std_commands, std_ui_surface,
};

#[cfg(feature = "tools")]
pub use extensions::{
    BackgroundTaskManager, BashKillTool, BashOutputTool, BashTool, EditTool, GlobTool, GrepTool,
    ReadTool, StdToolsExtension, WriteTool, register_std_tools,
};

#[cfg(feature = "providers")]
pub use extensions::{AnthropicProvider, JevProvider, OpenAiProvider, register_std_providers};
