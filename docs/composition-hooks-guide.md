# Composition Hooks Guide

## Overview

`CompositionHooks` is the primary extension point for intercepting and modifying the agent lifecycle loop at runtime. Unlike core hooks (`ContextHook`, `ToolHook`, `TraceHook`), which operate at the level of the pure `AgentLoop`, composition hooks operate at the **runtime composition layer** — they have access to session context, tool inputs and results, and can inject context messages, block execution, or observe events.

The hook system is the bridge between `gestalt-core`'s pure agent loop and `gestalt-runtime`'s extension and policy infrastructure.

## The `CompositionHooks` Trait

```rust
#[async_trait]
pub trait CompositionHooks: Send + Sync {
    async fn before_context_build(&self, context: &BeforeContextBuildCtx) -> Result<HookOutcome>;
    async fn after_context_build(&self, context: &AfterContextBuildCtx) -> Result<HookOutcome>;
    async fn before_tool_policy(&self, context: &BeforeToolPolicyCtx) -> Result<HookOutcome>;
    async fn after_tool_result(&self, context: &AfterToolResultCtx) -> Result<HookOutcome>;
    async fn on_event(&self, context: &OnEventCtx) -> Result<()>;
}
```

All five methods are `async` and return `gestalt_runtime::error::Result`. The first four return a `HookOutcome`; `on_event` is a fire-and-forget observer that returns `Result<()>`.

---

## Lifecycle Points

### `before_context_build`

```rust
pub struct BeforeContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
}
```

**When:** Before the context packet is assembled for the LLM prompt.

**What happens:** The hook receives the raw session history. It can inject system messages (`AddContext`) that will be included in the prompt, or abort the turn (`Block`).

**Returning `AddContext`:** The message is pushed into the `patch_store` (a `Mutex<Vec<Message>>`). The `RuntimeContextPipeline` then injects these messages into the assembled context packet after the first `Message::System`.

### `after_context_build`

```rust
pub struct AfterContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub packet: ContextPacket,
}
```

**When:** After the context packet has been assembled and all `ContextContributor` instances have run.

