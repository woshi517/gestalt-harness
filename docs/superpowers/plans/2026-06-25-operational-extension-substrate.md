# Operational Extension Substrate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn PR #22 from extension-domain scaffolding into an operational substrate that can load, configure, execute, inspect, and safely replace extension capabilities while keeping `gestalt-core` product-neutral.

**Architecture:** `gestalt-runtime` owns runtime composition through immutable `RuntimeExtensionSnapshot` generations. `AgentRuntime::run_session()` pins one snapshot and uses its tool catalog, lifecycle plans, MCP registry, process instances, and fingerprint for the whole run. `ExtensionManager` owns discovery, instance resolution, process lifecycle, reuse, transactional reload, draining, and shutdown; host applications interact through one `RuntimeControl` surface.

**Tech Stack:** Rust, Tokio, JSON-RPC over stdio, existing `gestalt-core` `AgentLoop`, existing `gestalt-runtime` registry/composition hooks/MCP/event bus, cargo tests plus `cargo fmt`, `cargo clippy`, and workspace test CI.

---

## File Structure

- Modify `crates/gestalt-runtime/src/extension/runtime_snapshot.rs`: make snapshots carry every executable domain needed by a run, plus package/component identity and complete fingerprint inputs.
- Modify `crates/gestalt-runtime/src/runtime.rs`: pin `RuntimeExtensionSnapshot` at session start and build `AgentLoop`, context hooks, policy, inspection, and MCP access from that snapshot.
- Modify `crates/gestalt-runtime/src/builder.rs`: extract composition construction into reusable code used by initial build and reload; route configured package instances through one resolver.
- Modify `crates/gestalt-runtime/src/control.rs`: replace placeholder reload with transactional candidate construction and extend `RuntimeControl` to include the orchestration methods currently on `AgentRuntimeHandle`.
- Modify `crates/gestalt-runtime/src/orchestration.rs`: move or delegate `AgentRuntimeHandle` behavior into `RuntimeControl`, then keep a compatibility adapter for existing orchestration tests and downstream callers.
- Modify `crates/gestalt-runtime/src/extension/manager.rs`: make `ExtensionManager` own inventory, configured instances, launch, reuse, draining, in-flight tracking, health, and snapshot publication.
- Modify `crates/gestalt-runtime/src/extension/launcher.rs`: implement `LocalProcessLauncher` using process-backed clients instead of returning "not wired".
- Modify `crates/gestalt-runtime/src/extension/process_instance.rs`: connect process state, broker/client ownership, in-flight guards, draining, and shutdown.
- Modify `crates/gestalt-runtime/src/lifecycle/client.rs` and add `crates/gestalt-runtime/src/lifecycle/process_client.rs`: implement concrete lifecycle protocol v2 JSON-RPC calls for `capabilities/describe` and `lifecycle/invoke`.
- Modify `crates/gestalt-runtime/src/process_extension.rs`: keep v1/v1.1 compatibility, but separate common stdio broker plumbing from legacy protocol assumptions so protocol 2.0 is accepted through the v2 client path.
- Modify `crates/gestalt-runtime/src/extension/command_tool.rs`: include configured instance identity in command-tool namespace and schema name/canonical ID so multiple instances do not collide.
- Modify `crates/gestalt-runtime/src/discovery.rs` and `crates/gestalt-runtime/src/extension/package.rs`: preserve manifest path/hash/source root, apply `gestalt.json` instance selection/config/grants, and calculate executable/content fingerprints.
- Modify `crates/gestalt-cli/src/runtime.rs`: replace legacy `discover_all -> process_extension` construction with `discover_packages -> resolve configured instances -> build runtime`.
- Add or modify tests under `crates/gestalt-runtime/tests/`: snapshot pinning, transactional reload, lifecycle v2 process fixture, instance resolution, process reuse/drain, fingerprint changes, multi-instance command tools, runtime control consolidation.
- Modify `.github/workflows/*.yml` or add `.github/workflows/ci.yml`: run formatting, clippy, and workspace tests for every PR.

---

### Task 1: Make Runtime Snapshots Authoritative for Runs

