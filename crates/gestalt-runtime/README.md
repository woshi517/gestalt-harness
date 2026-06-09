# Gestalt Runtime Crate (`gestalt-runtime`)

The `gestalt-runtime` crate is the **Runtime Composition Layer** of the Gestalt agent harness. It wraps `AgentLoop` (from `gestalt-core`) with concrete implementations of providers, tool registries, policy engines, context pipelines, trace hooks, and a full **process-backed extension system** that lets child processes register tools, hooks, and context contributors via JSON-RPC 2.0 over stdio.

This crate is the primary integration point for:
- Building an `AgentRuntime` with all required dependencies
- Loading and managing **process extensions** (child processes that communicate over stdio)
- Implementing **composition hooks** to intercept and modify the agent lifecycle
- Registering **context contributors** that inject system messages into the context packet
- Applying **permission sandboxing** (filesystem, network, shell) against extension manifests
- Composing base tools with extension-provided tools via `ComposedToolCatalog`
- Coordinating multi-session orchestration with `ArtifactStore` and `AgentRuntimeHandle`

---

## Architecture Overview

`gestalt-runtime` sits between applications (CLI, TUI, SDK clients) and the core agent execution engine:

```mermaid
graph TD
    App[CLI / TUI / SDK Client] -->|invokes| Runtime[gestalt-runtime::AgentRuntime]
    Runtime -->|composed of| Registry[RuntimeRegistry]
    Runtime -->|manages| Adapters[Hook Adapters]
    Adapters -->|wrap| Hooks[CompositionHooks]
    Runtime -->|orchestrates| AgentLoop[gestalt-core::AgentLoop]

    subgraph Extensions[Process-Backed Extensions]
        Broker[ProcessExtensionBroker]
        Tools[ProcessBackedTool]
        Ctx[ProcessBackedContextContributor]
        Discovery[ExtensionDiscovery]
        Manifest[gestalt.extension.toml]
    end

    Discovery -->|spawns| Broker
    Broker -->|wraps| Tools
    Broker -->|wraps| Ctx
    Manifest -->|discovered by| Discovery
    Registry -->|registers| Tools
    Registry -->|registers| Ctx

    subgraph Core Boundaries [gestalt-core (Pure)]
        AgentLoop
    end
```

---

## Quick Start

Construct a runtime using the builder pattern:

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

---

## Core Interfaces

### `AgentRuntime`

The main execution boundary wrapping `gestalt-core::AgentLoop`. It owns the full dependency graph — provider, tool catalog, middleware, policy engine, approval provider, and event bus — and provides two execution paths:

```rust
pub struct AgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<dyn ToolCatalog>,
    pub middleware: Arc<dyn ContextPipeline>,
    pub policy: Arc<dyn PolicyEngine>,
    pub approval: Arc<dyn ApprovalProvider>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub config: RuntimeConfig,
    pub registry: RuntimeRegistry,
    pub hooks: HookRegistry,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub event_bus: RuntimeEventBus,
}
```

Key methods:
- `run_prompt(input: UserInput)` — Starts a fresh session, captures a workspace snapshot, pushes the user message onto the history, and runs `run_session`.
- `run_session(session: &mut Session, cancel_token: &CancelToken, event_tx: ...)` — Continues execution on an existing mutable `Session`. This method wires up composition hooks, context contributors, the `RuntimePolicyEngine`, and the `RuntimeTraceHookAdapter` before delegating to `AgentLoop::run`.
- `inspect() -> RuntimeInspect` — Returns a diagnostic snapshot of the runtime's configuration and registered capabilities.

### `AgentRuntimeBuilder`

Fluent builder that validates all required dependencies are present and wires extensions before constructing the `AgentRuntime`:

```rust
pub struct AgentRuntimeBuilder {
    pub provider: Option<Arc<dyn Provider>>,
    pub tools: Option<Arc<dyn ToolCatalog>>,
    pub middleware: Option<Arc<dyn ContextPipeline>>,
    pub policy: Option<Arc<dyn PolicyEngine>>,
    pub approval: Option<Arc<dyn ApprovalProvider>>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub config: RuntimeConfig,
    pub hooks: HookRegistry,
    pub registry: RuntimeRegistry,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub extensions: Vec<Arc<dyn GestaltExtension>>,
    pub event_bus: RuntimeEventBus,
}
```

