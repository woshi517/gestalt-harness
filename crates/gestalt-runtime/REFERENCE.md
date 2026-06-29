# Gestalt Runtime API Reference (`gestalt-runtime`)

This document serves as the comprehensive API reference for the `gestalt-runtime` crate. It provides detailed signatures, descriptions, configuration defaults, and invariant explanations for all major traits and structs within the runtime composition layer.

---

## 1. Orchestration & Control API

### `RuntimeControl` (Trait)
The runtime-scoped control trait for inspecting an already-built runtime, reloading its extension composition, and querying generation/health state. It is implemented by `AgentRuntime`, `RuntimeHost`, and `DefaultAgentRuntimeHandle`.

```rust
#[async_trait]
pub trait RuntimeControl: Send + Sync {
    /// Inspect the runtime configuration, registered capabilities, active extensions, and skills.
    async fn inspect_runtime(&self) -> RuntimeInspect;

    /// Initiate a transaction-safe reload of extensions.
    /// Can dry-run to validate candidates or execute a live reload incrementing the generation.
    async fn reload_extensions(
        &self,
        request: ReloadExtensionsRequest,
    ) -> Result<ReloadExtensionsReport>;

    /// Returns the current runtime generation sequence number.
    fn current_generation(&self) -> RuntimeGeneration;

    /// Inspect the health status of active extension instances.
    fn extension_health(&self) -> Vec<ExtensionInstanceHealth>;

}
```

### `HostControl` (Trait)
The host-scoped orchestration trait for multi-session control. Only host-oriented types implement this trait; `AgentRuntime` does not expose these methods.

```rust
#[async_trait]
pub trait HostControl: Send + Sync {
    async fn spawn_session(
        &self,
        session_id: &str,
        config_override: Option<RuntimeConfig>,
    ) -> Result<String>;
    async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult>;
    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck>;
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Arc<RuntimeEvent>>;
    fn artifact_store(&self) -> Arc<dyn ArtifactStore>;
    async fn create_artifact(&self, session_id: &str, name: &str, content: &[u8])
        -> Result<String>;
    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>>;
    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>>;
    async fn respond_to_approval(
        &self,
        approval_id: &str,
        decision: gestalt_core::approval::ApprovalDecision,
    ) -> Result<()>;
}
```

### `AgentRuntimeHandle` (Trait)
A compatibility trait used by orchestrators. It combines runtime-scoped inspection/reload with host-scoped session orchestration.

```rust
#[async_trait]
pub trait AgentRuntimeHandle: RuntimeControl + HostControl {}
```

### `RuntimeHost` (Struct)
`RuntimeHost` is the single host boundary for one workspace and one extension-generation lineage. It owns the shared `ExtensionManager`, discovery source, session registry, event bus, artifact store, and approval broker.

### `DefaultAgentRuntimeHandle` (Struct)
Thread-safe compatibility adapter over `Arc<RuntimeHost>`.

```rust
pub struct DefaultAgentRuntimeHandle {
    // Builder used to clone and construct fresh sessions
    builder: AgentRuntimeBuilder,
    // Active session runtimes mapped by session ID
    runtimes: Arc<Mutex<HashMap<String, Arc<AgentRuntime>>>>,
    // Pluggable artifact store
    artifact_store: Arc<dyn ArtifactStore>,
    // Broadcaster for all runtime and agent events
    event_bus: RuntimeEventBus,
}
```

---

## 2. Core Runtime & Builder

### `AgentRuntime` (Struct)
The primary execution context wrapping the pure `AgentLoop` from `gestalt-core`.

#### Key Methods

- **`pub async fn run_prompt(&self, input: UserInput) -> Result<RunResult>`**
  Spawns a fresh session, builds context, and executes the prompt.
- **`pub async fn run_session(&self, session: &mut Session, cancel_token: &CancelToken, ...)`**
  Drives execution on an existing mutable `Session` session instance, coordinating policies, context assembly, and tools.
- **`pub fn inspect(&self) -> RuntimeInspect`**
  Computes a full diagnostic inspection snapshot.

---

### `AgentRuntimeBuilder` (Struct)
A fluent builder for constructing `AgentRuntime` instances. It enforces required dependencies and registers extensions.

