# Gestalt Runtime Crate (`gestalt-runtime`)

The `gestalt-runtime` crate acts as the **Runtime Composition Layer** of the Gestalt agent harness. It encapsulates concrete implementations of the provider, tools registry, policy engine, context pipeline, and trace hooks, wrapping the pure `AgentLoop` (from `gestalt-core`) inside a unified, reusable boundary (`AgentRuntime`).

This crate is the primary integration point for extending Gestalt via **custom extensions, composition hooks, context contributors, or custom policies**.

---

## Architecture Overview

`gestalt-runtime` sits between applications (such as CLI, TUI, or third-party orchestrators) and the core agent execution engine:

```mermaid
graph TD
    App[CLI / TUI / SDK Client] -->|invokes| Runtime[gestalt-runtime::AgentRuntime]
    Runtime -->|composed of| Registry[RuntimeRegistry]
    Runtime -->|manages| Adapters[Hook Adapters]
    Adapters -->|wrap| Hooks[CompositionHooks]
    Runtime -->|orchestrates| AgentLoop[gestalt-core::AgentLoop]
    
    subgraph Core Boundaries [gestalt-core (Pure)]
        AgentLoop
    end
```

---

## Core Interfaces

### 1. `AgentRuntime` and `AgentRuntimeBuilder`
`AgentRuntime` is the stateless execution boundary. You run agent prompts or continue existing sessions using:
- `run_prompt(input: UserInput)`: Starts a fresh session, captures a workspace snapshot, and runs the turn loop.
- `run_session(session: &mut Session, ...)`: Continues execution on an existing, mutable `Session` object.

To construct a runtime:
```rust
use std::sync::Arc;
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};

let runtime = AgentRuntimeBuilder::new()
    .provider(provider)      // Arc<dyn Provider>
    .tools(tools)            // Arc<dyn ToolCatalog>
    .middleware(middleware)  // Arc<dyn ContextPipeline>
    .policy(policy)          // Arc<dyn PolicyEngine>
    .approval(approval)      // Arc<dyn ApprovalProvider>
    .config(runtime_config)  // RuntimeConfig
    .build()
    .unwrap();
```

### 2. `RuntimeRegistry`
Stores the registered capabilities of the runtime. Extensions interact with the registry to install tools, custom providers, and hook adapters.
```rust
pub struct RuntimeRegistry {
    pub tools: HashMap<String, ToolMetadata>,
    pub providers: HashMap<String, ProviderMetadata>,
    pub context_contributors: HashMap<String, Arc<dyn ContextContributor>>,
    pub verifiers: Vec<String>,
    pub hooks: Vec<String>,
    pub extensions: Vec<String>,
}
```

---

## Extending the Runtime

Developers can customize and extend `AgentRuntime` using three primary mechanisms:

### 1. Context Contributors
Context contributors allow injecting system instructions or workspace state into the context packet during the context-building phase.

```rust
use std::path::Path;
use async_trait::async_trait;
use gestalt_core::message::Message;
use gestalt_runtime::{ContextContributor, Result};

pub struct GitBranchContributor;

#[async_trait]
impl ContextContributor for GitBranchContributor {
    fn name(&self) -> &str {
        "GitBranchContributor"
    }

    async fn contribute(&self, workspace_root: &Path) -> Result<Message> {
        // Run git command or read files to get current branch
        let branch = "feature/agent-composition"; 
        Ok(Message::System {
            content: format!("The current working git branch is: {}", branch),
        })
    }
}
```

### 2. Runtime Extensions (`GestaltExtension`)
Extensions are self-contained plugins that configure the runtime registry (e.g., registering new tools or context contributors).

```rust
use gestalt_runtime::{GestaltExtension, RuntimeRegistry, Result};

pub struct MyCustomExtension;

impl GestaltExtension for MyCustomExtension {
    fn name(&self) -> &str {
        "MyCustomExtension"
    }

    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()> {
        // Register extension name
        registry.register_extension(self.name().to_string())?;
        
        // Add custom context contributor
        registry.register_context_contributor(
            "git_contributor".to_string(),
            Arc::new(GitBranchContributor),
        )?;
        Ok(())
    }
}
```

---

## Composition Hooks (`CompositionHooks`)