Builder methods:

| Method | Description |
|--------|-------------|
| `.provider(Arc<dyn Provider>)` | Sets the LLM provider (required) |
| `.tools(Arc<dyn ToolCatalog>)` | Sets the base tool catalog (required) |
| `.middleware(Arc<dyn ContextPipeline>)` | Sets the context pipeline (required) |
| `.policy(Arc<dyn PolicyEngine>)` | Sets the base policy engine (required) |
| `.approval(Arc<dyn ApprovalProvider>)` | Sets the approval provider (required) |
| `.trace_sink(Arc<dyn TraceSink>)` | Sets an optional trace sink |
| `.config(RuntimeConfig)` | Sets the runtime configuration |
| `.composition_hooks(Arc<dyn CompositionHooks>)` | Sets user-defined composition hooks |
| `.extension(Arc<dyn GestaltExtension>)` | Adds a single extension |
| `.extensions(Vec<...>)` | Adds multiple extensions |
| `.hooks(HookRegistry)` | Sets core hook registry |
| `.build() -> Result<AgentRuntime>` | Validates and constructs the runtime |

During `build()`:
1. Each extension is registered (duplicate names are rejected).
2. Base tools are composed with extension tools via `ComposedToolCatalog`.
3. User hooks and extension hooks are composed via `ComposedCompositionHooks`.

### `RuntimeConfig`

```rust
pub struct RuntimeConfig {
    pub workspace_root: PathBuf,
    pub execution_mode: ExecutionMode,
    pub max_turns: usize,
    pub model: String,
    pub provider: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub max_context_window: Option<usize>,
    pub reserved_output_tokens: Option<usize>,
    pub bash_timeout_secs: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub allow_network: bool,
    pub environment: HashMap<String, String>,
    pub enabled_cli_features: Vec<String>,
}
```

Defaults:

| Field | Default |
|-------|---------|
| `workspace_root` | `env::current_dir()` or `"."` |
| `execution_mode` | `ExecutionMode::Confirm` |
| `max_turns` | `10` |
| `model` | `""` (uses provider default) |
| `provider` | `""` |
| `max_tokens` | `4096` |
| `temperature` | `Some(0.0)` |
| `max_context_window` | `None` (falls back to 120,000) |
| `reserved_output_tokens` | `None` (falls back to 8,000) |
| `bash_timeout_secs` | `None` (falls back to 60) |
| `max_output_tokens` | `None` (falls back to 4,000) |
| `allow_network` | `false` |
| `environment` | empty `HashMap` |
| `enabled_cli_features` | empty `Vec` |

---

## Runtime Registry

The `RuntimeRegistry` stores all named capabilities registered by extensions and user code:

```rust
pub struct RuntimeRegistry {
    pub tools: BTreeMap<String, ToolMetadata>,
    pub providers: BTreeMap<String, ProviderMetadata>,
    pub context_contributors: BTreeMap<String, ContextContributorMetadata>,
    pub verifiers: Vec<String>,
    pub hooks: Vec<String>,
    pub extensions: Vec<String>,
}
```

**`ToolMetadata`** — A registered tool with its JSON schema, SHA-256 schema hash, optional executable `Tool` implementation, and optional owning extension ID:

```rust
pub struct ToolMetadata {
    pub name: String,
    pub schema: ToolSchema,
    pub schema_hash: String,
    pub tool: Option<Arc<dyn Tool>>,
    pub extension_id: Option<String>,
}
```

**`ProviderMetadata`** — A named provider factory:

```rust
pub type ProviderFactory = Arc<
    dyn Fn(serde_json::Value) -> Result<Arc<dyn Provider>, HarnessError> + Send + Sync,
>;

pub struct ProviderMetadata {
    pub name: String,
    pub factory: ProviderFactory,
}
```

**`ContextContributorMetadata`** — A named context contributor optionally scoped to an extension:

```rust
pub struct ContextContributorMetadata {
    pub name: String,
    pub contributor: Arc<dyn ContextContributor>,
    pub extension_id: Option<String>,
}
```

### Registry Methods