**Files:**
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/extension/runtime_snapshot.rs`
- Modify: `crates/gestalt-runtime/src/registry/snapshot.rs`
- Test: `crates/gestalt-runtime/tests/runtime_snapshot_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_run_tests.rs`

- [ ] **Step 1: Write failing tests for snapshot-pinned tools and inspection**

Add tests proving that a session uses the active snapshot captured at run start, not `self.tools`, and that runtime inspection reports the active generation and fingerprint from `ExtensionManager`.

Test cases:
- `run_session_uses_active_snapshot_tool_catalog`
- `run_session_does_not_switch_tools_after_reload_mid_run`
- `inspect_reads_tools_from_active_snapshot`

Run:

```bash
cargo test -p gestalt-runtime run_session_uses_active_snapshot_tool_catalog run_session_does_not_switch_tools_after_reload_mid_run inspect_reads_tools_from_active_snapshot
```

Expected before implementation: at least the pinned catalog test fails because `AgentLoop::new()` still receives `self.tools.clone()`.

- [ ] **Step 2: Add executable snapshot accessors**

Extend `RuntimeExtensionSnapshot` with helper methods:

```rust
impl RuntimeExtensionSnapshot {
    pub fn tool_catalog(&self) -> Arc<dyn ToolCatalog> {
        self.tool_catalog.clone()
    }

    pub fn mcp_registry(&self) -> Arc<gestalt_mcp::McpRegistry> {
        self.mcp_registry.clone()
    }
}
```

Add a `registry_snapshot: RuntimeRegistrySnapshot` field to the snapshot and include it in `from_registry_snapshot()` so context contributor metadata and inspection data are pinned with the executable catalog.

- [ ] **Step 3: Change `AgentRuntime::run_session()` to use the pinned snapshot**

At the top of `run_session()`, bind the snapshot once:

```rust
let active_extension_snapshot = self.extension_manager.active_snapshot();
let pinned_tools = active_extension_snapshot.tool_catalog();
let pinned_mcp_registry = active_extension_snapshot.mcp_registry();
```

Then pass `pinned_tools` to `AgentLoop::new()` instead of `self.tools.clone()`.

- [ ] **Step 4: Build context and policy adapters from the pinned snapshot**

Replace direct reads from `self.registry.context_contributors` with snapshot-derived contributor registrations. For the first implementation, keep native `CompositionHooks` in the adapter but source contributor ordering and lifecycle membership from `active_extension_snapshot.context_plan`.

The invariant is:

```text
All context providers, policy guards, routers, verifiers, observers, MCP registry, process instances, and tool catalog consulted during a run come from active_extension_snapshot.
```

- [ ] **Step 5: Update inspection to read snapshot tools**

In `inspect()`, replace:

```rust
let schemas = self.tools.schemas();
```

with:

```rust
let active_extension_snapshot = self.extension_manager.active_snapshot();
let tools = active_extension_snapshot.tool_catalog();
let schemas = tools.schemas();
```

Use the same `tools` handle for backend descriptor lookup.

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime runtime_snapshot_tests runtime_run_tests
```

Expected: snapshot pinning tests pass, existing runtime run tests continue passing.

---

### Task 2: Route CLI and Builder Through Package Discovery and Configured Instances

