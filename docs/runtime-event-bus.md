# Runtime Event Bus

## Overview

The `RuntimeEventBus` is a publish-subscribe event bus built on `tokio::sync::broadcast`. It provides real-time event streaming for the runtime layer, extending the core `AgentEvent` stream with runtime-specific events (extension lifecycle, permission decisions, hook invocations, process management, RPC calls).

There are two layers of events:
- **`RuntimeEvent`** — runtime-layer events defined in `gestalt-runtime` (extension lifecycle, hooks, permissions, RPC, etc.)
- **`AgentEvent`** — core agent-loop events defined in `gestalt-core` (tool calls, errors, messages, etc.), wrapped in `RuntimeEvent::Agent` with a sequence number

Both types are consumed through the same `broadcast::Receiver<Arc<RuntimeEvent>>`.

---

## `RuntimeEventBus` Struct

```rust
#[derive(Clone)]
pub struct RuntimeEventBus {
    tx: broadcast::Sender<Arc<RuntimeEvent>>,
    next_seq: Arc<AtomicU64>,
    history: Arc<Mutex<Vec<RuntimeEvent>>>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `tx` | `broadcast::Sender<Arc<RuntimeEvent>>` | The broadcast channel sender (all subscribers share this) |
| `next_seq` | `Arc<AtomicU64>` | Monotonic sequence counter for `publish_agent()` |
| `history` | `Arc<Mutex<Vec<RuntimeEvent>>>` | In-memory buffer of all published events |

### `new()`

```rust
pub fn new() -> Self
```

Creates a bus with a broadcast channel capacity of **4096** events. When the channel is full, the oldest unconsumed event is dropped for lagging subscribers (`broadcast` behavior). The history buffer is unbounded.

---

## Publishing

### `publish(event)`

```rust
pub fn publish(&self, event: RuntimeEvent)
```

1. Appends a clone of the event to the `history` buffer (under `Mutex` lock).
2. Wraps the event in `Arc` and sends it through the broadcast channel.
3. Errors from the broadcast send are silently ignored (no subscribers = no problem).

### `publish_agent(agent_event)`

```rust
pub fn publish_agent(&self, agent_event: AgentEvent) -> u64
```

1. Atomically fetches and increments `next_seq` (using `Ordering::SeqCst`).
2. Wraps the `AgentEvent` in `RuntimeEvent::Agent { sequence_number, event }`.
3. Calls `publish()`.
4. Returns the assigned sequence number.

Sequence numbers are **monotonic per-bus** — each `publish_agent` call gets a unique, strictly increasing `u64`. This enables ordered replay of core agent events.

---

## Subscribing

### `subscribe()`

```rust
pub fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>>
```

Returns a new `broadcast::Receiver` attached to the bus's sender. Every subscriber receives every event published after subscription. Events published before the subscription are not replayed.

### `history()`

```rust
pub fn history(&self) -> Vec<RuntimeEvent>
```

Returns a clone of the entire in-memory event history. If the mutex is poisoned, returns an empty `Vec`. This is useful for diagnostics, testing, and late-joining consumers that need the full event log.

---

## `RuntimeEvent` Enum

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Agent {
        sequence_number: u64,
        event: AgentEvent,
    },
    ExtensionDiscovered {
        extension_id: String,
        manifest_path: String,
        manifest_hash: String,
    },
    ExtensionLoaded {
        extension_id: String,
    },
    ExtensionRejected {
        extension_id: String,
        reason: String,
    },
    ExtensionError {
        extension_id: String,
        message: String,
    },
    HookStarted {
        hook_name: String,
        lifecycle_point: String,
    },
    HookCompleted {
        hook_name: String,
        lifecycle_point: String,
        outcome: String,
    },
    HookFailed {
        hook_name: String,
        lifecycle_point: String,
        error: String,
    },
    ToolRegistered {
        extension_id: Option<String>,
        tool_name: String,
        schema_hash: String,
    },
    ContextInjectorRegistered {
        extension_id: Option<String>,
        injector_name: String,
    },
    PermissionDecision {
        extension_id: String,
        capability: String,
        permission: String,
        resource: Option<String>,
        granted: bool,
        reason: Option<String>,
    },
    ProcessSpawned {
        extension_id: String,
        pid: u32,
    },
    ProcessExited {
        extension_id: String,
        exit_code: Option<i32>,
    },
    ProcessKilled {
        extension_id: String,
        reason: String,
    },
    RpcRequest {
        extension_id: String,
        method: String,
        request_id: String,
    },
    RpcResponse {
        extension_id: String,
        method: String,
        request_id: String,
        success: bool,
    },
    ArtifactRouted {
        session_id: String,
        path: String,
        size_bytes: usize,
    },
    SessionSpawned {
        session_id: String,
    },
    SessionMessageQueued {
        session_id: String,
        message: gestalt_core::session_queue::QueuedSessionMessage,
    },
    ReloadStarted,
    ReloadCompleted,
    RuntimeError {
        message: String,
    },
}
```

### Variant Descriptions