| Method | Description |
|--------|-------------|
| `register_tool(name, schema)` | Registers a tool name + schema (no executable impl) |
| `register_executable_tool(name, schema, tool, extension_id)` | Registers a tool with an executable `Arc<dyn Tool>` |
| `register_provider(name, factory)` | Registers a named provider factory |
| `register_context_contributor(name, contributor)` | Registers a context contributor |
| `register_executable_context_contributor(name, contributor, extension_id)` | Registers a context contributor with extension scoping |
| `register_verifier(name)` | Registers a verifier name |
| `register_hook(name)` | Registers a hook name |
| `register_extension(name)` | Registers an extension name |

All registration methods enforce uniqueness — duplicate names return `RuntimeError::Registry`.

Schema hashing utilities:
- `compute_schema_hash(schema: &serde_json::Value) -> String` — SHA-256 of the JSON-serialized schema.
- `compute_tool_schema_hash(schemas: &[ToolSchema]) -> String` — SHA-256 over sorted schemas for deterministic fingerprinting.

---

## Process-Backed Extensions

The extension system is the primary mechanism for extending the runtime with **out-of-process plugins** — child processes that communicate via JSON-RPC 2.0 over stdio.

### Concept

Instead of compiling Rust code into the runtime, extensions run as separate OS processes. Each extension declares its capabilities in a `gestalt.extension.toml` manifest. The `ProcessExtensionBroker` spawns the process, performs a JSON-RPC 2.0 initialization handshake, and routes tool calls / context injections / hook invocations over the stdio channel.

### Extension Manifest (`gestalt.extension.toml`)

```toml
id = "mock-ext"
name = "Mock Stdio Extension"
version = "0.1.0"
runtime = "stdio"

[entrypoint]
command = "./mock_ext.sh"

[capabilities]
tools = true
hooks = false
context = true

[permissions]
allow_network = []
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = []

[[tools]]
name = "bash_tool"
description = "Brings hello from bash"
input_schema = { type = "object" }

[[context_injectors]]
name = "bash_context"
```

**Schema:**

```rust
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: String,
    pub entrypoint: Entrypoint,
    pub capabilities: Capabilities,
    pub permissions: Permissions,
    pub tools: Vec<ToolDeclaration>,
    pub hooks: Vec<HookDeclaration>,
    pub context_injectors: Vec<ContextInjectorDeclaration>,
}

pub struct Entrypoint {
    pub command: String,
    pub args: Vec<String>,
}

pub struct Capabilities {
    pub tools: bool,
    pub hooks: bool,
    pub context: bool,
}

pub struct Permissions {
    pub allow_network: Vec<String>,
    pub allow_workspace_read: bool,
    pub allow_workspace_write: bool,
    pub allow_shell: bool,
    pub allow_all_paths: bool,
    pub allowed_paths: Vec<String>,
}

pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub risk: Option<String>,
    pub read_only: Option<bool>,
    pub idempotent: Option<bool>,
}

pub struct HookDeclaration {
    pub name: String,
    pub lifecycle_point: String,
}

pub struct ContextInjectorDeclaration {
    pub name: String,
}
```

### Manifest Validation Rules

`ExtensionManifest::validate()` enforces:

| Rule | Error |
|------|-------|
| `id` must not be empty | `"Extension ID cannot be empty"` |
| `name` must not be empty | `"Extension Name cannot be empty"` |
| `runtime` must be `"stdio"` | `"Unsupported runtime: '{runtime}'. Only 'stdio' is supported"` |
| `entrypoint.command` must not be empty | `"Entrypoint command cannot be empty"` |
| Tools declared but `capabilities.tools` is `false` | `"Extension declares tools but capabilities.tools is false"` |
| Hooks declared but `capabilities.hooks` is `false` | `"Extension declares hooks but capabilities.hooks is false"` |
| Context injectors declared but `capabilities.context` is `false` | `"Extension declares context injectors but capabilities.context is false"` |
| Shell interpretation required but `allow_shell` is `false` | Rejects commands with spaces, `\|`, `&`, `;`, `>`, `<` |
| Entrypoint is a known shell (`sh`, `bash`, `zsh`, `ksh`, `csh`, `tcsh`, `cmd`, `powershell`, `pwsh`, `fish`) and `allow_shell` is `false` | `"Entrypoint command is a shell executable but allow_shell permission is false"` |

### Extension Discovery

`ExtensionDiscovery` resolves manifests through a three-tier lookup with deterministic ordering:

```rust
pub struct ExtensionDiscovery {
    workspace_root: PathBuf,
    global_dir: Option<PathBuf>,
}

impl ExtensionDiscovery {
    pub fn new(workspace_root: PathBuf, global_dir: Option<PathBuf>) -> Self;
    pub fn discover_all(&self, explicit_paths: &[PathBuf]) -> Result<Vec<DiscoveredExtension>>;
}
```

**Lookup order (first wins for duplicate IDs):**
1. **Explicit paths** — Paths passed by the CLI or application code (highest priority). If a path is a directory, `gestalt.extension.toml` is looked up inside it.
2. **Project-local** — `{workspace_root}/.gestalt/extensions/`. Each subdirectory containing `gestalt.extension.toml` is loaded.
3. **Global** — `{global_dir}/extensions/` (typically `~/.config/gestalt/extensions/`). Same directory-per-extension convention.

Duplicate extension IDs are silently deduplicated — the first encounter wins.

Each discovered extension includes a SHA-256 hash of the raw manifest content:

```rust
pub struct DiscoveredExtension {
    pub manifest_path: PathBuf,
    pub manifest: ExtensionManifest,
    pub manifest_hash: String,
    pub enabled: bool,
}
```

### Trust Model

- **Explicit paths** are trusted by default (user explicitly passed them).
- **Global extensions** are trusted by default (user installed them globally).
- **Project-local extensions** require a `trusted` allowlist (not yet implemented in MVP — currently all discovered extensions are marked `enabled: true`).

### `ProcessExtensionBroker` Lifecycle

The broker manages the full lifecycle of a stdio-backed extension process:

```rust
pub struct ProcessExtensionBroker {
    manifest: ExtensionManifest,
    event_bus: RuntimeEventBus,
    tx: mpsc::Sender<(JsonRpcRequest, oneshot::Sender<Result<JsonRpcResponse, String>>)>,
    child: Arc<Mutex<Option<Child>>>,
}
```

**Lifecycle:**

1. **Shell Permission Check** — Refuses to spawn if the entrypoint requires shell interpretation but `allow_shell` is `false`.
2. **Environment Isolation** — Calls `cmd.env_clear()` then selectively inherits only safe environment variables: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TMPDIR`, `TEMP`, `TMP`.
3. **Spawn** — Pipes `stdin`, `stdout`, `stderr`. Sets `kill_on_drop(true)`.
4. **Stderr Drainer** — A background tokio task reads stderr and publishes `RuntimeEvent::ExtensionError` for each line.
5. **JSON-RPC Loop** — A background task reads JSON-RPC 2.0 responses from stdout and dispatches them to pending request channels.
6. **Initialize Handshake** — Sends `{"jsonrpc":"2.0","method":"initialize","params":{"capabilities":...,"version":"..."}}`. If the handshake fails, the process is killed and `RuntimeEvent::ExtensionRejected` is published.
7. **RPC Dispatch** — `call(method, params)` sends a JSON-RPC request with a UUID request ID and awaits the response with a 30-second timeout.
8. **Shutdown** — Kills the child process and publishes `RuntimeEvent::ProcessExited`.

### `ProcessBackedTool`

Wraps an extension's tool declaration as a `gestalt_core::tool::Tool` implementation:

```rust
pub struct ProcessBackedTool {
    broker: Arc<ProcessExtensionBroker>,
    name: String,
    description: String,
    schema: ToolSchema,
    risk: RiskLevel,
}
```

On `execute(input, ctx)`:
1. Verifies `capabilities.tools` is enabled.
2. Scans the JSON input for path-like keys (`path`, `file`, `dir`, `dest`, `src`, `target`, `output`) and URL-like keys (`url`, `host`, `uri`, `address`) and calls `check_path_permission` / `check_network_permission` against the extension's manifest permissions.
3. Sends `{"method":"tools/call","params":{"name":"...","input":...}}` via the broker.
4. Returns the response `content` field as `ToolOutput::Text`.

### `ProcessBackedContextContributor`

Wraps an extension's context injector as a `ContextContributor`:

```rust
pub struct ProcessBackedContextContributor {
    broker: Arc<ProcessExtensionBroker>,
    name: String,
}
```

On `contribute(workspace_root)`:
1. Verifies `capabilities.context` is enabled.
2. Checks path permission against the workspace root.
3. Sends `{"method":"context/inject","params":{"name":"..."}}` via the broker.
4. Returns the response `content` as a `Message::System`.

### `ProcessExtension`

The bridge between the extension system and the `GestaltExtension` trait:

```rust
pub struct ProcessExtension {
    pub manifest: ExtensionManifest,
    pub broker: Arc<ProcessExtensionBroker>,
}
```

`ProcessExtension::register()` iterates over the manifest's `tools`, `context_injectors`, and `hooks`, creating `ProcessBackedTool`, `ProcessBackedContextContributor`, and registering them in the `RuntimeRegistry`. Tool risk levels default to `High` when unspecified.

### `GestaltExtension` Trait

The abstract trait all extensions implement:

```rust
pub trait GestaltExtension: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()>;
    fn as_process_extension(&self) -> Option<&ProcessExtension> {
        None
    }
}
```

The `as_process_extension()` method enables the composition hooks system to dispatch RPC calls to extension-backed hooks.

---

## Permissions & Sandboxing

The permissions module enforces the sandbox declared in the extension manifest's `Permissions` block.

### Filesystem Path Gating

```rust
pub fn check_path_permission(
    manifest: &ExtensionManifest,
    workspace_root: &Path,
    path: &Path,
    write: bool,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), String>;