**Files:**
- Modify: `crates/gestalt-runtime/src/discovery.rs`
- Modify: `crates/gestalt-runtime/src/extension/package.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Modify: `crates/gestalt-cli/src/runtime.rs`
- Test: `crates/gestalt-runtime/tests/extension_instance_config_tests.rs`
- Test: `crates/gestalt-cli/tests/config_tests.rs`
- Test: `crates/gestalt-cli/tests/runtime_cli_tests.rs`

- [ ] **Step 1: Write failing tests for operational instance resolution**

Add tests proving:
- `gestalt.json` can instantiate the same package twice with different instance IDs.
- Disabled instances do not register components.
- Component selection disables individual components.
- Instance `config` and `grants` flow into resolved components.
- CLI runtime construction uses `discover_packages()` rather than `discover_all()`.

Run:

```bash
cargo test -p gestalt-runtime extension_instance_config_tests
cargo test -p gestalt-cli config_tests runtime_cli_tests
```

Expected before implementation: instance parsing tests pass, but operational resolution tests fail because config is not connected to runtime construction.

- [ ] **Step 2: Preserve manifest source metadata**

Extend discovered package data so each package has:

```rust
pub struct DiscoveredExtensionPackage {
    pub manifest_path: PathBuf,
    pub source_root: PathBuf,
    pub package: ResolvedExtensionPackage,
    pub manifest_hash: String,
    pub enabled: bool,
}
```

Set `source_root` to the manifest parent directory.

- [ ] **Step 3: Add an instance resolver**

Add a resolver function in `extension/package.rs` or a new focused file `extension/instance_resolver.rs`:

```rust
pub fn resolve_configured_instances(
    discovered: &[DiscoveredExtensionPackage],
    configured: &BTreeMap<String, ExtensionInstanceConfig>,
) -> Result<Vec<ResolvedExtensionPackage>>
```

Rules:
- If no configured instances exist, preserve compatibility by enabling one default instance per discovered package.
- If configured instances exist, only enabled configured instances are activated.
- `ExtensionInstanceConfig.package` must match a discovered package ID.
- Component overrides are keyed by component ID; absent component entries default to enabled.
- Instance config and grants are copied into each selected component.
- Manifest hash, source root, package version, and effective instance ID are retained for fingerprinting.

- [ ] **Step 4: Route builder through resolved packages**

Make `AgentRuntimeBuilder::build_inner()` consume `self.config.extension_instances` through the resolver before registering extension package components. Do not let CLI-specific code perform package/instance policy decisions.

- [ ] **Step 5: Update CLI runtime construction**

In `crates/gestalt-cli/src/runtime.rs`, replace the legacy path:

```text
discover_all -> trust by manifest ID/hash -> builder.process_extension
```

with:

```text
discover_packages -> builder.extension_packages(discovered packages) -> builder config applies configured instances
```

Keep v1 manifests working by normalizing them into packages.

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime extension_instance_config_tests extension_manifest_v2_tests extension_conformance_tests
cargo test -p gestalt-cli config_tests runtime_cli_tests
```

Expected: configured instances are operationally reflected in runtime tools, lifecycle plans, and health.

---

### Task 3: Fix Multi-Instance Command Tool Identity

**Files:**
- Modify: `crates/gestalt-runtime/src/extension/command_tool.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Test: `crates/gestalt-runtime/tests/command_tool_tests.rs`
- Test: `crates/gestalt-runtime/tests/extension_instance_config_tests.rs`

- [ ] **Step 1: Write failing collision tests**

Add tests:
- `command_tools_include_instance_in_namespace`
- `two_instances_of_same_command_tool_register_independently`

Expected before implementation: duplicate bare component names collide.

- [ ] **Step 2: Include instance identity in tool descriptor**

Change `CommandTool` fields from `package_id` and `name` only to:

```rust
package_id: String,
instance_id: String,
component_id: String,
runtime_name: String,
```

Use a stable runtime name such as:

```rust
format!("{}__{}", component.id.instance_id, component.id.component_id)
```

Keep the canonical descriptor namespace explicit:

```rust
ToolNamespace::Extension(format!("{}:{}", self.package_id, self.instance_id))
```

- [ ] **Step 3: Register by runtime name**

In `builder.rs`, register command tools with `tool.name().to_string()` instead of `component.id.component_id`.

- [ ] **Step 4: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime command_tool_tests extension_instance_config_tests
```

Expected: two configured instances of one command-tool package produce two independent callable tools with distinct canonical IDs.

---

### Task 4: Implement Lifecycle Protocol v2 Process Client

**Files:**
- Add: `crates/gestalt-runtime/src/lifecycle/process_client.rs`
- Modify: `crates/gestalt-runtime/src/lifecycle/mod.rs`
- Modify: `crates/gestalt-runtime/src/lifecycle/client.rs`
- Modify: `crates/gestalt-runtime/src/process_extension.rs`
- Modify: `crates/gestalt-runtime/src/extension/launcher.rs`
- Add fixture: `crates/gestalt-runtime/tests/fixtures/protocol-v2/lifecycle_server.py`
- Test: `crates/gestalt-runtime/tests/lifecycle_protocol_v2_tests.rs`

- [ ] **Step 1: Write end-to-end protocol fixture test**

Add a fixture process that:
- responds to `initialize` with `{"negotiated_version":"2.0"}`;
- responds to `capabilities/describe` with context, policy, verifier, router, and observer descriptors;
- responds to `lifecycle/invoke` by echoing the capability payload inside a typed response;
- responds to `shutdown`.