| Variant | Published When | Key Fields |
|---------|---------------|------------|
| `Agent` | Every `AgentEvent` emitted by the core agent loop | `sequence_number` (monotonic per-bus), `event` (the core event) |
| `ExtensionDiscovered` | A manifest is found and parsed during `ExtensionDiscovery::discover_all` | `extension_id`, `manifest_path`, `manifest_hash` (SHA-256 of raw manifest content) |
| `ExtensionLoaded` | `ProcessExtensionBroker` handshake succeeds | `extension_id` |
| `ExtensionRejected` | Validation failure, spawn failure, or handshake timeout | `extension_id`, `reason` |
| `ExtensionError` | A line is read from the extension's stderr | `extension_id`, `message` |
| `HookStarted` | Before any composition hook method is called | `hook_name` (e.g. `"before_context_build"`), `lifecycle_point` (e.g. `"before_context_build"`) |
| `HookCompleted` | After a composition hook method returns `Ok` | `hook_name`, `lifecycle_point`, `outcome` (Debug string of `HookOutcome`) |
| `HookFailed` | After a composition hook method returns `Err` | `hook_name`, `lifecycle_point`, `error` |
| `ToolRegistered` | A tool is registered in the `RuntimeRegistry` | `extension_id` (None for base tools), `tool_name`, `schema_hash` |
| `ContextInjectorRegistered` | A context injector is registered | `extension_id` (None for user-defined), `injector_name` |
| `PermissionDecision` | Every permission check (path, network, shell) | `extension_id`, `capability`, `permission`, `resource`, `granted`, `reason` |
| `ProcessSpawned` | Extension child process starts | `extension_id`, `pid` |
| `ProcessExited` | Extension child process terminates | `extension_id`, `exit_code` |
| `ProcessKilled` | Extension child process is killed | `extension_id`, `reason` |
| `RpcRequest` | A JSON-RPC 2.0 request is sent to an extension | `extension_id`, `method`, `request_id` |
| `RpcResponse` | A JSON-RPC 2.0 response is received from an extension | `extension_id`, `method`, `request_id`, `success` |
| `ArtifactRouted` | An artifact is routed between sessions via `ArtifactStore` | `session_id`, `path`, `size_bytes` |
| `SessionSpawned` | A new agent session is created | `session_id` |
| `SessionMessageQueued` | A steering message is accepted into the runtime queue (only when lifecycle is `Active`) | `session_id`, `message` (the queued message details) |
| `ReloadStarted` | Runtime config or extension reload begins | — |
| `ReloadCompleted` | Runtime reload completes | — |
| `RuntimeError` | An internal runtime error occurs | `message` |

---

## Consumption Patterns

### CLI

The `gestalt runtime events` command subscribes to the event bus and streams formatted events to stdout. This is the primary way users observe runtime behavior.

### Diagnostics

`RuntimeInspect` provides a snapshot of the runtime's configuration and registered capabilities, sourced from the `RuntimeRegistry`. The event bus history can be consumed for post-hoc analysis.

```rust
let inspect = runtime.inspect();
println!("{:#?}", inspect);
```

### Tracing / Hooks

The `on_event` composition hook receives `AgentEvent` instances (via `OnEventCtx`), enabling hook-based observation of the core agent loop. The `RuntimeTraceHookAdapter` funnels trace events from the core loop into the composition hooks' `on_event` handler through an `mpsc::unbounded_channel`.

### Testing

Integration tests can subscribe to the event bus to assert that specific events were published during a session:

```rust
let mut rx = event_bus.subscribe();
runtime.run_prompt(input).await?;
// Assert specific events occurred
while let Ok(event) = rx.try_recv() {
    match &*event {
        RuntimeEvent::HookCompleted { hook_name, .. } => { /* ... */ }
        RuntimeEvent::SessionSpawned { .. } => { /* ... */ }
        _ => {}
    }
}
```

The `history()` method is particularly useful in tests for collecting all events after a run completes without worrying about channel capacity.

---

## Thread Safety

| Component | Mechanism |
|-----------|-----------|
| **Broadcast channel** (`tx`) | `tokio::sync::broadcast` is multi-producer, multi-consumer. Clone the `RuntimeEventBus` to share across threads/tasks. |
| **History buffer** | `Mutex<Vec<RuntimeEvent>>` — serializes concurrent writes and reads. The lock is held only for the duration of the push/clone. |
| **Sequence counter** | `AtomicU64` with `Ordering::SeqCst` — lock-free, safe across concurrent `publish_agent` calls. |
| **Event ownership** | `Arc<RuntimeEvent>` — zero-copy sharing of events across subscribers. |

---

## Integration with `AgentEvent`

The core `AgentEvent` type (defined in `gestalt-core/src/event.rs`) is bridged into the runtime event bus through `publish_agent()`:

```rust
// In runtime.rs, the agent loop callback:
let event_bus = self.event_bus.clone();
loop_.run(
    session,
    cancel_token,
    trace_sink,
    |event| {
        event_bus.publish_agent(event.clone());
        // ...
    },
);
```

This ensures every core agent event (tool calls, model responses, errors, etc.) is available in the runtime event stream with a deterministic sequence number, enabling ordered replay and correlation with runtime-level events.