```

Logic:
1. If `allow_all_paths` is `true`, all paths are permitted.
2. Canonicalize the workspace root and the target path to prevent traversal attacks.
3. If the path is **within** the workspace root:
   - Read access requires `allow_workspace_read`.
   - Write access requires `allow_workspace_write`.
4. If the path is **outside** the workspace root, check against `allowed_paths` list (canonicalized prefix match).
5. Publishes a `RuntimeEvent::PermissionDecision` event for auditability.

### Network Host Allowlists

```rust
pub fn check_network_permission(
    manifest: &ExtensionManifest,
    host: &str,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), String>;
```

Logic:
- Iterates `allow_network` entries.
- A value of `"*"` permits all hosts.
- Otherwise, an exact string match is required.
- URL-like inputs are parsed with the `url` crate; `host_str()` is extracted for matching.

### Shell Command Blocking

```rust
pub fn check_shell_permission(
    manifest: &ExtensionManifest,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), String>;
```

Simply reads `allow_shell` — if `false`, shell execution is denied. This is evaluated at manifest validation time for the entrypoint and at runtime for tool inputs.

### Environment Isolation

When spawning extension processes, `ProcessExtensionBroker`:
1. Calls `cmd.env_clear()` — no parent environment variables are inherited by default.
2. Selectively allows only: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TMPDIR`, `TEMP`, `TMP`.

### Runtime-Level Network Gating

The `RuntimeConfig.allow_network` flag gates network access for all tool executions at the session level (separate from per-extension manifest permissions). This is passed into `ToolContext` and consumed by tools at runtime.

---

## Composition Hooks (`CompositionHooks`)

The `CompositionHooks` trait allows you to intercept key lifecycle points of the agent loop:

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

### Context Structs

| Struct | Fields |
|--------|--------|
| `BeforeContextBuildCtx` | `session_id: String`, `history: Vec<Message>` |
| `AfterContextBuildCtx` | `session_id: String`, `history: Vec<Message>`, `packet: ContextPacket` |
| `BeforeToolPolicyCtx` | `session_id: String`, `tool_name: String`, `tool_input: serde_json::Value` |
| `AfterToolResultCtx` | `session_id: String`, `tool_name: String`, `result: ToolExecutionResult` |
| `PrepareNextTurnCtx` | `session_id: String`, `history: Vec<Message>`, `turn_index: usize`, `current_model: String`, `current_provider: String` |
| `OnEventCtx` | `session_id: String`, `event: AgentEvent` |

### Hook Outcomes (`HookOutcome`)

```rust
pub enum HookOutcome {
    Continue,
    Block { reason: String },
    AddContext { message: Message },
    Annotate { metadata: serde_json::Value },
    SwitchModel { model: String, provider: Option<String> },
}
```

- **`Continue`** — Proceed with normal execution.
- **`Block { reason }`** — Abort execution. In context hooks, aborts the entire turn. In tool policy hooks, blocks the specific tool call with `PolicyDecision::Denied`.
- **`AddContext { message }`** — Injects a custom `Message` into the context packet (inserted after the first `Message::System`).
- **`Annotate { metadata }`** — Adds custom metadata to the hook outcome (consumed by observers).
- **`SwitchModel { model, provider? }`** — V1-only narrow override. Applies to the next request only and is then cleared automatically.

