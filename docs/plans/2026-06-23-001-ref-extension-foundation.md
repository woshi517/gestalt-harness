---
title: "refactor: Product-neutral extension architecture foundation"
type: refactor
status: proposed
date: 2026-06-23
origin: docs/feature-spec/product-neutral-extension-architecture.md
related:
  - docs/feature-spec/product-neutral-extension-architecture.md
  - docs/feature-spec/config-extension.md
  - docs/gestalt-harness-architecture.md
  - docs/adrs/ADR-023-runtime-composition-layer.md
  - docs/adrs/ADR-024-process-extensions.md
  - docs/adrs/ADR-027-mcp-client-integration.md

---

# Product-Neutral Extension Architecture Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor Gestalt's current static, universal extension model into a product-neutral runtime extension foundation with explicit packages, components, configured instances, immutable runtime snapshots, typed lifecycle capabilities, and transactional manual reload—without introducing client-specific behavior into the harness.

**Architecture:** `gestalt-core` remains unaware of manifests, package discovery, processes, reload, or client extensions. `gestalt-runtime` owns package normalization, component activation, runtime snapshots, lifecycle protocol adapters, process ownership, and reload. `gestalt-cli` owns human-facing configuration loading and commands but no longer owns process spawning. `gestalt-mcp` remains the canonical external tool integration. Existing `AgentRuntimeHandle` evolves into the application-neutral control boundary instead of introducing a second competing client API.

**Tech Stack:** Rust workspace crates (`gestalt-core`, `gestalt-runtime`, `gestalt-mcp`, `gestalt-cli`, `gestalt-verify`, `gestalt-trace`), serde/TOML/JSON schemas, JSON-RPC 2.0 over stdio, existing MCP registry, existing tool/policy/context abstractions, runtime event bus, process-backed extension fixtures.

---

## Summary

This plan intentionally implements a narrower foundation than the full feature specification.

The immediate refactor will:

- distinguish an extension **package**, **component**, **configured instance**, and **running process instance**;
- rename the current trusted native extension abstraction to `RuntimeModule`;
- preserve current extension manifests and JSON-RPC v1 through compatibility adapters;
- add a component-based manifest v2 without package dependencies;
- add namespaced extension instances to `gestalt.json`;
- split mutable registry construction from immutable runtime snapshots;
- pin one runtime generation for each `AgentLoop::run` invocation;
- move extension discovery and process ownership from CLI startup into `gestalt-runtime`;
- introduce a real `ExtensionManager`;
- replace generic external hooks with typed lifecycle capabilities;
- add a simple command-tool component;
- allow package-declared MCP components to normalize into the existing MCP path;
- implement transactional, blue/green, manual extension reload;
- extend the existing `AgentRuntimeHandle` into a minimal `RuntimeControl` boundary;
- parse and validate client/product component descriptors without executing client code.

The immediate refactor will **not** implement:

- a client/product code host;
- graphical panels, views, or renderers;
- package registry installation or updates;
- package dependencies;
- `gestalt.lock`;
- automatic filesystem watching;
- remote extension transports;
- state export/import across reload;
- OS sandboxing;
- removal of legacy `tools/call`;
- mid-turn or per-turn runtime-generation adoption.

The first implementation pins a runtime snapshot for one complete `AgentLoop::run` call. A reload can occur while a run is active, but the active run keeps its old generation and the next run adopts the new generation. This matches the current core architecture, where provider, tool catalog, context pipeline, policy engine, and executor are bound when `AgentLoop` is constructed.

---

# 1. Current Architecture Anchor

This plan is anchored to repository state at commit:

```text
d246119ecaf535ed9074a302467e452961001ba1
```

## 1.1 Existing architectural boundary

`ADR-023` already establishes `gestalt-runtime` as the composition layer above the pure kernel. It owns:

- `AgentRuntimeBuilder`;
- `RuntimeRegistry`;
- `RuntimeEventBus`;
- composition hooks;
- extension composition;
- runtime inspection;
- orchestration handles.

This remains the correct boundary. The refactor should deepen that separation rather than move extension concerns into `gestalt-core`.

## 1.2 Existing native extension abstraction

Current file:

```text
crates/gestalt-runtime/src/extension.rs
```

Current contract:

```rust
pub trait GestaltExtension: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()>;
    fn as_process_extension(&self) -> Option<&ProcessExtension>;
}
```

Problems:

- trusted native modules and installable process extensions share one name and interface;
- downcasting through `as_process_extension` leaks process-specific behavior into generic composition;
- the trait can register any registry category;
- package identity, process identity, and runtime module identity are conflated.

## 1.3 Existing registry

Current file:

```text
crates/gestalt-runtime/src/registry.rs
```

`RuntimeRegistry` is a mutable structure containing:

- tools;
- provider factories;
- context contributors;
- verifier names;
- hook names;
- extension names.

It provides deterministic maps for some categories and duplicate checks, but:

- it is both a construction API and a runtime inspection store;
- verifier and hook entries are only strings;
- extension identity is not namespaced by package, instance, and component;
- it does not represent an immutable executable generation;
- it cannot safely support candidate construction and atomic reload.

## 1.4 Existing builder and runtime ownership

Current files:

```text
crates/gestalt-runtime/src/builder.rs
crates/gestalt-runtime/src/runtime.rs
```

Current flow:

1. `AgentRuntimeBuilder` owns `Vec<Arc<dyn GestaltExtension>>`.
2. `build()` calls every extension's `register`.
3. The builder constructs one `ComposedToolCatalog`.
4. The builder constructs one `ComposedCompositionHooks`.
5. `AgentRuntime` stores static tools, middleware, policy, registry, hooks, MCP registry, and extension objects.
6. `run_session()` constructs `AgentLoop` from those static fields.

This is a startup composition model, not a reloadable runtime model.

## 1.5 Existing process-extension ownership

Current file:

```text
crates/gestalt-cli/src/runtime.rs
```

The CLI currently:

- discovers manifests;
- resolves trust;
- spawns process brokers;
- wraps them as `ProcessExtension`;
- inserts them into `AgentRuntimeBuilder`.

This makes the CLI responsible for runtime process lifecycle and prevents non-CLI embedders from getting the same lifecycle behavior without reproducing CLI wiring.

## 1.6 Existing manifest model

Current file:

```text
crates/gestalt-runtime/src/manifest.rs
```

The current manifest describes one process with:

- one entrypoint;
- boolean capabilities;
- requested permissions;
- tools;
- hooks;
- context injectors.

This is sufficient for protocol v1 compatibility, but it cannot model independently activatable runtime and client/product components.

## 1.7 Existing lifecycle model

Current file:

```text
crates/gestalt-runtime/src/composition_hooks.rs
```

The public model exposes internal hook points:

- `before_context_build`;
- `after_context_build`;
- `before_tool_policy`;
- `after_tool_result`;
- `prepare_next_turn`;
- `on_event`.

All use one generic `HookOutcome`, even where outcomes are semantically invalid.

The current runtime also invokes `before_tool_policy` through both:

- `RuntimePolicyEngine`;
- `RuntimeToolHookAdapter`.

This must be resolved before introducing a typed policy-guard contract.

## 1.8 Existing hot reload

The runtime event enum contains `ReloadStarted` and `ReloadCompleted`, but the CLI `extension reload` path only rediscovers/list-counts extensions. It does not:

- build a candidate registry;
- restart a process;
- publish a new runtime generation;
- drain old instances;
- update a live runtime.

## 1.9 Existing application-control boundary

Current file:

```text
crates/gestalt-runtime/src/orchestration.rs
```

`AgentRuntimeHandle` already supports:

- spawning sessions;
- sending messages;
- event subscription;
- artifact operations;
- steering messages.

This should evolve into `RuntimeControl`; a second parallel client API should not be introduced.

## 1.10 Existing core-loop constraint

Current file:

```text
crates/gestalt-core/src/agent.rs
```

`AgentLoop` binds:

- provider;
- context pipeline;
- tool executor;
- policy;
- approval provider;
- hooks;

for the duration of one `run()` invocation.

Therefore, the safe first-generation reload boundary is:

```text
one AgentLoop::run invocation = one runtime generation
```

Per-turn adoption would require a new dynamic component-resolution contract in `gestalt-core`. That is deliberately deferred until run-boundary reload has demonstrated a concrete limitation.

---

# 2. Scope Decisions and Open-Question Resolutions

## OQ-1: May profiles define extension configuration?

**Decision:** Profiles may eventually select extension instance IDs, but they must not contain inline extension configuration.

Canonical configuration remains:

```text
extensions.instances.<instance-id>.config
```

A future profile field may contain:

```json
{
  "extension_instances": ["research-default", "citation-checker"]
}
```

This plan does not add profile selection yet. All enabled instances apply to the effective runtime configuration.

**Reason:** Inline profile configuration would duplicate merge, provenance, validation, and permission semantics.

**Forward-compatibility note:** profile→instance selection is treated as **purely additive later**. U2 introduces `extensions.instances` as a `BTreeMap<String, ExtensionInstanceConfig>` with no reserved profile field. A future profile that selects instance IDs will be a separate top-level field (e.g. `extension_instances: Vec<instance-id>`) layered on top of the resolved instances map, not a reshape of `ExtensionInstanceConfig`. U2 therefore adds no profile hook, no reserved field, and no migration cost; implement `ExtensionsConfig` in U2 with this additive assumption and document it in `docs/feature-spec/config-extension.md`.

---

## OQ-2: Are client/product contributions declarative-only initially?

**Decision:** Yes.

This plan adds:

- component descriptors;
- compatibility metadata;
- declarative contribution metadata validation.

It does not load or execute client code.

**Reason:** No generic client host exists in the repository. Executing client code now would prematurely commit Gestalt to a UI language or application framework.

---

## OQ-3: Are package dependencies supported?

**Decision:** Deferred.

A package may contain multiple components, but cannot depend on another extension package in manifest v2.

**Reason:** Dependency resolution requires a package source model, lockfile, cycle handling, version conflict policy, and supply-chain policy. None are required for the runtime-boundary refactor.

---

## OQ-4: Do components have independent versions?

**Decision:** No. All components share the package version.

A component may expose its own protocol version, but not its own distribution version.

**Reason:** Independent component versions complicate activation, reload fingerprints, compatibility, and diagnostics without immediate value.

---

## OQ-5: How are configuration migrations executed?

**Decision:** Extension executable code does not run configuration migrations.

For this plan:

- extension config schemas may declare a schema version;
- incompatible versions fail validation with an actionable diagnostic;
- migration is manual or host-provided.

A later feature may add host-owned declarative transforms.

**Reason:** Running extension code over configuration before trust and activation creates an unsafe and circular lifecycle.

---

## OQ-6: How will remote lifecycle components work?

**Decision:** Reuse the same lifecycle protocol semantics over a future transport.

Remote worker orchestration remains a separate protocol responsible for:

- job submission;
- context transfer;
- event streaming;
- artifact transfer;
- worker lifecycle.

**Reason:** Lifecycle semantics should not change based on transport, but a worker protocol has responsibilities beyond extension invocation.

---

## OQ-7: How do optional components affect package health?

**Decision:**

- components are required by default;
- `optional = true` marks an optional component;
- required component failure rejects the extension instance or reload candidate;
- optional component failure activates the instance in `degraded` state;
- degraded state is visible in inspection and reload reports.

No silent skipping is allowed.

---

## OQ-8: How do direct MCP configuration and package-declared MCP coexist?

**Decision:** Support both and normalize them into the same candidate MCP configuration.

Sources:

```text
gestalt.json.mcp.servers
extension package mcp-server components
```

Rules:

- direct MCP names retain their configured names;
- package MCP names are canonicalized using package, instance, and component IDs;
- collisions are errors;
- both enter the same `McpRegistry` and canonical tool path.

---

## OQ-9: When is legacy process `tools/call` removed?

**Decision:** It is not removed by this plan.

After command-tool and MCP package components are available:

- new documentation recommends command tools or MCP;
- protocol v1 `tools/call` is labeled legacy;
- compatibility tests remain mandatory.

Removal requires a separate deprecation proposal and usage evidence.

---

## OQ-10: Which events enter stable client projection v1?

**Decision:** The stable projection contains categories, not every internal debug event:

- sequenced agent events;
- session lifecycle;
- approval lifecycle when emitted by the approval subsystem;
- artifacts;
- runtime-generation adoption;
- extension/package health;
- reload lifecycle;
- terminal/runtime errors.

Internal events such as every RPC request, raw hook invocation, or process stderr remain diagnostic events and are not guaranteed as stable client API.

---

# 3. Scope Boundaries

- Do not modify `gestalt-core` to understand extension packages or runtime generations.
- Do not introduce dynamic libraries.
- Do not add package dependencies.
- Do not add a package registry or installer.
- Do not add `gestalt.lock` in this plan.
- Do not execute client/product component code.
- Do not implement automatic watch mode.
- Do not implement remote transports.
- Do not implement state transfer across reload.
- Do not remove protocol v1.
- Do not move provider registration into untrusted process extensions.
- Do not let extension configuration grant authority.
- Do not add turn-level snapshot adoption.
- Do not preserve `GestaltExtension` as the long-term public name.
- Do not let CLI remain the owner of child extension processes.

**Naming note (applies throughout):** this plan refers to the CLI crate as `gestalt-cli` because that is the directory name (`crates/gestalt-cli/`). The crate's published `name` in its `Cargo.toml` is `gestalt-harness`. File paths in this plan (`crates/gestalt-cli/...`) are correct; any `cargo` invocation, dependency reference, or published artifact must use `gestalt-harness`. When U8 updates documentation, normalize these references so the crate name and directory name are not conflated.

---

# 4. Implementation Units

## U0. Characterize current behavior and remove lifecycle ambiguity

**Goal:** Establish regression coverage for the current v1 system and make policy-hook ownership unambiguous before structural refactoring.

**Requirements:** V1 behavior fixtures, exactly-once policy guard semantics, no behavior drift hidden inside later refactors.

**Dependencies:** None

**Files:**