Add tests:
- `protocol_v2_process_describes_capabilities`
- `protocol_v2_process_invokes_lifecycle_capability`
- `protocol_v2_rejects_unsupported_version`

Expected before implementation: process-backed v2 tests fail because the current broker rejects protocol `"2.0"`.

- [ ] **Step 2: Extract common stdio JSON-RPC broker behavior**

Keep legacy v1/v1.1 behavior intact, but separate:
- spawn and stdio read/write;
- bounded message parsing;
- pending request tracking;
- timeout and cancellation;
- shutdown.

This common broker must not hard-code `tools/*`, `context/*`, and `hooks/*` as the only meaningful protocol methods.

- [ ] **Step 3: Implement `ProcessLifecycleClient`**

Add:

```rust
pub struct ProcessLifecycleClient {
    broker: Arc<ProcessExtensionBroker>,
}
```

Implement `LifecycleClient` by calling:
- `initialize` with `InitializeRequestV2 { supported_versions: vec!["2.0".to_string()] }`;
- `capabilities/describe`;
- `lifecycle/invoke`;
- `shutdown`.

- [ ] **Step 4: Accept protocol 2.0 only on lifecycle components**

In the process launch path, allow `"2.0"` for `ComponentKind::GestaltLifecycle`. Keep legacy manifest paths limited to existing v1/v1.1 behavior.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime lifecycle_protocol_v2_tests
```

Expected: lifecycle protocol v2 is proven against a real child process, not just constants.

---

### Task 5: Make ExtensionManager Own Process Lifecycle, Reuse, Draining, and Health

**Files:**
- Modify: `crates/gestalt-runtime/src/extension/manager.rs`
- Modify: `crates/gestalt-runtime/src/extension/launcher.rs`
- Modify: `crates/gestalt-runtime/src/extension/process_instance.rs`
- Modify: `crates/gestalt-runtime/src/process_extension.rs`
- Test: `crates/gestalt-runtime/tests/extension_manager_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_process_extension_tests.rs`

- [ ] **Step 1: Write failing manager ownership tests**

Add tests:
- `manager_reuses_ready_process_for_same_reuse_key`
- `manager_launches_new_process_when_fingerprint_changes`
- `manager_marks_old_process_draining_after_publish`
- `draining_process_rejects_new_calls_and_waits_for_in_flight`
- `manager_shutdown_terminates_owned_processes`

Expected before implementation: reuse and draining tests fail because `ExtensionProcessInstance` does not own broker/client calls.

- [ ] **Step 2: Store process-backed clients inside `ExtensionProcessInstance`**

Extend `ExtensionProcessInstance` so it owns one of:

```rust
pub enum ExtensionProcessBackend {
    Legacy(Arc<ProcessExtension>),
    LifecycleV2(Arc<dyn LifecycleClient>),
    McpRegistry(Arc<gestalt_mcp::McpRegistry>),
}
```

The MCP variant owns the registry selected for this snapshot; calls still go through named MCP clients inside `gestalt_mcp::McpRegistry`.

- [ ] **Step 3: Connect in-flight guards**

Add methods:

```rust
pub async fn begin_call(&self) -> Result<InFlightGuard>;
pub async fn begin_drain(&self);
pub async fn wait_for_drain(&self, timeout: Duration) -> Result<()>;
pub async fn shutdown(&self) -> Result<()>;
```

Rules:
- Ready instances accept calls and increment in-flight count.
- Draining instances reject new calls.
- Shutdown waits for in-flight calls up to configured timeout, then terminates.

- [ ] **Step 4: Implement `LocalProcessLauncher::launch()`**

Launch based on `ExtensionRuntimeComponent.kind`:
- `LegacyProcess`: spawn legacy broker and wrap it.
- `GestaltLifecycle`: spawn v2 lifecycle client and describe capabilities.
- `McpServer`: create or reuse MCP registry/client configuration through the snapshot build path.
- `CommandTool`: no persistent process; command tools remain per-call tools.

- [ ] **Step 5: Fix legacy reuse**

In `launch_legacy_process_extension()`, return the existing ready backend when the reuse key matches. Do not always spawn a new process after detecting reuse.

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime extension_manager_tests runtime_process_extension_tests
```

Expected: `ExtensionManager` is the sole owner of extension process lifecycle and reuse.

---

### Task 6: Implement Transactional Reload Candidate Reconstruction