### Hook Adapters

The runtime wires composition hooks into the core `AgentLoop` through three adapter types:

- **`RuntimeContextHookAdapter`** — Implements `gestalt_core::hook::ContextHook`. Runs `before_context_build` / `after_context_build` and invokes all registered `ContextContributor` instances. Manages a `patch_store` (the context injection buffer) and a `block_reason` signal that the session loop polls.
- **`RuntimeToolHookAdapter`** — Implements `gestalt_core::hook::ToolHook`. Runs `before_tool_policy` / `after_tool_result`.
- **`RuntimeTraceHookAdapter`** — Implements `gestalt_core::hook::TraceHook`. Funnels `AgentEvent`s through a `tokio::sync::mpsc::UnboundedSender` to the `on_event` handler.

### `ComposedCompositionHooks`

Composes user-defined hooks with extension-provided hooks. User hooks run first; if they return non-`Continue`, the result short-circuits and extension hooks are skipped. Extension hooks dispatch to the child process via `pe.broker.call("hooks/call", ...)` with the relevant lifecycle context.

---

## Context Contributors

The `ContextContributor` trait allows registering components that inject system messages into the context packet before the LLM prompt is constructed:

```rust
#[async_trait]
pub trait ContextContributor: Send + Sync {
    fn name(&self) -> &str;
    async fn contribute(&self, workspace_root: &Path) -> Result<Message>;
}
```

Contributors are invoked during the `before_context_build` phase of `RuntimeContextHookAdapter`. Their returned `Message` objects are pushed into the `patch_store`, which is consumed by `RuntimeContextPipeline` during context building.

```rust
pub struct RuntimeContextPipeline {
    pub base: Arc<dyn ContextPipeline>,
    pub patch_store: Arc<Mutex<Vec<Message>>>,
}
```

The pipeline wraps the base `ContextPipeline`, injecting patch store messages after the first `Message::System` in the processed message list, and recalculating the packet hash and message hashes.

Contributors can be:
- **User-defined** — Implement `ContextContributor` directly and register via `registry.register_context_contributor`.
- **Process-backed** — Declared in an extension manifest's `[[context_injectors]]` and wrapped in `ProcessBackedContextContributor`.

---

## Tool Composition

### `ComposedToolCatalog`

Merges base tools with extension-provided tools, detecting name collisions:

```rust
pub struct ComposedToolCatalog {
    base: Arc<dyn ToolCatalog>,
    extension_tools: BTreeMap<String, Arc<dyn Tool>>,
}
```

- **Duplicate detection** — `ComposedToolCatalog::new()` checks that no extension tool name collides with a base tool name. Returns `Err(String)` on collision.
- **Deterministic ordering** — `schemas()` concatenates base schemas and extension schemas, then sorts alphabetically by tool name.
- **Precedence** — `get(name)` checks extension tools first, then falls back to the base catalog.

The catalog is constructed automatically by `AgentRuntimeBuilder::build()` from `self.registry.tools` entries that have an executable `tool` field.

---

## Runtime Events

### `RuntimeEventBus`

A publish-subscribe event bus built on `tokio::sync::broadcast`:

```rust
pub struct RuntimeEventBus {
    tx: broadcast::Sender<Arc<RuntimeEvent>>,
    next_seq: Arc<AtomicU64>,
    history: Arc<Mutex<Vec<RuntimeEvent>>>,
}
```

| Method | Description |
|--------|-------------|
| `new()` | Creates a bus with a 4096-capacity broadcast channel |
| `publish(event)` | Appends to history and broadcasts to all subscribers |
| `publish_agent(agent_event)` | Wraps in `RuntimeEvent::Agent` with auto-incrementing sequence number |
| `subscribe()` | Returns a new `broadcast::Receiver` |
| `history()` | Returns a snapshot of all events published so far |