- Modify: `crates/gestalt-runtime/src/composition_hooks.rs`
- Modify: `crates/gestalt-runtime/src/policy.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Test: `crates/gestalt-runtime/tests/runtime_process_extension_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_extension_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_run_tests.rs`
- Add: `crates/gestalt-runtime/tests/extension_refactor_baseline_tests.rs`

**Approach:**

- [ ] Add fixture tests for current manifest parsing, discovery ordering, trust-hash handling, handshake negotiation, tool registration, context registration, and hook registration.
- [ ] Add a regression test proving `before_tool_policy` currently has two potential invocation paths.
- [ ] Make `RuntimePolicyEngine` the sole owner of pre-policy extension evaluation.
- [ ] Remove pre-policy composition invocation from `RuntimeToolHookAdapter::before_tool_execution`.
- [ ] **Trace-compat note:** `RuntimeToolHookAdapter::before_tool_execution` currently publishes `HookStarted`/`HookCompleted`/`HookFailed` events keyed on `hook_name == "before_tool_policy"` (see `composition_hooks.rs`). Once this path stops invoking the hook, those events disappear from the tool-hook adapter. To keep the event surface stable for trace consumers, the policy-engine path (`RuntimePolicyEngine`) must continue publishing under the same `hook_name == "before_tool_policy"` so that downstream consumers keyed on that name still see exactly one event per policy decision. Update U0's tests to assert that the stable event surface (one `before_tool_policy` event per decision, attributed to the policy engine) survives the deduplication.
- [ ] Keep `RuntimeToolHookAdapter::after_tool_execution` for post-result behavior until typed lifecycle plans replace it.
- [ ] Add a test proving a blocking policy extension prevents execution and runs exactly once.
- [ ] Add a baseline test showing current CLI reload does not mutate a runtime; use this as the red test for U7.

**Patterns to follow:**

- Existing policy wrapper in `crates/gestalt-runtime/src/policy.rs`.
- Existing process-extension fixtures under `crates/gestalt-runtime/tests/fixtures/extensions/`.

**Test scenarios:**

- One extension hook invocation produces one policy decision.
- A failed closed pre-policy hook denies execution.
- An open post-result hook failure remains recoverable.
- Existing v1 process extension fixtures continue to start and register.

**Verification:**

```bash
cargo test -p gestalt-runtime extension_refactor_baseline_tests
cargo test -p gestalt-runtime runtime_process_extension_tests
cargo test -p gestalt-runtime runtime_extension_tests
cargo test -p gestalt-runtime runtime_run_tests
```

---

## U1. Introduce normalized package, component, and instance domain types

**Goal:** Separate installable package identity, component declarations, configured instances, trusted native modules, and running processes.

**Requirements:** Manifest v1 compatibility, component-based manifest v2, stable canonical identities, no client execution.

**Dependencies:** U0

**Files:**

- Replace: `crates/gestalt-runtime/src/extension.rs`
- Add: `crates/gestalt-runtime/src/extension/mod.rs`
- Add: `crates/gestalt-runtime/src/extension/package.rs`
- Add: `crates/gestalt-runtime/src/extension/component.rs`
- Add: `crates/gestalt-runtime/src/extension/instance.rs`
- Add: `crates/gestalt-runtime/src/extension/runtime_module.rs`
- Modify: `crates/gestalt-runtime/src/manifest.rs`
- Modify: `crates/gestalt-runtime/src/discovery.rs`
- Modify: `crates/gestalt-runtime/src/lib.rs`
- Test: `crates/gestalt-runtime/tests/runtime_extension_tests.rs`
- Add: `crates/gestalt-runtime/tests/extension_manifest_v2_tests.rs`
- Add fixtures: `crates/gestalt-runtime/tests/fixtures/extensions-v2/`

**Approach:**

- [ ] Rename the trusted in-process trait to:

```rust
pub trait RuntimeModule: Send + Sync {
    fn id(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistryBuilder) -> Result<()>;
}
```

- [ ] Keep a deprecated `GestaltExtension` compatibility alias or adapter for one release.
- [ ] Remove `as_process_extension`; process instances must be represented explicitly.
- [ ] Add normalized domain types:

```rust
ExtensionPackageDescriptor
ExtensionComponentDescriptor
ExtensionInstanceSpec
ComponentInstanceId
ResolvedExtensionPackage
ResolvedExtensionComponent
```

- [ ] Define initial component kinds:

```rust
LegacyProcess
GestaltLifecycle
CommandTool
McpServer
Skill
ClientProduct
```

- [ ] Parse current manifests as `ExtensionManifestV1`.
- [ ] Add `ExtensionManifestV2` with `[package]`, `[compatibility]`, and `[[components]]`.
- [ ] Normalize v1 into one `LegacyProcess` component while preserving its current tools, hooks, context injectors, permissions, and entrypoint.
- [ ] Require all components to share the package version.
- [ ] Add `optional = false` default.
- [ ] Parse client/product components as opaque validated descriptors; do not activate them.
- [ ] Canonicalize IDs:

```text
package:<package-id>
instance:<instance-id>
component:<package-id>:<instance-id>:<component-id>
```

- [ ] Reject duplicate component IDs within a package.
- [ ] Preserve deterministic discovery ordering.
- [ ] Change discovery output from `DiscoveredExtension` to package-oriented inventory while retaining a compatibility view for CLI output.

**Manifest v2 example:**

```toml
manifest_version = 2

[package]
id = "com.example.review"
name = "Review"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"
optional = false

[components.entrypoint]
command = "python"
args = ["-m", "review.lifecycle"]

