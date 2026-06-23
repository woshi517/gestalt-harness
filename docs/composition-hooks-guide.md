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
    async fn prepare_next_turn(&self, context: &PrepareNextTurnCtx) -> Result<HookOutcome>;
    async fn on_event(&self, context: &OnEventCtx) -> Result<()>;
}
```

All six methods are `async` and return `gestalt_runtime::error::Result`. The first five return a `HookOutcome`; `on_event` is a fire-and-forget observer that returns `Result<()>`.

---

## Lifecycle Points

### `before_context_build`

```rust
pub struct BeforeContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub artifact_dir: Option<PathBuf>,
}
```

**When:** Before the context packet is assembled for the LLM prompt.

**What happens:** The hook receives the raw session history. It can inject system messages (`AddContext`) that will be included in the prompt, or abort the turn (`Block`).

**Returning `AddContext`:** The message is pushed into the `patch_store` (an `Arc<Mutex<Vec<ContextPatch>>>`). The `RuntimeContextPipeline` then injects these messages into the assembled context packet after the first `Message::System`.

### `after_context_build`

```rust
pub struct AfterContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub packet: ContextPacket,
    pub artifact_dir: Option<PathBuf>,
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

### `prepare_next_turn`

```rust
pub struct PrepareNextTurnCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub turn_index: usize,
    pub current_model: String,
    pub current_provider: String,
}
```

**When:** After a turn completes and tool execution has finished, but before the next request is built, if the session would otherwise proceed into another turn.

**What happens:** The hook can inspect the completed turn history, including tool results that already landed in the session, and decide whether the next LLM request should use a different model or provider. Returning `Continue` keeps the next request unchanged. Returning `SwitchModel { model, provider? }` applies a one-shot override to the very next request only. Returning `Block { reason }` stops the session before the next turn begins, provided the agent loop would otherwise continue.

`current_model` and `current_provider` reflect the effective model/provider for the turn that just executed. If a previous turn set a one-shot override, those effective values are reported rather than the original session defaults.

**Important:** V1 keeps this lifecycle narrow. It is intentionally limited to next-turn model/provider switching. It is not a general request-override surface for parameters like `temperature` or `max_tokens`. Cross-provider switching is not yet reliably honored by the runtime in V1; the model override is applied, but a provider hint that does not match the active provider is surfaced explicitly and is not relied upon for routing.

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
    SwitchModel { model: String, provider: Option<String> },
}
```

| Variant | Context | Tool Policy | Effect |
|---------|---------|-------------|--------|
| `Continue` | ✓ | ✓ | Proceed normally |
| `Block { reason }` | Aborts the turn | Denies the tool call | Emits `AgentEvent::Error` with the reason |
| `AddContext { message }` | Injects a `Message` into the context packet (turn N+1 for after_context_build, same turn for before_context_build) | N/A (ignored) | Cached in `patch_store` |
| `Annotate { metadata }` | Attaches `serde_json::Value` metadata to the hook outcome | Attaches metadata | Consumed by observers / event subscribers |
| `SwitchModel { model, provider? }` | N/A | N/A | Sets a one-shot override for the next request only |

---

## Fail-Closed Semantics

- **`before_tool_policy` returning `Err`:** The `RuntimePolicyEngine` treats any error as a policy denial and returns `PolicyDecision::Denied`. This is a security invariant — hook execution failure must not accidentally permit a tool call.
- **Context hooks (`before_context_build` / `after_context_build`) returning `Err`:** The error is logged and the hook outcome is silently treated as `Continue` (the turn proceeds). The error is published as `RuntimeEvent::HookFailed`.
- **`prepare_next_turn` returning `Err`:** Treat it as fail-open for V1 by default. The runtime logs the error, emits `RuntimeEvent::HookFailed`, and proceeds without applying a next-turn override.
- **`after_tool_result` returning `Err`:** The error is published as `HookFailed` and the turn continues without special recovery.

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
        "turn_index": 0,         // only in prepare_next_turn
        "current_model": "...",  // only in prepare_next_turn
        "current_provider": "...", // only in prepare_next_turn
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
    .assembler(assembler)
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

---

## Queue-Backed Steering vs. Context Patching

Queue-backed steering and context patching are distinct mechanisms designed for different durability and semantic needs:

1. **Queue-Backed Steering (Durable)**
   * **Purpose**: Used for user, operator, or automation inputs that represent canonical history additions.
   * **Mechanism**: Messages are enqueued into the runtime steering queue, drained at turn boundaries, and appended directly to `session.history` as `Message::User` with optional `MessageMetadata` (source, queued message id, injected turn).
   * **Durability & Replay**: These messages are captured in trace checkpoints and persisted. Replay/resume flows treat these as canonical committed history, ensuring exact determinism.
   * **History schema**: The `Message::User` variant now carries an optional `metadata: Option<MessageMetadata>` field. Metadata records the original `MessageSource` (User, Operator, Automation, FollowUp), the queued message id, and the turn at which injection occurred. All non-injected user messages use `metadata: None`. Model adapters treat all user messages uniformly regardless of metadata.

### Queue Lifecycle Ownership

The steering queue transitions through three states with explicit ownership boundaries:

| State | Owner | When |
|-------|-------|------|
| `Active` | **`AgentRuntime`** | Runtime sets `Active` at `run_session()` entry, before the agent loop starts |
| `Closing` | **`AgentLoop`** | The core loop sets `Closing` at the terminal stop boundary, after the last safe pre-request drain point and before session-end hooks |
| `Completed` | **`AgentRuntime`** | Runtime sets `Completed` after `AgentLoop::run()` returns, trace workers are shut down, and outer cleanup finishes |

**Why this split matters:**
- `Active` and `Completed` are runtime-level decisions: the runtime layer owns session lifecycle, trace workers, and event bus subscriptions outside the core loop.
- `Closing` is an agent-level decision: only `AgentLoop` knows the exact semantic boundary where no further `build_request()` cycle can occur. If `Closing` were owned by runtime, the queue would remain `Active` during `on_session_end` hooks, allowing late enqueues to succeed even though no model turn can consume them.
- `Completed` has destructive semantics in `InMemorySteeringQueue` (pending messages are cleared), making it that is a runtime cleanup action.
- This split prevents future contributors from "simplifying" lifecycle transitions back into the wrong layer.

### Enqueue Rejection by Lifecycle

| Lifecycle State | `enqueue()` Result |
|----------------|--------------------|
| `Active` | `Queued` (normal path) or `Duplicate` (idempotent) |
| `Closing` | `SessionClosing` — the core loop has terminated; no further drain will occur |
| `Completed` | `SessionNotActive` — the run is fully finished; use explicit continue/branch semantics |

2. **Context Patching (Transient)**
   * **Purpose**: Used for injecting prompt-only instructions, skills metadata, or ephemeral UI state.
   * **Mechanism**: Implemented via `ContextContributor` or composition hooks (e.g. `before_context_build`). They inject temporary patches into the context packet.
   * **Durability & Replay**: These patches do *not* mutate `session.history`. Instead, they are resolved dynamically per-turn during context compilation based on active runtime, skill, or extension state.

By keeping these two paths separate, Gestalt preserves full auditability and replay safety for direct user steering while maintaining the flexibility of hook-driven context assembly.