```rust
pub struct AgentRuntimeBuilder {
    pub provider: Option<Arc<dyn Provider>>,
    pub tools: Option<Arc<dyn ToolCatalog>>,
    pub middleware: Option<Arc<dyn ContextPipeline>>,
    pub assembler: Option<Arc<dyn ContextAssembler>>,
    pub policy: Option<Arc<dyn PolicyEngine>>,
    pub approval: Option<Arc<dyn ApprovalProvider>>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub config: RuntimeConfig,
    pub hooks: HookRegistry,
    pub registry: RuntimeRegistry,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub extensions: Vec<Arc<dyn GestaltExtension>>,
    pub extension_packages: Vec<ResolvedExtensionPackage>,
    pub pending_process_extensions: Vec<PendingProcessExtension>,
    pub extension_manager: Option<Arc<ExtensionManager>>,
    pub event_bus: RuntimeEventBus,
}
```

#### Builder Methods

- **`provider(Arc<dyn Provider>)`**: Bind the LLM client backend.
- **`tools(Arc<dyn ToolCatalog>)`**: Bind base capabilities catalog.
- **`assembler(Arc<dyn ContextAssembler>)`**: Bind prompt assembly component.
- **`policy(Arc<dyn PolicyEngine>)`**: Bind agent decision policy framework.
- **`approval(Arc<dyn ApprovalProvider>)`**: Bind human-in-the-loop action gate.
- **`config(RuntimeConfig)`**: Pass configuration.
- **`extension_package(ResolvedExtensionPackage)`**: Register resolved extension packages.
- **`build() -> Result<AgentRuntime>`**: Validates config, constructs the base composition, and activates configured extension packages through `ExtensionActivationPipeline`.
- **`async fn build_async() -> Result<AgentRuntime>`**: Converts pending legacy manifests into resolved packages and uses the same activation pipeline as `build()`.

### Activation and Reload Invariants

- Startup and reload both execute through `ExtensionActivationPipeline`.
- `ActivationCandidate` owns newly-started resources until commit. Dropping an uncommitted candidate rolls them back.
- `RuntimeSnapshotLease` pins a generation for the duration of a run. Retired generations are drained only after the last lease is released.
- `ResolvedExtensionPackage::trust` is explicit and independent of `manifest_hash`; discovery and trust policy are applied separately.
- `RuntimeExtensionSnapshot` carries executable lifecycle client handles, and `run_session()` only dispatches lifecycle hooks against the pinned snapshot clients.
- `ManagedExtensionResource` tracks a `ReuseKey`; retirement compares reuse keys so replaced components can drain correctly.
- `HostLaunchContext` carries host network policy and trusted extension IDs so permission checks can enforce the host layer alongside manifests and grants.
- `ExtensionManager::combined_health()` merges immutable snapshot diagnostics with live process state.

### Lifecycle Protocol v2

`InitializeResponseV2` reports the negotiated protocol version together with an explicit `supports_cancellation` flag. Cancellation is best-effort when declared and is surfaced through the negotiated lifecycle client rather than as an untyped hook outcome.

---

### `RuntimeConfig` (Struct)
Session-level configuration settings.

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
    pub enabled_host_features: Vec<String>,
    pub tool_profile: Option<ToolProfile>,
    pub mcp_servers: HashMap<String, gestalt_mcp::McpServerConfig>,
    pub mcp_discovery_threshold: Option<usize>,
    pub extension_instances: BTreeMap<String, ExtensionInstanceConfig>,
    pub effective_config_fingerprint: Option<String>,
}
```

---

## 3. Fingerprinting & Inspection

### `RuntimeInspect` (Struct)
Diagnostic snapshot of the runtime configuration and registered extension capacities.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeInspect {
    pub runtime_generation: u64,
    pub runtime_fingerprint: Option<String>,
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
    pub enabled_host_features: Vec<String>,
    pub discovered_skills: Vec<SkillInspectInfo>,
    pub active_skills: Vec<String>,
    pub skill_fingerprint: Option<String>,
    pub mcp_servers: Vec<gestalt_mcp::McpServerState>,
    pub mcp_discovery_threshold: Option<usize>,
    pub effective_config_fingerprint: Option<String>,
    pub variant_fingerprint: Option<String>,
    pub negotiated_protocol_fingerprint: Option<String>,
}
```

---

### Fingerprint Calculations

The Gestalt substrate ensures hot-reload stability and security auditability by computing deterministic hashes representing all loaded state:

#### 1. Snapshot Fingerprint (`compute_complete_fingerprint`)
Folds in the base registry configuration with all resolved extension packages and components, computing a final SHA-256 hash. The fingerprint incorporates direct MCP server configuration, declared and resolved entrypoints, package trust state, cancellation declarations, permissions/grants/configuration, lifecycle declarations, and dependency lock/executable hashes. Non-file entrypoints such as `python -m ...` or `python -c ...` are distinguished by their declared command/argument material even when no executable file hash exists.

```rust
pub fn compute_complete_fingerprint(
    registry_fingerprint: &str,
    resolved_packages: &[ResolvedExtensionPackage],
) -> String;
```

#### 2. Policy Fingerprint (`compute_policy_fingerprint`)
SHA-256 hash of the active system security policies (`policies.toml` or JSON config).
```rust
pub fn compute_policy_fingerprint(policies_content: &str) -> String;
```

#### 3. Hook Contract Fingerprint (`compute_hook_contract_hash`)
SHA-256 hash of sorted hook names.
```rust
pub fn compute_hook_contract_hash(hook_names: &[String]) -> String;
```

#### 4. Dependency Lock & Executable Helpers (`manager`)
Utilities to scan and hash binary and configuration dependencies inside package folders:
- **`compute_dependency_lock_hash(source_root: &Path) -> Option<String>`**
  Looks for and hashes `Cargo.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `poetry.lock`, `uv.lock`, or `requirements.txt`.
- **`compute_executable_hash(source_root: &Path, entry_cmd: &str, args: &[String]) -> Option<String>`**
  Hashes the entrypoint executable file and script arguments.

---

## 4. Extension Substrate & Sandboxing

### Manifests
The extension system supports two manifest variants:
- **`ExtensionManifest`**: Legacy v1 schema specifying standard single-broker properties.
- **`ExtensionManifestV2`**: Canonical multi-component schema supporting multiple kinds of components (`CommandTool`, `McpServer`, `GestaltLifecycle`, etc.).

### Sandboxing & Verification
All filesystem, network, and shell permissions declared in manifests are validated at load time and enforced at runtime:

- **`check_path_permission(manifest: &ExtensionManifest, workspace_root: &Path, path: &Path, write: bool, event_bus: &RuntimeEventBus) -> Result<(), String>`**
  Performs path validation, directory traversal blocking (`..`), and checks matches against `allow_workspace_read`, `allow_workspace_write`, and `allowed_paths`.
- **`check_network_permission(manifest: &ExtensionManifest, host: &str, event_bus: &RuntimeEventBus) -> Result<(), String>`**
  Gates remote queries against host strings and host matchers in `allow_network`.
- **`check_shell_permission(manifest: &ExtensionManifest, event_bus: &RuntimeEventBus) -> Result<(), String>`**
  Rejects commands requiring shell execution unless explicitly allowed.

---

## 5. Composition Hooks

### `CompositionHooks` (Trait)
Exposes hooks that intercept execution points in the main `AgentLoop`.

```rust
#[async_trait]
pub trait CompositionHooks: Send + Sync {
    /// Intercept context construction before system prompt rendering.
    async fn before_context_build(&self, context: &BeforeContextBuildCtx) -> Result<HookOutcome>;

    /// Modify or evaluate context packet after prompt generation.
    async fn after_context_build(&self, context: &AfterContextBuildCtx) -> Result<HookOutcome>;

    /// Enforce policies or reject tool execution request prior to running a tool.
    async fn before_tool_policy(&self, context: &BeforeToolPolicyCtx) -> Result<HookOutcome>;

    /// Intercept and modify tool output before presenting to the agent.
    async fn after_tool_result(&self, context: &AfterToolResultCtx) -> Result<HookOutcome>;

    /// Intercept execution right before transitioning turns.
    async fn prepare_next_turn(&self, context: &PrepareNextTurnCtx) -> Result<HookOutcome>;

    /// Event listener hook to monitor execution telemetry.
    async fn on_event(&self, context: &OnEventCtx) -> Result<()>;
}
```

#### Hook Outcomes (`HookOutcome`)
Allows hooks to influence the agent execution flow:

```rust
pub enum HookOutcome {
    /// Proceed normally.
    Continue,
    /// Terminate execution (aborts turn or blocks tool call).
    Block { reason: String },
    /// Inject a context message.
    AddContext { message: gestalt_core::message::Message },
    /// Attach metadata to the outcome.
    Annotate { metadata: serde_json::Value },
    /// Override the active model/provider.
    SwitchModel { model: String, provider: Option<String> },
}
```