[[components]]
id = "client-metadata"
kind = "client-product"
optional = true
descriptor = "client/contributions.json"
```

**Test scenarios:**

- Current v1 fixture normalizes to one required legacy component.
- Manifest v2 parses multiple component kinds.
- Duplicate component IDs are rejected.
- Client/product descriptors parse without entering runtime registration.
- Package version applies to every component.
- Reserved package namespaces remain rejected.
- Discovery precedence remains explicit path, workspace, global.

**Verification:**

```bash
cargo test -p gestalt-runtime extension_manifest_v2_tests
cargo test -p gestalt-runtime runtime_extension_tests
cargo test -p gestalt-runtime runtime_process_extension_tests
```

---

## U2. Add configured extension instances to `gestalt.json`

**Goal:** Make `gestalt.json` select and configure extension package instances while manifests continue to declare package facts.

**Requirements:** Multiple instances per package, backward compatibility, schema validation, host-owned grants.

**Dependencies:** U1

**Files:**

- Modify: `crates/gestalt-cli/src/config.rs`
- Modify: `crates/gestalt-cli/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/config.rs`
- Add: `crates/gestalt-runtime/src/extension/config.rs`
- Modify: `docs/feature-spec/config-extension.md`
- Test: `crates/gestalt-cli/tests/config_tests.rs` or nearest existing config test module
- Add: `crates/gestalt-runtime/tests/extension_instance_config_tests.rs`

**Approach:**

- [ ] Extend `ExtensionsConfig` additively; keep `gestalt.json` schema version `1`.
- [ ] Add:

```rust
pub struct ExtensionInstanceConfig {
    pub package: String,
    pub enabled: bool,
    pub components: BTreeMap<String, bool>,
    pub config: serde_json::Value,
    pub grants: ExtensionGrantConfig,
}
```

- [ ] Add:

```rust
pub instances: BTreeMap<String, ExtensionInstanceConfig>
```

- [ ] Continue accepting:
  - `explicit_loads`;
  - `disabled`;
  - `trusted`;
  - `allow_untrusted`;
  - timeout and limit settings.
- [ ] Translate legacy configuration into synthetic instances:
  - instance ID defaults to package ID;
  - `disabled` maps to `enabled = false`;
  - legacy trust maps into host trust metadata;
  - current manifest permissions remain requested permissions.
- [ ] Do not put extension config inside profiles.
- [ ] Validate instance IDs using the same stable identifier rules as package IDs.
- [ ] Resolve each enabled instance to exactly one discovered package.
- [ ] Reject unknown package IDs and unknown component IDs.
- [ ] Load optional package-provided JSON Schema.
- [ ] Validate `instances.<id>.config` before process launch.
- [ ] Keep raw secrets out of instance config; permit credential references only.
- [ ] Compute effective grants as the intersection of:
  - package requested permissions;
  - instance grants;
  - existing host/runtime policy.
- [ ] Record configuration and grant fingerprints in resolved instance descriptors.
- [ ] Add effective-config diagnostics showing source provenance for instance configuration.

**Example:**

```json
{
  "version": 1,
  "extensions": {
    "instances": {
      "review-primary": {
        "package": "com.example.review",
        "enabled": true,
        "components": {
          "lifecycle": true,
          "client-metadata": true
        },
        "config": {
          "policySet": "default"
        },
        "grants": {
          "workspaceRead": true,
          "workspaceWrite": false,
          "network": []
        }
      }
    }
  }
}
```

**Test scenarios:**

- Existing extension configuration still loads.
- Two instances of the same package resolve with different config fingerprints.
- Unknown package or component produces a config error before spawn.
- Package-requested network access is not granted when instance grants are empty.
- Invalid extension config is rejected against package schema.
- A disabled instance is discovered but not activated.

**Verification:**

```bash
cargo test -p gestalt-runtime extension_instance_config_tests
cargo test -p gestalt-cli config
cargo test -p gestalt-cli runtime
```

---

## U3. Split registry construction from immutable runtime snapshots

**Goal:** Replace the startup-only mutable registry as the runtime source of truth with a mutable builder and immutable published snapshots.

**Requirements:** Deterministic snapshot fingerprint, run-boundary pinning, current core loop unchanged.

**Dependencies:** U1, U2

**Files:**

- Replace/modify: `crates/gestalt-runtime/src/registry.rs`
- Add: `crates/gestalt-runtime/src/registry/builder.rs`
- Add: `crates/gestalt-runtime/src/registry/snapshot.rs`
- Add: `crates/gestalt-runtime/src/extension/runtime_snapshot.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/inspect.rs`
- Modify: `crates/gestalt-runtime/src/tool_catalog.rs`
- Modify: `crates/gestalt-runtime/src/lib.rs`
- Test: `crates/gestalt-runtime/tests/runtime_registry_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_builder_tests.rs`
- Add: `crates/gestalt-runtime/tests/runtime_snapshot_tests.rs`

**Approach:**

- [ ] Rename the mutable construction type to `RuntimeRegistryBuilder`.
- [ ] Keep a deprecated `RuntimeRegistry` alias during migration if necessary.
- [ ] Replace string-only hook/verifier entries with typed registrations or descriptors.
- [ ] Add immutable:

```rust
pub struct RuntimeExtensionSnapshot {
    pub generation: RuntimeGeneration,
    pub fingerprint: RuntimeFingerprint,
    pub tool_catalog: Arc<dyn ToolCatalog>,
    pub context_plan: Arc<ContextProviderPlan>,
    pub policy_plan: Arc<PolicyGuardPlan>,
    pub routing_plan: Arc<TurnRoutingPlan>,
    pub verification_plan: Arc<VerificationPlan>,
    pub observer_plan: Arc<EventObserverPlan>,
    pub mcp_registry: Arc<gestalt_mcp::McpRegistry>,
    pub process_instances: Arc<[Arc<ExtensionProcessInstance>]>,
    pub package_health: Arc<[ExtensionInstanceHealth]>,
}
```

This six-plan shape matches the feature spec (`docs/feature-spec/product-neutral-extension-architecture.md` §16.2). Carry one typed plan per capability kind rather than a single `LifecyclePlan`. The snapshot therefore does not need to be restructured when U5 lands: U5 fills `context_plan` and `policy_plan` first, then `routing_plan`, `verification_plan`, and `observer_plan`, all behind the same immutable snapshot contract. The former `registry`/`lifecycle_plan` fields are superseded by these typed plans; `RuntimeRegistrySnapshot` is dropped in favor of plans plus the tool catalog.

- [ ] Keep provider selection outside the snapshot in this plan.
- [ ] Build snapshot fingerprints from:
  - package/version/component identities;
  - manifest hashes;
  - instance config fingerprints;
  - grant fingerprints;
  - tool schema hash;
  - lifecycle plan hash;
  - MCP configuration hash;
  - negotiated protocol versions.
- [ ] Make `AgentRuntime` own:
  - stable provider;
  - base tool catalog;
  - base context assembler;
  - base policy;
  - approval provider;
  - trace sink;
  - runtime config;
  - core hooks;
  - `ExtensionManager`.
- [ ] At the start of `run_session()`, load the current snapshot once.
- [ ] Materialize session-specific wrappers from the pinned snapshot.
- [ ] Construct `AgentLoop` from the pinned components.
- [ ] Keep that snapshot alive through the entire `AgentLoop::run`.
- [ ] Emit `RuntimeGenerationAdopted` with session ID, generation, and fingerprint.
- [ ] Make `inspect()` read the active snapshot rather than stale builder fields.
- [ ] Do not change `gestalt-core::AgentLoop` in this unit.

**Run-boundary invariant:**

```text
active AgentLoop::run
    uses generation N from start to finish

reload publishes generation N+1

next AgentLoop::run
    uses generation N+1
```

**Test scenarios:**

- Snapshot fingerprint is deterministic for equal inputs.
- Registry builder mutation after snapshot construction cannot affect the snapshot.
- A run started on generation N keeps N after N+1 is published.
- A subsequent run adopts N+1.
- Tool schema and execution backend come from the same pinned snapshot.
- Runtime inspection reports active generation and fingerprint.

**Verification:**

```bash
cargo test -p gestalt-runtime runtime_registry_tests
cargo test -p gestalt-runtime runtime_builder_tests
cargo test -p gestalt-runtime runtime_snapshot_tests
cargo test -p gestalt-runtime runtime_run_tests
```

---

## U4. Move discovery, launch, and process ownership into `ExtensionManager`

**Goal:** Remove extension lifecycle ownership from the CLI and create a reusable runtime manager for local, embedded, and headless hosts.

**Requirements:** Launcher abstraction, process state, in-flight tracking, async initialization, no sandbox implementation.

**Dependencies:** U3

**Files:**

- Add: `crates/gestalt-runtime/src/extension/manager.rs`
- Add: `crates/gestalt-runtime/src/extension/launcher.rs`
- Add: `crates/gestalt-runtime/src/extension/process_instance.rs`
- Add: `crates/gestalt-runtime/src/extension/inventory.rs`
- Refactor: `crates/gestalt-runtime/src/process_extension.rs`
- Modify: `crates/gestalt-runtime/src/discovery.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-cli/src/runtime.rs`
- Test: `crates/gestalt-runtime/tests/runtime_process_extension_tests.rs`
- Add: `crates/gestalt-runtime/tests/extension_manager_tests.rs`

**Approach:**

- [ ] Add:

```rust
#[async_trait]
pub trait ExtensionLauncher: Send + Sync {
    async fn launch(
        &self,
        component: &ResolvedRuntimeComponent,
    ) -> Result<Arc<ExtensionProcessInstance>>;
}
```

- [ ] Implement `LocalProcessLauncher` using the current broker spawn logic.
- [ ] Keep environment clearing, protocol size limits, timeout handling, cancellation, and shutdown behavior.
- [ ] Split `ProcessExtensionBroker` into:
  - JSON-RPC transport/client;
  - process lifecycle handle;
  - capability adapters.
- [ ] Add process states:

```rust
Starting
Ready
Draining
Stopping
Stopped
Failed
```

- [ ] Add an in-flight call guard.
- [ ] Reject new calls after the instance enters `Draining`.
- [ ] Add `ExtensionManager` owning:
  - package inventory;
  - resolved instances;
  - process instances;
  - active runtime snapshot;
  - reload mutex;
  - event bus;
  - launcher.
- [ ] Add `AgentRuntimeBuilder::build_async()` for configurations containing external runtime components.
- [ ] Retain synchronous `build()` for native-only runtimes and tests; return a clear error if external process activation is requested.
- [ ] Move discovery, trust resolution, spawn, and registration out of `gestalt-cli/src/runtime.rs`.
- [ ] Make CLI pass resolved paths/configuration into runtime construction rather than process objects.
- [ ] Compute component fingerprints before launch.
- [ ] **Define the canonical reuse key and fingerprint used by both U4 (process reuse) and U6 (MCP client reuse):**

```rust
type ReuseKey = (ComponentInstanceId, ComponentFingerprint);