**What happens:** The hook sees the full `ContextPacket`. The `patch_store` is **cleared** at the start of this phase (previous turn's context additions are consumed, not duplicated). Any new `AddContext` result is cached for the **next** turn.

### `before_tool_policy`

```rust
pub struct BeforeToolPolicyCtx {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}
```

**When:** Before the policy engine evaluates a tool invocation.

**What happens:** The hook can inspect the tool name and parsed JSON input. Returning `Block { reason }` prevents the tool from executing — the runtime emits `AgentEvent::Error` with the reason and the session continues. Returning `Err` also blocks (fail-closed).

### `after_tool_result`

```rust
pub struct AfterToolResultCtx {
    pub session_id: String,
    pub tool_name: String,
    pub result: ToolExecutionResult,
}
```

**When:** After a tool completes execution and a result is available.

**What happens:** The hook inspects the `ToolExecutionResult`. Blocking here emits an `AgentEvent::Error` that flags the result to the session loop.

### `on_event`

```rust
pub struct OnEventCtx {
    pub session_id: String,
    pub event: AgentEvent,
}
```

**When:** On every `AgentEvent` emitted by the core agent loop.

**What happens:** This is a non-blocking observation hook — it receives all events (tool calls, errors, messages, etc.) but its return value has no effect on execution. It is wired through `RuntimeTraceHookAdapter` → `tokio::sync::mpsc::unbounded_channel` → a background `tokio::spawn` worker that drains the channel and calls `on_event` for each event.

---

## `HookOutcome` Variants

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum HookOutcome {
    Continue,
    Block { reason: String },
    AddContext { message: Message },
    Annotate { metadata: serde_json::Value },
}
```

| Variant | Context | Tool Policy | Effect |
|---------|---------|-------------|--------|
| `Continue` | ✓ | ✓ | Proceed normally |
| `Block { reason }` | Aborts the turn | Denies the tool call | Emits `AgentEvent::Error` with the reason |
| `AddContext { message }` | Injects a `Message` into the context packet (turn N+1 for after_context_build, same turn for before_context_build) | N/A (ignored) | Cached in `patch_store` |
| `Annotate { metadata }` | Attaches `serde_json::Value` metadata to the hook outcome | Attaches metadata | Consumed by observers / event subscribers |

---

## Fail-Closed Semantics

- **`before_tool_policy` returning `Err`:** The `RuntimePolicyEngine` treats any error as a policy denial and returns `PolicyDecision::Denied`. This is a security invariant — hook execution failure must not accidentally permit a tool call.
- **Context hooks (`before_context_build` / `after_context_build`) returning `Err`:** The error is logged and the hook outcome is silently treated as `Continue` (the turn proceeds). The error is published as `RuntimeEvent::HookFailed`.
- **`before_tool_policy` returning `Err`:** The error is published as `HookFailed` and the tool is denied.

---

## Patch Store & Turn-to-Turn Context

The `patch_store` is an `Arc<Mutex<Vec<ContextPatch>>>` shared between `RuntimeContextHookAdapter` and `RuntimeContextPipeline`. Its lifecycle:

1. **Turn N, `before_context_build`:** Any `AddContext` message is pushed into the patch store as a `ContextPatch` with a `ContextStability` tag. `RuntimeContextPipeline` reads the patch store and injects messages into the assembled context packet.
2. **Turn N, `after_context_build`:** The patch store is **cleared first**, then any new `AddContext` from this hook is pushed. This ensures the previous turn's context injection is consumed (by the pipeline) and the new injection is cached for turn N+1.
3. **Turn N+1, `before_context_build`:** The cached message from turn N's `after_context_build` is in the patch store, gets injected by the pipeline, and the cycle repeats.

This means:
- `before_context_build` `AddContext` → injected into the **current** turn
- `after_context_build` `AddContext` → cached and applied to the **next** turn
- No indefinite duplication — the store is cleared each turn

### Context Stability & Cache-Aware Placement

When the pipeline uses the `Snapshot` assembly strategy (see [ADR-026](adrs/ADR-026-cache-aware-prompt-assembly.md)), messages are placed based on their `ContextStability` tag:

| Stability | Placement | Rationale |
|-----------|-----------|-----------|
| `SessionStatic` | Stable prefix | System prompt, tool definitions, workspace description — never changes mid-session |
| `ActivationStatic` | Stable prefix | MCP tool schemas, extension-registered tools — stable until a toolset activation changes |
| `TurnDynamic` | Dynamic tail | User/assistant messages, tool results — changes every turn |
| `Ephemeral` | Dynamic tail | Budget exhaustion notices, one-shot annotations — single-turn lifetime |

The stable prefix is grouped before the cache breakpoint so provider caches can recognize it across turns. The dynamic tail varies freely without affecting cache hit rates.

When `ContextStability` is not explicitly set by a `ContextContributor`, it defaults to `TurnDynamic`.

---

## Adapter Pattern

The runtime wires composition hooks into the core `AgentLoop` through three adapter types that implement `gestalt_core::hook` traits:

### `RuntimeContextHookAdapter`

```rust
pub struct RuntimeContextHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
    pub patch_store: Arc<Mutex<Vec<ContextPatch>>>,
    pub contributors: Vec<Arc<dyn ContextContributor>>,
    pub workspace_root: std::path::PathBuf,
    pub block_reason: Option<Arc<Mutex<Option<String>>>>,
    pub event_bus: RuntimeEventBus,
    pub initial_prompt_snapshot_hash: Option<String>,
}
```

Implements `gestalt_core::hook::ContextHook`. On each call:
1. Publishes `RuntimeEvent::HookStarted`
2. Builds the appropriate context struct (`BeforeContextBuildCtx` / `AfterContextBuildCtx`)
3. Calls the corresponding `CompositionHooks` method
4. Publishes `RuntimeEvent::HookCompleted` or `RuntimeEvent::HookFailed`
5. Handles the outcome (stores `AddContext` in patch store, stores `Block` reason)
6. After `before_context_build`: runs all registered `ContextContributor` instances

### `RuntimeToolHookAdapter`

```rust
pub struct RuntimeToolHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
    pub event_bus: RuntimeEventBus,
}
```

Implements `gestalt_core::hook::ToolHook`. On `before_tool_execution`:
1. Publishes `HookStarted`
2. Calls `hooks.before_tool_policy()` with `BeforeToolPolicyCtx`
3. If `Block`, emits `AgentEvent::Error { recoverable: true }`
4. On `Err`, publishes `HookFailed` and emits error event (fail-closed)

On `after_tool_execution`:
1. Publishes `HookStarted`
2. Calls `hooks.after_tool_result()` with `AfterToolResultCtx`
3. If `Block`, emits `AgentEvent::Error`

### `RuntimeTraceHookAdapter`

```rust
pub struct RuntimeTraceHookAdapter {
    pub tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}