### `RuntimeEvent` Enum

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Agent { sequence_number: u64, event: AgentEvent },
    ExtensionDiscovered { extension_id: String, manifest_path: String, manifest_hash: String },
    ExtensionLoaded { extension_id: String },
    ExtensionRejected { extension_id: String, reason: String },
    ExtensionError { extension_id: String, message: String },
    HookStarted { hook_name: String, lifecycle_point: String },
    HookCompleted { hook_name: String, lifecycle_point: String, outcome: String },
    HookFailed { hook_name: String, lifecycle_point: String, error: String },
    ToolRegistered { extension_id: Option<String>, tool_name: String, schema_hash: String },
    ContextInjectorRegistered { extension_id: Option<String>, injector_name: String },
    PermissionDecision { extension_id: String, capability: String, permission: String, resource: Option<String>, granted: bool, reason: Option<String> },
    ProcessSpawned { extension_id: String, pid: u32 },
    ProcessExited { extension_id: String, exit_code: Option<i32> },
    ProcessKilled { extension_id: String, reason: String },
    RpcRequest { extension_id: String, method: String, request_id: String },
    RpcResponse { extension_id: String, method: String, request_id: String, success: bool },
    ArtifactRouted { session_id: String, path: String, size_bytes: usize },
    SessionSpawned { session_id: String },
    ReloadStarted,
    ReloadCompleted,
    RuntimeError { message: String },
}
```

Key variants and their semantics:

| Variant | When Published |
|---------|---------------|
| `ExtensionDiscovered` | Manifest found and parsed during discovery |
| `ExtensionLoaded` | ProcessExtensionBroker handshake succeeded |
| `ExtensionRejected` | Validation failure, spawn failure, or handshake failure |
| `ExtensionError` | Line from extension's stderr |
| `ProcessSpawned` | Extension child process started (includes PID) |
| `ProcessExited` | Extension child process terminated (includes exit code) |
| `PermissionDecision` | Every `check_path_permission`, `check_network_permission`, `check_shell_permission` call |
| `RpcRequest` / `RpcResponse` | Every JSON-RPC call and response |
| `ToolRegistered` / `ContextInjectorRegistered` | Extension registration in the registry |
| `HookStarted` / `HookCompleted` / `HookFailed` | Each lifecycle hook invocation |
| `SessionSpawned` | New agent session started |

---

## Orchestration & Multi-Session

Provides abstractions for coordinating multiple agent sessions and passing artifacts between them.

### `ArtifactStore`

A pluggable interface for saving and retrieving files produced by or fed to sessions:

```rust
pub trait ArtifactStore: Send + Sync {
    fn put_artifact(&self, session_id: &str, name: &str, content: &[u8]) -> Result<String>;
    fn get_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>>;
    fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>>;
}
```

Implementations:
- `InMemoryArtifactStore` — Thread-safe in-memory HashMap (ideal for testing).
- `FilesystemArtifactStore` — Persists artifacts on disk relative to a base workspace path with built-in path-traversal prevention.

### `AgentRuntimeHandle`

Exposed to orchestrators to control the lifecycle of individual sessions:

```rust
#[async_trait]
pub trait AgentRuntimeHandle: Send + Sync {
    async fn spawn_session(&self, session_id: &str, config_override: Option<RuntimeConfig>) -> Result<String>;
    async fn send_message(&self, session_id: &str, prompt: &str) -> Result<RunResult>;
    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>>;
    fn artifact_store(&self) -> Arc<dyn ArtifactStore>;
    async fn create_artifact(&self, session_id: &str, name: &str, content: &[u8]) -> Result<String>;
    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>>;
    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>>;
}
```

`DefaultAgentRuntimeHandle` implements this trait by cloning the `AgentRuntimeBuilder` for each session, supporting per-session configuration overrides.

### `Orchestrator`

A trait implemented by developers to compose workflows (e.g., writer-reviewer loops or multi-agent delegation chains):

```rust
#[async_trait]
pub trait Orchestrator: Send + Sync {
    async fn execute(&self, handle: Arc<dyn AgentRuntimeHandle>, task: OrchestrationTask) -> Result<OrchestrationResult>;
}

pub struct OrchestrationTask {
    pub prompt: String,
    pub input_artifacts: Vec<String>,
}

pub struct OrchestrationResult {
    pub output: String,
    pub output_artifacts: Vec<String>,
}
```

---

## Runtime Inspection

The `RuntimeInspect` struct provides a diagnostic snapshot of the runtime's configuration:

```rust
pub struct RuntimeInspect {
    pub provider_name: String,
    pub provider_model: String,
    pub execution_mode: String,
    pub max_turns: usize,
    pub context_pipeline_version: String,
    pub tools: Vec<ToolInspectInfo>,
    pub tool_schema_hash: String,
    pub policy_fingerprint: Option<String>,
    pub policy_source_path: Option<String>,
    pub hooks: Vec<String>,
    pub hook_contract_hash: String,
    pub verifiers: Vec<String>,
    pub extensions: Vec<String>,
    pub context_injectors: Vec<String>,
    pub trace_sink_kind: Option<String>,
    pub trace_run_dir: Option<String>,
    pub workspace_root: String,
    pub enabled_cli_features: Vec<String>,
}