struct ComponentFingerprint(/* opaque hash */);
```

Fingerprint inputs — all execution-relevant resolved state:

```text
package version
component ID and kind
normalized component declaration
entrypoint / content hash
component-scoped effective instance config
effective grants and trust
protocol / compatibility inputs
normalized backend configuration (for MCP: transport, command/args or URL,
  lifecycle mode, timeouts, annotations, environment, headers, trust)
credential reference identity or non-public credential-generation digest
```

Do **not** expose raw credential values in fingerprints; use credential-reference identity or a non-public digest.

`ExtensionManager` computes the fingerprint before deciding whether to launch a new instance or retain an existing one. This single reuse rule prevents U4 process reuse and U6 MCP reuse from drifting into subtly different lifecycle semantics. U6 must delegate the retain-vs-replace decision to `ExtensionManager` using this key, not maintain its own independent reuse logic.

The current `McpRegistry` (`crates/gestalt-mcp/src/registry.rs`) receives a fixed configuration map at construction and caches clients by server name. After U6, `McpRegistry` construction is governed by the snapshot lifecycle: the `ExtensionManager` decides whether to carry an existing `McpRegistry` (and its clients) forward into the new snapshot based on this reuse key, rather than creating a fresh registry per generation.
- [ ] Apply effective host grants rather than treating requested manifest permissions as authority.
- [ ] Preserve the current unsandboxed local-process behavior and report it accurately in inspection.

**Test scenarios:**

- Non-CLI code can build an async runtime with a process extension.
- CLI no longer directly calls `ProcessExtensionBroker::spawn`.
- Instance state progresses `Starting → Ready`.
- A failed required component rejects initial runtime construction.
- A failed optional component produces degraded package health.
- Draining instances refuse new calls but allow existing calls to finish.
- Native-only `build()` remains synchronous.
- Component fingerprint is deterministic for equal resolved inputs and changes when any fingerprinted input changes.
- Reuse key `(ComponentInstanceId, ComponentFingerprint)` matches across snapshot generations when the component is unchanged.

**Verification:**

```bash
cargo test -p gestalt-runtime extension_manager_tests
cargo test -p gestalt-runtime runtime_process_extension_tests
cargo test -p gestalt-runtime runtime_builder_tests
cargo test -p gestalt-cli runtime
```

---

## U5. Introduce lifecycle protocol v2 and typed capability plans

**Goal:** Replace the external generic hook model with explicit context, policy, routing, verification, and observation capabilities.

**Requirements:** Minimal protocol methods, stable DTOs, host-assigned trust, deterministic reducers, v1 compatibility.

**Dependencies:** U4

**Files:**

- Add: `crates/gestalt-runtime/src/lifecycle/mod.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/protocol.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/client.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/context_provider.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/policy_guard.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/turn_router.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/verifier.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/event_observer.rs`
- Add: `crates/gestalt-runtime/src/lifecycle/plan.rs`
- Modify: `crates/gestalt-runtime/src/composition_hooks.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Modify: `crates/gestalt-runtime/src/policy.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/manifest.rs`
- Test: `crates/gestalt-runtime/tests/runtime_hooks_tests.rs`
- Add: `crates/gestalt-runtime/tests/lifecycle_protocol_v2_tests.rs`
- Add: `crates/gestalt-runtime/tests/lifecycle_composition_tests.rs`

**Approach:**

- [ ] Define protocol v2 method set:

```text
initialize
capabilities/describe
lifecycle/invoke
shutdown
$/cancelRequest
```

- [ ] Add explicit version negotiation using supported-version arrays.
- [ ] Introduce versioned DTOs independent from internal Rust domain types.
- [ ] Do not serialize `Session`, `ContextPacket`, or raw `AgentEvent` as the stable protocol contract.
- [ ] Add typed internal interfaces:

```rust
ContextProvider
PolicyGuard
TurnRouter
ExternalVerifier
EventObserver
```

- [ ] Add typed registration descriptors including:
  - priority;
  - timeout;
  - failure mode;
  - required data scope;
  - component identity.
- [ ] Build one immutable typed plan per capability kind, stored in the snapshot fields added in U3 (`context_plan`, `policy_plan`, `routing_plan`, `verification_plan`, `observer_plan`). Do not introduce a single `LifecyclePlan` aggregate.
- [ ] Land this unit in two stages to keep tool/catalog/policy wiring stable for U6:
  - **U5a — internal typed interfaces:** introduce `ContextProvider`, `PolicyGuard`, `TurnRouter`, `ExternalVerifier`, `EventObserver` as internal Rust traits; adapt trusted native `CompositionHooks` and the v1 mappings (`before_tool_policy` → policy guard, `prepare_next_turn` → turn router, `on_event` → observer, context injectors → context provider) into typed registrations; fill `context_plan` and `policy_plan` first. No external protocol change yet. This stage owns the `policy.rs` / `composition_hooks.rs` / `runtime.rs` wiring that U6 also touches, so it must land before U6.
  - **U5b — external protocol v2:** add `initialize`, `capabilities/describe`, `lifecycle/invoke`, versioned DTOs, data-scope projection, and deterministic reducers on top of the now-stable internal interfaces from U5a. Fill `routing_plan`, `verification_plan`, `observer_plan` for external components.
- [ ] Map protocol v1 context injectors into context providers.
- [ ] Map v1:
  - `before_tool_policy` into policy guards;
  - `prepare_next_turn` into turn routers;
  - `on_event` into observers;
  - remaining generic hooks through a temporary legacy adapter.
- [ ] Keep trusted native `CompositionHooks` as a compatibility surface, but adapt them into typed internal registrations.
- [ ] Replace external `HookOutcome` with capability-specific results.
- [ ] Context contributions must preserve:
  - contribution ID;
  - content;
  - requested priority;
  - stability;
  - provenance;
  - lifetime.
- [ ] Host assigns final trust and placement.
- [ ] Policy reduction:

```text
Deny > RequireApproval > Annotate > Abstain
```

- [ ] Turn routing:
  - highest-priority stop wins;
  - otherwise highest-priority route wins;
  - equal-priority conflicts are traced and ignored.