**Files:**
- Modify: `crates/gestalt-runtime/src/control.rs`
- Modify: `crates/gestalt-runtime/src/extension/manager.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Modify: `crates/gestalt-runtime/src/event_bus.rs`
- Test: `crates/gestalt-runtime/tests/extension_reload_tests.rs`

- [ ] **Step 1: Replace generation-clone tests with transactional tests**

Add tests:
- `reload_dry_run_builds_candidate_without_publishing`
- `reload_rejects_invalid_candidate_and_keeps_old_generation`
- `reload_publishes_candidate_atomically`
- `reload_instance_id_limits_reconstruction_scope`
- `reload_force_restarts_even_reusable_instances`
- `reload_drains_old_generation_after_publish`

Expected before implementation: tests fail because reload only clones the active snapshot and appends a generation suffix to the fingerprint.

- [ ] **Step 2: Add reload candidate data structure**

In `ExtensionManager`, add:

```rust
pub struct ReloadCandidate {
    pub generation: RuntimeGeneration,
    pub snapshot: Arc<RuntimeExtensionSnapshot>,
    pub reused: Vec<ComponentInstanceId>,
    pub launched: Vec<ComponentInstanceId>,
    pub validation_errors: Vec<String>,
}
```

- [ ] **Step 3: Reconstruct candidate from discovery and config**

Under the reload mutex:
- rediscover packages;
- resolve configured instances;
- compute component fingerprints;
- reuse matching ready process instances unless `force` applies;
- launch changed lifecycle/process/MCP components;
- build command tools and MCP config;
- build lifecycle plans from v2 `capabilities/describe`;
- check tool name conflicts and component conflicts;
- build a new `RuntimeExtensionSnapshot`.

- [ ] **Step 4: Publish atomically**

Only call `publish_snapshot()` after the full candidate validates and all required processes are ready. On error, shut down newly launched candidate processes and leave the active snapshot unchanged.

- [ ] **Step 5: Drain old generation**

After publish:
- mark old non-reused processes draining;
- allow active sessions that pinned the old snapshot to finish;
- shut down drained processes.

- [ ] **Step 6: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime extension_reload_tests extension_manager_tests runtime_snapshot_tests
```

Expected: reload is transactional, `dry_run`, `force`, and `instance_id` are meaningful, and failed reloads preserve the previous generation.

---

### Task 7: Complete Fingerprints for Safe Reuse and Replay

**Files:**
- Modify: `crates/gestalt-runtime/src/extension/manager.rs`
- Modify: `crates/gestalt-runtime/src/extension/package.rs`
- Modify: `crates/gestalt-runtime/src/extension/runtime_snapshot.rs`
- Modify: `crates/gestalt-runtime/src/registry/snapshot.rs`
- Test: `crates/gestalt-runtime/tests/runtime_snapshot_tests.rs`
- Test: `crates/gestalt-runtime/tests/extension_manager_tests.rs`

- [ ] **Step 1: Write failing fingerprint tests**

Add tests proving fingerprints change when these inputs change:
- package version;
- manifest hash;
- executable content hash;
- dependency lock hash when present;
- effective instance config;
- grants;
- trust;
- negotiated protocol;
- MCP backend config;
- lifecycle descriptors.

- [ ] **Step 2: Add package source identity**

Carry these fields through resolved packages/components:

```rust
manifest_hash: String,
source_root: PathBuf,
package_version: String,
executable_hash: Option<String>,
dependency_lock_hash: Option<String>,
```

For local files, hash the entrypoint target and common lock files in the package root when present, for example `uv.lock`, `poetry.lock`, `package-lock.json`, `pnpm-lock.yaml`, `Cargo.lock`, and `requirements.txt`.

- [ ] **Step 3: Expand `ComponentFingerprint`**

Hash:
- component canonical ID;
- component kind;
- optional flag;
- package version;
- manifest hash;
- executable hash;
- dependency lock hash;
- entrypoint command and args;
- effective instance config;
- grants;
- trust;
- negotiated protocol;
- backend-specific MCP/lifecycle descriptors.

- [ ] **Step 4: Expand runtime fingerprint**