The `CompositionHooks` trait allows you to intercept key lifecycle points of the agent loop. You can modify context, block tool executions, or observe events.

```rust
#[async_trait::async_trait]
pub trait CompositionHooks: Send + Sync {
    /// Runs before the LLM prompt is constructed.
    async fn before_context_build(&self, context: &BeforeContextBuildCtx) -> Result<HookOutcome>;

    /// Runs after the context packet is constructed but before sending it to the LLM.
    async fn after_context_build(&self, context: &AfterContextBuildCtx) -> Result<HookOutcome>;

    /// Runs before executing policy checks on a tool call.
    async fn before_tool_policy(&self, context: &BeforeToolPolicyCtx) -> Result<HookOutcome>;

    /// Runs after a tool has completed execution.
    async fn after_tool_result(&self, context: &AfterToolResultCtx) -> Result<HookOutcome>;

    /// Ordered, sequenced, and non-blocking event observation.
    async fn on_event(&self, context: &OnEventCtx) -> Result<()>;
}
```

### Hook Outcomes (`HookOutcome`)
Hooks return a `HookOutcome` to signal how the loop should proceed:
* `Continue`: Proceed with normal loop execution.
* `Block { reason }`: Abort execution.
  * In context hooks: Aborts the entire turn immediately.
  * In tool policy hooks: Blocks the specific tool execution.
* `AddContext { message }`: Inject a custom system or user message.
* `Annotate { metadata }`: Add custom metadata fields.

---

## Safety and Lifecycle Invariants

When implementing custom hooks, be aware of the following safety guarantees enforced by the runtime layer:

### 1. Fail-Closed Security
- **Tool Policy Hooks:** Any error (`Err(...)`) thrown by a `before_tool_policy` composition hook is treated as a security violation. The runtime fails closed, immediately returning a `PolicyDecision::Denied` to block the tool call.
- **Context Build Hooks:** If `before_context_build` or `after_context_build` returns a `Block` outcome, the session loop aborts immediately on the next emitted event with a `PolicyError::Denied` error.

### 2. Turn-to-Turn Context Accumulation
Context additions (`HookOutcome::AddContext`) returned by `after_context_build` on Turn $N$ are cached in the runtime's patch store and prepended to the system prompt on Turn $N+1$. 

> [!NOTE]
> The patch store is automatically cleared at the beginning of each turn's `after_context_build` phase. This ensures that context injected in one turn is applied to the next turn, but does not duplicate indefinitely over subsequent turns.

### 3. Sequenced, Lossless Event Observation
Trace hook events dispatched to `on_event` are funneled through a Tokio channel (`mpsc::unbounded_channel`).
- This guarantees **ordered delivery** of all execution events to your hook.
- The runtime ensures all events are fully processed by custom hooks before `run_session` returns, preventing loss of trailing events on loop shutdown.

---

## Example: Building a Runtime with Custom Hooks

Here is a full integration showing how to implement a custom hook that checks input prompts for safety violations:

```rust
use async_trait::async_trait;
use gestalt_runtime::{
    CompositionHooks, HookOutcome, BeforeContextBuildCtx, AfterContextBuildCtx,
    BeforeToolPolicyCtx, AfterToolResultCtx, OnEventCtx, Result
};

pub struct SafetyHook;

#[async_trait]
impl CompositionHooks for SafetyHook {
    async fn before_context_build(&self, ctx: &BeforeContextBuildCtx) -> Result<HookOutcome> {
        // Inspect the user prompt (last message in history)
        if let Some(msg) = ctx.history.last() {
            if let gestalt_core::message::Message::User { content } = msg {
                for block in content {
                    if let gestalt_core::message::ContentBlock::Text { text } = block {
                        if text.contains("drop database") {
                            return Ok(HookOutcome::Block {
                                reason: "Prompt contains unsafe commands".to_string(),
                            });
                        }
                    }
                }
            }
        }
        Ok(HookOutcome::Continue)
    }

    async fn after_context_build(&self, _ctx: &AfterContextBuildCtx) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn before_tool_policy(&self, _ctx: &BeforeToolPolicyCtx) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_tool_result(&self, _ctx: &AfterToolResultCtx) -> Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn on_event(&self, _ctx: &OnEventCtx) -> Result<()> {
        Ok(())
    }
}
```