- [ ] Verifier reports are collected, not overwritten.
- [ ] Event observers are non-authoritative and use bounded delivery.
- [ ] Remove full-history transfer unless a handler explicitly declares and the host permits that data scope.
- [ ] Remove `HookOutcome::Aggregated` from the external protocol.
- [ ] Preserve v1 behavior behind compatibility tests.

**Test scenarios:**

- V2 initialization rejects incompatible protocol versions.
- Capability descriptors must match manifest declarations.
- Untrusted context cannot self-promote to trusted or critical.
- Policy guards run once in deterministic order.
- Deny dominates approval and annotation.
- Equal-priority conflicting routes produce no route.
- Verifier reports from multiple components are all retained.
- Observer failure never blocks the run.
- V1 fixture behavior remains compatible.

**Verification:**

```bash
cargo test -p gestalt-runtime lifecycle_protocol_v2_tests
cargo test -p gestalt-runtime lifecycle_composition_tests
cargo test -p gestalt-runtime runtime_hooks_tests
cargo test -p gestalt-runtime runtime_process_extension_tests
```

---

## U6. Add command-tool components and normalize package MCP components

**Goal:** Make simple custom tools easy to author while using MCP as the preferred general external tool protocol.

**Requirements:** JSON stdin/stdout command tools, package MCP normalization, canonical tool IDs, legacy process compatibility.

**Dependencies:** U3, U4, U5a

**Files:**

- Add: `crates/gestalt-runtime/src/extension/command_tool.rs`
- Add: `crates/gestalt-runtime/src/extension/mcp_component.rs`
- Modify: `crates/gestalt-runtime/src/tool_catalog.rs`
- Modify: `crates/gestalt-runtime/src/tool_catalog_planner.rs`
- Modify: `crates/gestalt-runtime/src/mcp.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Modify: `crates/gestalt-mcp/src/registry.rs`
- Modify: `crates/gestalt-runtime/src/manifest.rs`
- Test: `crates/gestalt-runtime/tests/runtime_registry_tests.rs`
- Add: `crates/gestalt-runtime/tests/command_tool_tests.rs`
- Add: `crates/gestalt-runtime/tests/package_mcp_component_tests.rs`

**Approach:**

- [ ] Add `command-tool` manifest component.
- [ ] Implement per-call process execution:
  - request JSON on stdin;
  - result JSON on stdout;
  - stderr captured as diagnostics;
  - timeout;
  - output-size limit;
  - cancellation by process termination.
- [ ] Reuse the canonical `Tool` trait and existing policy/approval/execution path.
- [ ] Require explicit tool schema, risk, read-only, and idempotency declarations.
- [ ] Preserve annotation source as extension-declared unless host trust promotes it.
- [ ] Add package `mcp-server` components.
- [ ] Merge direct `mcp.servers` and package components into one candidate MCP configuration.
- [ ] Canonicalize package MCP server IDs.
- [ ] Create or reuse an `McpRegistry` per runtime snapshot.
- [ ] Reuse unchanged MCP registries/clients by delegating the retain-vs-replace decision to `ExtensionManager` using the canonical `ReuseKey` defined in U4 (`(ComponentInstanceId, ComponentFingerprint)`). Do not introduce an MCP-specific reuse key. The `ExtensionManager` carries forward the existing `McpRegistry` and its clients when the reuse key matches; otherwise a fresh registry is constructed.
- [ ] Ensure MCP tools remain policy-gated and traced through the existing composed catalog.
- [ ] Keep protocol v1 `tools/call` operational.
- [ ] Mark v1 process tools as legacy in documentation only after this unit lands.

**Test scenarios:**

- A Python or shell-independent command tool returns structured text.
- Invalid JSON output becomes a tool execution error.
- Timeout kills the command process.
- Direct MCP config and package MCP config coexist.
- MCP server-name collisions fail candidate construction.
- Old runtime snapshot retains old MCP clients while a replacement snapshot is active.
- Legacy process tool fixtures remain green.

**Verification:**

```bash
cargo test -p gestalt-runtime command_tool_tests
cargo test -p gestalt-runtime package_mcp_component_tests
cargo test -p gestalt-runtime runtime_registry_tests
cargo test -p gestalt-mcp
```

---

## U7. Implement transactional reload and evolve `AgentRuntimeHandle` into `RuntimeControl`

**Goal:** Deliver real manual hot reload and a stable application-neutral control surface without implementing a client/product code host.

**Requirements:** Candidate validation, atomic publication, draining, reload reports, dry-run, stable event projection.

**Dependencies:** U3-U6

**Files:**

- Modify: `crates/gestalt-runtime/src/extension/manager.rs`
- Modify: `crates/gestalt-runtime/src/extension/process_instance.rs`
- Modify: `crates/gestalt-runtime/src/event_bus.rs`
- Modify: `crates/gestalt-runtime/src/orchestration.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/inspect.rs`
- Modify: `crates/gestalt-cli/src/main.rs`
- Modify: `crates/gestalt-cli/src/runtime.rs`
- Modify: `crates/gestalt-cli/src/output.rs`
- Add: `crates/gestalt-runtime/src/control.rs`
- Add: `crates/gestalt-runtime/tests/extension_reload_tests.rs`
- Add: `crates/gestalt-runtime/tests/runtime_control_tests.rs`
- Add: `crates/gestalt-cli/tests/extension_cli_tests.rs`

**Approach:**

- [ ] Add candidate reload flow:

```text
discover
→ resolve instances
→ validate manifests/config
→ diff components
→ reuse unchanged instances
→ launch changed instances
→ negotiate capabilities
→ build registry candidate
→ validate conflicts
→ build snapshot candidate
→ publish atomically
→ mark old instances draining
→ stop drained instances
```

- [ ] Use a runtime-owned reload mutex.
- [ ] Use `RwLock<Arc<RuntimeExtensionSnapshot>>` initially for all-or-nothing publication; do not add `arc-swap` unless profiling justifies it.
  - **Invariant this choice relies on:** the run-boundary pin (U3) means a reader holds a snapshot `Arc` for at most one `AgentLoop::run` invocation. Writers only contend for the write lock at `run_session()` entry — the brief window where the next snapshot is loaded — and during reload publication. Neither path is a hot loop, so RwLock contention is acceptable. Do not "optimize" the RwLock out (e.g. by cloning the snapshot into the run path and dropping the lock) without first re-deriving that readers still finish within bounded time, or the atomic-publication guarantee is silently lost.
- [ ] Add reload dry-run returning:
  - package diff;
  - component diff;
  - planned restarts;
  - candidate fingerprint;
  - validation errors.
- [ ] Reuse unchanged component instances by fingerprint.
- [ ] Required candidate failure leaves the active generation untouched.
- [ ] Optional component failure produces degraded candidate health.
- [ ] Add configurable drain grace period.
- [ ] Rename or supersede `AgentRuntimeHandle` with:

```rust
pub trait RuntimeControl
```

- [ ] Preserve a deprecated `AgentRuntimeHandle` alias/adapter.
- [ ] Move current session, steering, artifact, and subscription operations into `RuntimeControl`.
- [ ] Add:
  - `inspect_runtime`;
  - `reload_extensions`;
  - `current_generation`;
  - `extension_health`;
  - `respond_to_approval`.
- [ ] **Approval control surface (replaces prior "do not add a broker" stance):** add a small runtime-owned `ApprovalBroker` and expose approval response through `RuntimeControl::respond_to_approval`. Rationale and constraints:
  - `ApprovalProvider` (`crates/gestalt-core/src/approval.rs`) only exposes `approve(request) -> decision` and `approve_cancellable`; it has no pending-request lookup or response method. A thin pass-through is therefore impossible.
  - The executor already emits `AgentEvent::ApprovalRequested` (`crates/gestalt-core/src/agent/executor.rs`) before awaiting the provider, and the TUI already implements the required broker pattern using a `oneshot` response channel created at `approve()` time (`crates/gestalt-cli/src/tui/approval.rs`).
  - The new `ApprovalBroker` therefore:
    - implements `ApprovalProvider`;
    - stores pending approvals under a **stable approval ID** with a per-approval `oneshot::Sender<ApprovalDecision>`;
    - emits approval events on the runtime event bus;
    - resolves them through `RuntimeControl::respond_to_approval(approval_id, decision)`;
    - cleans up pending entries on cancellation, timeout, or session termination.
  - The CLI may retain its existing direct stdin approval provider unchanged; the broker is the path used by `RuntimeControl` consumers (and the TUI may later route through it).
  - `RuntimeControl::respond_to_approval` must report approval response as **unsupported** when the runtime was built with a non-broker `ApprovalProvider` (e.g. the stdin provider), rather than silently dropping the response.
  - **Out of scope:** multi-voter/parallel approval, approval policy, persistence of pending approvals across reload. One request, one response, one stable ID.
- [ ] Add stable `RuntimeEventEnvelopeV1` projection.
- [ ] Sequence all stable projected events.
- [ ] Keep raw `RuntimeEvent` for internal diagnostics.
- [ ] Replace coarse reload events with:
  - reload started;
  - candidate built;
  - reload failed;
  - runtime published;
  - component draining;
  - component stopped.
- [ ] Make `gestalt extension reload` call the live manager/control path.
- [ ] Add:
  - `gestalt extension reload <instance-id>`;
  - `--dry-run`;
  - `--force`.
- [ ] Do not add automatic file watching.
- [ ] Parse and validate client/product descriptors during reload, but return them as inventory for an embedding host; do not execute them.

**Test scenarios:**

- Successful reload increments generation and changes fingerprint.
- Failed candidate leaves generation and fingerprint unchanged.
- Active run keeps generation N while reload publishes N+1.
- Next run adopts N+1.
- Unchanged extension process PID is reused.
- Changed process enters draining and stops after in-flight calls finish.
- Dry-run launches no process and publishes no generation.
- RuntimeControl works without CLI.
- CLI reload report matches runtime reload report.
- Client/product descriptors are exposed as inactive inventory.
- `ApprovalBroker` stores a pending approval under a stable ID and resolves it via `RuntimeControl::respond_to_approval`.
- `respond_to_approval` returns "unsupported" when the runtime was built with a non-broker provider.
- Pending approvals are cleaned up on session termination and cancellation.

**Verification:**

```bash
cargo test -p gestalt-runtime extension_reload_tests
cargo test -p gestalt-runtime runtime_control_tests
cargo test -p gestalt-runtime runtime_run_tests
cargo test -p gestalt-cli extension_cli_tests
```

---

## U8. Add conformance fixtures, migration documentation, and architectural records

**Goal:** Make the new boundaries implementable by third parties and keep source, docs, and compatibility behavior aligned.

**Requirements:** ADRs, generated examples, compatibility matrix, protocol fixtures, no undocumented behavior.

**Dependencies:** U1-U7

**Files:**

- Add: `docs/adrs/ADR-0XX-extension-package-components.md`
- Add: `docs/adrs/ADR-0XX-runtime-snapshot-reload.md`
- Add: `docs/adrs/ADR-0XX-lifecycle-protocol-v2.md`
- Update: `docs/gestalt-harness-architecture.md`
- Update: `docs/extension-development-guide.md`
- Update: `docs/jsonrpc-extension-protocol.md`
- Update: `docs/extension-manifest-schema.md`
- Update: `docs/feature-spec/config-extension.md`
- Update: `docs/runtime-event-bus.md`
- Add: `docs/migrations/extension-manifest-v1-to-v2.md`
- Add fixtures: `crates/gestalt-runtime/tests/fixtures/protocol-v2/`
- Add: `crates/gestalt-runtime/tests/extension_conformance_tests.rs`

**Approach:**

- [ ] Document the distinction between:
  - runtime module;
  - package;
  - component;
  - configured instance;
  - process instance;
  - runtime generation;
  - client/product descriptor.
- [ ] Document v1 compatibility and exact deprecation status.
- [ ] Add protocol fixtures for:
  - valid initialization;
  - unsupported protocol;
  - capability mismatch;
  - malformed response;
  - timeout;
  - cancellation;
  - oversized message;
  - context contribution;
  - policy denial;
  - route conflict;
  - verifier report;
  - observer lag.
- [ ] Add manifest v2 examples for:
  - lifecycle-only;
  - command-tool-only;
  - MCP-only;
  - combined runtime/client descriptor package;
  - optional component.
- [ ] Add migration examples from current manifest format.
- [ ] Generate or validate documentation examples in tests where practical.
- [ ] Update architecture diagrams to show runtime and client/product extension hosts as separate boundaries.
- [ ] Mark sandboxing, remote transport, package registry, lockfile, dependencies, client code loading, and turn-level adoption as explicitly deferred.

**Verification:**

```bash
cargo test -p gestalt-runtime extension_conformance_tests
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