Hash:
- all tool schemas and canonical IDs;
- context, policy, routing, verification, and observer plans;
- MCP server configuration;
- package IDs, versions, instance IDs, component IDs;
- process component fingerprints;
- grants and trust;
- negotiated protocol and executable identity.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime runtime_snapshot_tests extension_manager_tests
```

Expected: a script/content/dependency/config/protocol change produces a new reuse key and runtime fingerprint.

---

### Task 8: Consolidate RuntimeControl and AgentRuntimeHandle

**Files:**
- Modify: `crates/gestalt-runtime/src/control.rs`
- Modify: `crates/gestalt-runtime/src/orchestration.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/lib.rs`
- Test: `crates/gestalt-runtime/tests/runtime_control_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_orchestration_tests.rs`

- [ ] **Step 1: Write failing API consolidation tests**

Add tests proving a single `RuntimeControl` can:
- start a session;
- send or enqueue a message;
- subscribe to events;
- access artifacts;
- steer a run;
- inspect runtime;
- reload extensions;
- request or resolve approval.

- [ ] **Step 2: Move orchestration methods into `RuntimeControl`**

Extend `RuntimeControl` with the operational methods currently split across runtime and `AgentRuntimeHandle`. Keep method names close to existing orchestration tests to avoid churn.

- [ ] **Step 3: Implement compatibility adapter**

If downstream tests still require `AgentRuntimeHandle`, implement it as a thin adapter over `Arc<dyn RuntimeControl>` and mark it as compatibility-only in docs.

- [ ] **Step 4: Add runtime-owned approval broker hook**

Add the smallest runtime-owned approval interface needed for control-plane clients to observe and answer approval requests. Existing approval providers remain compatible, but new hosts should use `RuntimeControl`.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test -p gestalt-runtime runtime_control_tests runtime_orchestration_tests
```

Expected: `RuntimeControl` is the one host-facing control surface; `AgentRuntimeHandle` no longer competes as a second partial API.

---

### Task 9: Update Docs, Claims, and CI

**Files:**
- Modify: `docs/extension-development-guide.md`
- Modify: `docs/jsonrpc-extension-protocol.md`
- Modify: `docs/gestalt-harness-architecture.md`
- Modify: `.github/workflows/ci.yml`
- Test: workspace verification commands

- [ ] **Step 1: Correct architecture docs**

Update docs so they claim only implemented behavior:
- lifecycle v2 is process-backed;
- hot reload is transactional;
- configured instances are operational;
- snapshots are authoritative;
- `RuntimeControl` is unified.

- [ ] **Step 2: Add CI workflow**

Ensure PR CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- [ ] **Step 3: Run local final verification**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all commands pass locally and in CI.

---

## Acceptance Checklist

- [ ] `run_session()` builds execution from one pinned `RuntimeExtensionSnapshot`.
- [ ] Reload reconstructs a candidate through rediscovery, instance resolution, validation, launch, conflict checks, atomic publication, and old-generation draining.
- [ ] Lifecycle protocol v2 has a real process-backed client tested against a child process.
- [ ] CLI construction flows through package discovery and configured instances.
- [ ] `ExtensionManager` owns process launch, reuse, in-flight calls, draining, health, and shutdown.
- [ ] Component and runtime fingerprints include executable behavior, package identity, config, grants, protocol, and backend configuration.
- [ ] Multiple instances of one command-tool package can coexist without name or state collisions.
- [ ] `RuntimeControl` is the unified host boundary for sessions, messages, events, artifacts, steering, inspection, reload, and approvals.
- [ ] Compatibility adapters keep v1 extensions working.
- [ ] `gestalt-core` remains product-neutral and is not used as an extension substrate dumping ground.
- [ ] CI runs formatting, clippy, and workspace tests.

## Self-Review

Spec coverage:
- Blocking finding 1 is covered by Task 1.
- Blocking finding 2 is covered by Task 6.
- Blocking finding 3 is covered by Task 4.
- Blocking finding 4 is covered by Task 5.
- Blocking finding 5 is covered by Task 2.
- Blocking finding 6 is covered by Task 7.
- Blocking finding 7 is covered by Task 3.
- Blocking finding 8 is covered by Task 8.
- CI/test concerns are covered by Task 9.

Residual sequencing risk:
- Task 6 depends on Tasks 2, 4, 5, and 7 for a complete reload path. If execution needs smaller PRs, land Tasks 1-3 first as the snapshot/config/tool identity foundation, then Tasks 4-7 as process/reload work, then Tasks 8-9 as control/docs/CI.