```

Implements `gestalt_core::hook::TraceHook`. On `on_trace_write`:
1. Sends the `AgentEvent` through the unbounded channel
2. A background `tokio::spawn` worker drains the channel and calls `comp_hooks.on_event()` for each event
3. Each `on_event` call publishes `HookStarted` / `HookCompleted` / `HookFailed` events to the event bus

This decoupling ensures the core agent loop is not blocked by hook observation.

---

## `ComposedCompositionHooks`

```rust
pub struct ComposedCompositionHooks {
    pub user_hooks: Option<Arc<dyn CompositionHooks>>,
    pub extensions: Vec<Arc<dyn crate::extension::GestaltExtension>>,
}
```

Composes user-defined hooks with extension-provided hooks. Execution order:

1. **User hooks** run first (if present). If they return anything other than `Continue`, the result short-circuits and extension hooks are skipped.
2. **Extension hooks** run in order. Each extension that has a `HookDeclaration` matching the lifecycle point receives an RPC call via `pe.broker.call("hooks/call", params)`.
3. The `params` payload includes:
   - `name`: the hook declaration name
   - `lifecycle_point`: e.g. `"before_context_build"`
   - `context`: the full context struct serialized to JSON
4. The RPC response is parsed via `parse_hook_outcome(val)` which supports `"continue"`, `{ "type": "block", "reason": "..." }`, `{ "type": "add_context", "message": ... }`, and `{ "type": "annotate", "metadata": ... }`.
5. Extension hooks accumulate: if any extension returns `Block`, `AddContext`, or `Annotate`, that outcome wins (last writer wins for `AddContext`/`Annotate`; `Block` short-circuits).

The extension parameters are structured as:

```rust
serde_json::json!({
    "name": hook_decl.name,
    "lifecycle_point": "before_context_build",
    "context": {
        "session_id": "...",
        "history": [...],        // only present in context hooks
        "packet": {...},         // only in after_context_build
        "tool_name": "...",      // only in before_tool_policy / after_tool_result
        "tool_input": {...},     // only in before_tool_policy
        "result": {...},         // only in after_tool_result
        "event": {...},          // only in on_event
    }
})
```

`on_event` is fire-and-forget: extension errors are silently ignored (`let _ = pe.broker.call(...)`).

---

## Implementing a Custom Hook

```rust
use std::sync::Arc;
use gestalt_runtime::composition_hooks::{
    CompositionHooks, BeforeContextBuildCtx, AfterContextBuildCtx,
    BeforeToolPolicyCtx, AfterToolResultCtx, OnEventCtx, HookOutcome,
};

struct SafetyHook {
    blocked_tools: Vec<String>,
}

#[async_trait]
impl CompositionHooks for SafetyHook {
    async fn before_context_build(
        &self,
        _context: &BeforeContextBuildCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_context_build(
        &self,
        _context: &AfterContextBuildCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn before_tool_policy(
        &self,
        context: &BeforeToolPolicyCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        if self.blocked_tools.contains(&context.tool_name) {
            return Ok(HookOutcome::Block {
                reason: format!("Tool '{}' is blocked by safety policy", context.tool_name),
            });
        }
        Ok(HookOutcome::Continue)
    }

    async fn after_tool_result(
        &self,
        _context: &AfterToolResultCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn on_event(&self, _context: &OnEventCtx) -> gestalt_runtime::error::Result<()> {
        Ok(())
    }
}

// Register with the runtime
let runtime = AgentRuntimeBuilder::new()
    .provider(provider)
    .tools(tools)
    .middleware(middleware)
    .policy(policy)
    .approval(approval)
    .composition_hooks(Arc::new(SafetyHook {
        blocked_tools: vec!["rm".to_string(), "docker_exec".to_string()],
    }))
    .build()?;
```

---

## Event Integration

Each hook invocation publishes `RuntimeEvent` variants to the `RuntimeEventBus`:

```rust
RuntimeEvent::HookStarted {
    hook_name: String,           // e.g. "before_context_build"
    lifecycle_point: String,     // e.g. "before_context_build"
}

RuntimeEvent::HookCompleted {
    hook_name: String,
    lifecycle_point: String,
    outcome: String,             // Debug representation of HookOutcome
}

RuntimeEvent::HookFailed {
    hook_name: String,
    lifecycle_point: String,
    error: String,               // Error message
}
```

- `HookStarted` — published before every hook method call
- `HookCompleted` — published after a successful return
- `HookFailed` — published when a hook method returns `Err`

These events can be consumed by CLI subscribers (`gestalt runtime events`), the `RuntimeInspect` diagnostics, or any `broadcast::Receiver`.