---

# 5. Dependency Graph

```text
U0  Current behavior and lifecycle cleanup
 │
 ▼
U1  Package/component/instance domain
 │
 ▼
U2  gestalt.json instance configuration
 │
 ▼
U3  Registry builder and immutable snapshot
 │
 ▼
U4  ExtensionManager and process ownership
 │
 ▼
U5a Internal typed lifecycle interfaces + v1 mapping (context_plan, policy_plan)
 │
 ▼
U6  Command tools + package MCP
 │
 ▼
U5b External lifecycle protocol v2 (routing_plan, verification_plan, observer_plan)
 │
 ▼
U7  Transactional reload + RuntimeControl
 │
 ▼
U8  Conformance, ADRs, and migration docs
```

**Why U5 is split and sequenced around U6:** both U5a and U6 modify `policy.rs`, `composition_hooks.rs`/`tool_catalog*.rs`, `builder.rs`, and `runtime.rs`. If landed in parallel as originally drawn, they would conflict over the same catalog/policy wiring. U5a stabilizes the internal typed interfaces and the `context_plan`/`policy_plan` snapshot fields first; U6 then adds command tools and package-MCP against that stable surface; U5b layers the external protocol on top. U5b (external protocol, DTOs, reducers) touches none of the catalog/policy wiring U6 needs, so it is safe after U6.

---

# 6. Recommended Delivery Slices

The units should not be merged as one oversized pull request.

## Slice A — Semantic cleanup

```text
U0
```

Merge when current behavior is characterized and pre-policy invocation is exactly once.

## Slice B — Domain and configuration

```text
U1 + U2
```

Merge when manifests normalize into package/component/instance models while current extensions still run unchanged.

## Slice C — Runtime ownership

```text
U3 + U4
```

Merge when the runtime owns snapshots and process lifecycle, even before protocol v2 and reload are exposed.

This is the most important architectural milestone.