pub struct ToolInspectInfo {
    pub name: String,
    pub schema_hash: String,
}
```

Obtained via `AgentRuntime::inspect()`.

**Fingerprinting functions:**

```rust
pub fn compute_hook_contract_hash(hook_names: &[String]) -> String;
pub fn compute_policy_fingerprint(policies_content: &str) -> String;
```

- `compute_hook_contract_hash` — SHA-256 of sorted hook names, delimited by `:`. Used to detect hook contract drift.
- `compute_policy_fingerprint` — SHA-256 of the raw policy file content. Computed from the `policies` key of `gestalt.json` (or legacy `.gestalt/policies.toml` if `gestalt.json` is absent).

---

## Runtime Policy Engine

`RuntimePolicyEngine` wraps a base `PolicyEngine` with composition hook evaluation:

```rust
pub struct RuntimePolicyEngine {
    pub base: Arc<dyn PolicyEngine>,
    pub hooks: Arc<dyn CompositionHooks>,
    pub session_id: String,
    pub event_bus: RuntimeEventBus,
}
```

On `evaluate(request)`:
1. Dispatches `before_tool_policy` to the composition hooks.
2. If the hook returns `HookOutcome::Block { reason }`, returns `PolicyDecision::denied(reason, "hook.before_tool_policy")`.
3. If the hook returns `Err`, returns `PolicyDecision::denied(...)` (fail-closed).
4. Otherwise, delegates to the base `PolicyEngine::evaluate(request)`.

This policy engine is instantiated per-session by `AgentRuntime::run_session`, scoped to the session ID.

---

## Safety and Lifecycle Invariants

### 1. Fail-Closed Security
- **Tool Policy Hooks:** Any `Err(...)` from `before_tool_policy` is treated as a security violation. The runtime fails closed, returning `PolicyDecision::Denied`.
- **Context Build Hooks:** If `before_context_build` or `after_context_build` returns `Block`, the session loop aborts on the next emitted event with a `PolicyError::Denied`.

### 2. Turn-to-Turn Context Accumulation
Context additions (`HookOutcome::AddContext`) returned by `after_context_build` on turn *N* are cached in the runtime's patch store and prepended to the system prompt on turn *N+1*.

When the `Snapshot` assembly strategy is active (see [ADR-026](../../docs/adrs/ADR-026-cache-aware-prompt-assembly.md)), each context addition carries a `ContextStability` tag. Stable patches (`SessionStatic`, `ActivationStatic`) are placed in the cacheable prefix before the provider cache breakpoint; dynamic and ephemeral patches (`TurnDynamic`, `Ephemeral`) follow in the uncached tail.

> [!NOTE]
> The patch store is cleared at the beginning of each turn's `after_context_build` phase. Context injected in one turn is applied to the next turn, but does not duplicate indefinitely.

### 3. Sequenced, Lossless Event Observation
Trace hook events dispatched to `on_event` are funneled through a `tokio::sync::mpsc::unbounded_channel`. This guarantees ordered delivery of all execution events. The runtime drains the channel before `run_session` returns, preventing loss of trailing events.

### 4. Extension Lifecycle
- Extension processes are spawned with `env_clear()` — no accidental credential leakage.
- Failed initialization handshakes trigger `ExtensionRejected` and the process is killed.
- The stderr drainer ensures all extension diagnostic output is captured as `ExtensionError` events.
- All JSON-RPC calls have a 30-second timeout; timed-out requests return an error and the pending oneshot channel is cleaned up.
- On broker drop, the child process is killed via `kill_on_drop(true)`.

### 5. Registry Uniqueness
All registry insertions (tools, providers, context contributors, hooks, verifiers, extensions) enforce uniqueness. Duplicate registration returns `RuntimeError::Registry`.

### 6. Tool Name Collisions
The `ComposedToolCatalog` constructor rejects extension tool names that collide with base tool names. This prevents ambiguity in the LLM's tool selection.