## Slice D — Capability simplification

```text
U5a + U6 + U5b
```

U5a lands first (internal typed lifecycle interfaces + v1 mapping), then U6 (command tools + package MCP) against the stable internal surface, then U5b (external protocol v2). They may be three separate PRs in that order; do not merge U5a and U6 in parallel because both rewrite the same catalog/policy wiring.

## Slice E — Reload and control surface

```text
U7
```

Merge only after generation-pinning and draining tests are stable.

## Slice F — Contract stabilization

```text
U8
```

Documentation and conformance become release gates for declaring protocol v2 stable.

---

# 7. System-Wide Impact

## Core impact

`gestalt-core` remains mostly unchanged.

The only acceptable core changes are generic improvements needed by existing runtime contracts, not extension-package concepts.

Per-turn component resolution is deferred.

## Runtime impact

`gestalt-runtime` becomes the clear owner of:

- extension package normalization;
- configured instances;
- process lifecycle;
- runtime snapshot generations;
- lifecycle composition;
- reload;
- application-neutral control.

This is the primary refactor surface.

## CLI impact

The CLI stops spawning extension processes directly.

It becomes a consumer of:

- effective configuration;
- package discovery diagnostics;
- `RuntimeControl`;
- reload reports.

## MCP impact

MCP remains a first-class tool backend.

Package MCP components feed the same registry rather than creating a second MCP implementation.

## Verification impact

Verifier names in `RuntimeRegistry` must become executable typed registrations or typed reports.

This may require adapters around the existing `gestalt-verify` registry.

## Trace impact

Traces and runtime events gain:

- generation;
- fingerprint;
- package/instance/component identity;
- reload lifecycle;
- degraded health.

## Client/product impact

No client code host is implemented.

Embedding hosts receive:

- `RuntimeControl`;
- stable client event projection;
- parsed client/product component descriptors.

This creates room for later customization without coupling the harness to one product.

---

# 8. Risks and Mitigations

| Risk                                                         | Mitigation                                                   |
| ------------------------------------------------------------ | ------------------------------------------------------------ |
| The refactor becomes a rewrite of every runtime subsystem.   | Preserve current provider, approval, core loop, context assembler, and policy implementations; change ownership and composition first. |
| Manifest v2 breaks existing extensions.                      | Normalize v1 through a dedicated compatibility adapter and keep golden fixtures. |
| Snapshot publication is called “atomic” but components are still mutated underneath. | Snapshots contain immutable registrations and `Arc` process handles; process state changes only through explicit lifecycle transitions. |
| Reload swaps tools while a model turn is using old schemas.  | Pin one snapshot for the complete `AgentLoop::run` invocation. |
| Run-boundary adoption is insufficient for very long runs.    | Measure real need before adding a core-level per-turn resolver; keep it as a separate future ADR. |
| CLI and runtime keep duplicate discovery or trust logic.     | Move resolution into `ExtensionManager`; CLI only supplies effective config and renders diagnostics. |
| Lifecycle v2 duplicates core types.                          | Treat DTO duplication as intentional protocol isolation and add explicit conversion adapters. |
| Typed lifecycle work becomes too broad.                      | Split U5 into U5a (internal typed interfaces + v1 mapping, owning the shared catalog/policy wiring) landing before U6, and U5b (external protocol v2) landing after U6. U6 builds command tools and package MCP against the stable U5a surface instead of racing it for the same files. |
| Package-declared MCP requires live registry mutation.        | Put `McpRegistry` inside immutable snapshots; old snapshots retain old clients until drained. |
| Command tools become a hidden shell escape.                  | Spawn exact commands without shell interpretation by default; preserve existing shell-entrypoint checks and policy gates. |
| Client extension concerns leak into runtime.                 | Parse descriptors only; no client code execution or UI APIs in this plan. |
| Adding JSON Schema validation increases dependency weight.   | Keep validator in `gestalt-runtime`/CLI, not core; measure binary impact and feature-gate if necessary. |
| Existing orchestration handle and new RuntimeControl diverge. | Evolve/rename the existing trait rather than adding a parallel API. |
| Event compatibility becomes unmanageable.                    | Keep raw diagnostic events internal and define a small versioned client projection. |
| Approval response requires a new broker, expanding U7 scope. | `ApprovalProvider` has no response method, so add a minimal `ApprovalBroker` (stores pending by stable ID, resolves via `RuntimeControl::respond_to_approval`, cleans up on cancel/timeout/session-end). Report "unsupported" for non-broker providers. Keep CLI stdin provider unchanged. Single-voter only; no persistence across reload. |

---

# 9. Deferred Follow-Up Features

These require separate specifications and plans:

1. **Client/product extension code host**
   - code loading;
   - contribution registration;
   - disposal;
   - host API;
   - client-side hot reload.

2. **Package distribution**
   - install/update/remove;
   - package sources;
   - `gestalt.lock`;
   - signatures;
   - publisher identity;
   - package dependencies.

3. **Automatic watch mode**
   - filesystem watcher;
   - debounce;
   - build commands;
   - source-map diagnostics.

4. **Remote extension transport**
   - authenticated transport;
   - latency and retry semantics;
   - remote cancellation;
   - worker identity.

5. **Reload state transfer**
   - export/import;
   - schema compatibility;
   - encrypted state;
   - rollback.

6. **Turn-level generation adoption**
   - generic core resolver;
   - safe point contract;
   - per-turn trace identity;
   - provider/tool/context consistency.

7. **Sandboxing**
   - launcher implementations;
   - platform isolation;
   - network enforcement;
   - resource limits.

8. **Legacy protocol removal**
   - telemetry or ecosystem evidence;
   - deprecation window;
   - migration tooling.

9. **Advanced approval semantics**
   - multi-voter / parallel approval workflows;
   - approval policy rules (auto-approve/deny by scope);
   - pending approval persistence across reload;
   - approval audit trail.

---

# 10. Acceptance Check

The foundation is ready to merge when:

- `GestaltExtension` is no longer the primary universal abstraction;
- trusted native registration uses `RuntimeModule`;
- current manifests run through a v1 compatibility adapter;
- manifest v2 supports multiple component descriptors;
- `gestalt.json` can configure multiple instances of one package;
- requested permissions and host grants are distinct;
- the mutable registry is only a build-time object;
- active runtime composition is represented by an immutable generation;
- one complete `AgentLoop::run` invocation uses one generation;
- extension discovery and process spawning are owned by `gestalt-runtime`;
- CLI no longer launches extension processes directly;
- typed lifecycle capabilities replace generic external hook outcomes;
- context trust/provenance survives extension boundaries;
- policy guards execute exactly once;
- command-tool components work through the canonical tool path;
- direct and package-declared MCP servers share the existing MCP path;
- manual reload validates a candidate before publication;
- failed reload leaves the active generation untouched;
- replaced process instances drain before shutdown;
- unchanged instances are reused by canonical `(ComponentInstanceId, ComponentFingerprint)` key;
- `RuntimeControl` supersedes the narrower orchestration handle without creating a second API;
- `RuntimeControl::respond_to_approval` resolves pending approvals through a runtime-owned `ApprovalBroker`, and reports "unsupported" for non-broker providers;
- stable client event projection is versioned;
- client/product component descriptors are parseable but not executed;
- all v1 compatibility, reload, generation, and lifecycle fixtures are green;
- architecture, protocol, manifest, and migration documents match implementation.