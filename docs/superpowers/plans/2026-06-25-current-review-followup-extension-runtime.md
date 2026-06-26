# Current Review Follow-Up Extension Runtime Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the blockers called out in the current review of commit `8e7629a41544481096b7a1c89a8151bcba431766` so extension composition, execution, and reload are operationally safe rather than only structurally present.

**Architecture:** Introduce a concrete `RuntimeHost` that owns one workspace, one extension composition, one `ExtensionManager`, one session registry, one event bus, one artifact store, and one approval broker. Extension startup and reload both flow through one `ExtensionActivationPipeline` that computes effective permissions, resolves package-relative entrypoints, launches and negotiates long-lived resources, builds immutable `RuntimeExtensionSnapshot` generations, and publishes them using generation leases and deferred retirement. `AgentRuntime::run_session()` acquires a `RuntimeSnapshotLease`, executes only against that pinned snapshot, and remains isolated from later reloads.

**Tech Stack:** Rust, Tokio, JSON-RPC over stdio, `gestalt-runtime`, `gestalt-cli`, cargo tests, `cargo fmt`, `cargo clippy`.

---

## Contract Decisions

The implementation must lock these decisions before feature work:

- `RuntimeHost` owns one workspace and one extension composition generation lineage.
- Per-session overrides must not change workspace root, extension discovery roots, extension instances, grants, direct MCP configuration, or package trust decisions.
- Startup and reload must both call the same activation service:

```rust
pub struct ExtensionActivationPipeline {
    discovery: Arc<dyn ExtensionSource>,
    launcher: Arc<dyn ExtensionLauncher>,
    base_composition: Arc<BaseRuntimeComposition>,
    host_context: HostLaunchContext,
}

pub struct ActivationRequest {
    pub current: Option<Arc<RuntimeExtensionSnapshot>>,
    pub target_instance: Option<String>,
    pub force: bool,
    pub mode: ActivationMode,
}

pub struct ActivationCandidate {
    pub snapshot: Arc<RuntimeExtensionSnapshot>,
    pub diff: ExtensionGenerationDiff,
    pub newly_started: Vec<ManagedExtensionResource>,
    pub reused: Vec<ManagedExtensionResource>,
    pub diagnostics: Vec<ActivationDiagnostic>,
}
```

- `ActivationCandidate` owns rollback cleanup until explicitly committed.
- Reload and retirement use leases, not immediate draining:

```rust
pub struct RuntimeSnapshotLease {
    snapshot: Arc<RuntimeExtensionSnapshot>,
    retirement: Arc<GenerationRetirement>,
}
```

- A retired generation may still serve calls while leases exist. Only after the last lease is released may non-reused resources enter draining.
- Long-lived generation resources are managed uniformly:

```rust
enum ManagedExtensionResource {
    Process(Arc<ExtensionProcessInstance>),
    Mcp(Arc<ManagedMcpServer>),
    Observer(Arc<ObserverWorker>),
}
```

- Mutable live health must not live inside immutable snapshots. Snapshots may carry immutable activation diagnostics only.
- Protocol-v2 cancellation must be defined explicitly. This plan chooses: cancellation support is declared in `InitializeResponseV2`, and when declared it is mandatory best-effort.
- Lifecycle execution must use typed DTOs rather than unstructured `serde_json::Value`:

```rust
ContextProviderRequestV1
PolicyGuardRequestV1
TurnRouterRequestV1
VerifierRequestV1
EventObserverRequestV1
```

## File Structure

- Modify [crates/gestalt-runtime/src/control.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/control.rs): move host-wide control APIs onto `RuntimeHost` and add approval response.
- Modify [crates/gestalt-runtime/src/orchestration.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/orchestration.rs): deprecate `AgentRuntimeHandle` into a compatibility adapter over `RuntimeHost`.
- Add [crates/gestalt-runtime/src/activation.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/activation.rs): define `ExtensionActivationPipeline`, `ActivationRequest`, `ActivationCandidate`, `ExtensionGenerationDiff`, rollback ownership, and collision validation.
- Modify [crates/gestalt-runtime/src/extension/manager.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/manager.rs): add generation leases, retirement coordination, resource ownership, single-flight launch, stable reuse rules, and live health.
- Modify [crates/gestalt-runtime/src/extension/runtime_snapshot.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/runtime_snapshot.rs): carry immutable activation diagnostics, executable lifecycle plans, negotiated protocol data, and retained managed resources for the generation.
- Modify [crates/gestalt-runtime/src/permissions.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/permissions.rs): compute effective permissions and define path/network intersection rules.
- Modify [crates/gestalt-runtime/src/config.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/config.rs): extend `ExtensionGrantConfig` with shell and allowed path grants.
- Modify [crates/gestalt-runtime/src/extension/package.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/package.rs): add typed trust, requested permission declarations, package-relative entrypoint resolution, collision inputs, and fingerprint material.
- Modify [crates/gestalt-runtime/src/extension/launcher.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/launcher.rs): require `HostLaunchContext` and launch managed resources with host-scoped enforcement.
- Modify [crates/gestalt-runtime/src/extension/command_tool.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/command_tool.rs): enforce effective command permissions and package-relative working directories.
- Modify [crates/gestalt-runtime/src/extension/mcp_component.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/mcp_component.rs): enforce permissions for stdio and HTTP MCP backends and define managed MCP reuse/retirement.
- Modify [crates/gestalt-runtime/src/process_extension.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/process_extension.rs): separate common broker mechanics, legacy v1 compatibility, and protocol-v2 cancellation.
- Modify [crates/gestalt-runtime/src/lifecycle/process_client.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/process_client.rs): negotiate protocol version, cancellation support, descriptors, and typed capability requests.
- Modify [crates/gestalt-runtime/src/lifecycle/plan.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/plan.rs): make executable plan registrations with ordering, reducer, failure-mode, timeout, and data-scope semantics.
- Modify [crates/gestalt-runtime/src/runtime.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/runtime.rs): acquire `RuntimeSnapshotLease` and execute pinned lifecycle plans during runs.
- Modify [crates/gestalt-runtime/src/discovery.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/discovery.rs): surface discovery failures as explicit diagnostics and differentiate required from optional packages.
- Modify [crates/gestalt-runtime/REFERENCE.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/REFERENCE.md): reflect every public API, trait contract, lifecycle rule, and ownership invariant changed by this work.
- Modify docs under [docs/adrs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/adrs), [docs/feature-spec](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/feature-spec), and [docs/extension-development-guide.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/extension-development-guide.md): align docs with implemented semantics only.

## Task 1: Define Core Contracts and Host Ownership

**Files:**
- Add: [crates/gestalt-runtime/src/activation.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/activation.rs)
- Modify: [crates/gestalt-runtime/src/control.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/control.rs)
- Modify: [crates/gestalt-runtime/src/orchestration.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/orchestration.rs)
- Modify: [crates/gestalt-runtime/src/extension/runtime_snapshot.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/runtime_snapshot.rs)
- Modify: [crates/gestalt-runtime/src/lib.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lib.rs)
- Test: [crates/gestalt-runtime/tests/runtime_control_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_control_tests.rs)
- Test: [crates/gestalt-runtime/tests/runtime_snapshot_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_snapshot_tests.rs)

- [ ] Add failing tests that prove one host owns one workspace and one extension generation lineage, and that per-session overrides cannot mutate extension composition inputs.
- [ ] Define `RuntimeHost`, `ExtensionActivationPipeline`, `ActivationRequest`, `ActivationCandidate`, `ExtensionGenerationDiff`, `ActivationDiagnostic`, and `RuntimeSnapshotLease`.
- [ ] Add `respond_to_approval(approval_id, decision)` to `RuntimeControl` and make `RuntimeHost` the primary implementer.
- [ ] Change `AgentRuntimeHandle` to a compatibility adapter over `Arc<RuntimeHost>`; do not require `AgentRuntime` itself to implement the full host control surface.
- [ ] Record contract decisions in `REFERENCE.md` as soon as the types stabilize.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test runtime_control_tests \
  --test runtime_snapshot_tests
```

## Task 2: Define Permission, Trust, and Package Resolution Contracts

**Files:**
- Modify: [crates/gestalt-runtime/src/config.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/config.rs)
- Modify: [crates/gestalt-runtime/src/permissions.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/permissions.rs)
- Modify: [crates/gestalt-runtime/src/extension/package.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/package.rs)
- Modify: [crates/gestalt-runtime/src/manifest.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/manifest.rs)
- Modify: [crates/gestalt-runtime/src/extension_trust.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension_trust.rs)
- Test: [crates/gestalt-runtime/tests/runtime_permissions_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_permissions_tests.rs)
- Test: [crates/gestalt-runtime/tests/extension_manifest_v2_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/extension_manifest_v2_tests.rs)

- [ ] Add failing tests for shell grants, allowed external paths, wildcard host intersection, read/write independence, package-relative entrypoint resolution, and required-package discovery failure behavior.
- [ ] Extend grant configuration to:

```rust
pub struct ExtensionGrantConfig {
    pub workspace_read: bool,
    pub workspace_write: bool,
    pub shell: bool,
    pub network: Vec<String>,
    pub allowed_paths: Vec<PathBuf>,
}
```

- [ ] Add explicit requested permission declarations for v2 packages or executable components so the runtime can compute `manifest request ∩ instance grant ∩ host policy`.
- [ ] Replace string trust fingerprints with:

```rust
enum ExtensionTrust {
    BuiltIn,
    IntegrityTrusted { manifest_hash: String },
    Untrusted,
}
```

- [ ] Define and implement path/network intersection rules:
  - wildcard never expands beyond the next restricting layer;
  - `allow_all_paths` survives only if every layer permits it;
  - external paths must be present in every layer;
  - write is never implied by read.
- [ ] Preserve both declared and resolved entrypoints in diagnostics and fingerprints.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test runtime_permissions_tests \
  --test extension_manifest_v2_tests
```

## Task 3: Build Resource Manager Foundations Before Reload

**Files:**
- Modify: [crates/gestalt-runtime/src/extension/manager.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/manager.rs)
- Modify: [crates/gestalt-runtime/src/extension/process_instance.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/process_instance.rs)
- Modify: [crates/gestalt-runtime/src/extension/runtime_snapshot.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/runtime_snapshot.rs)
- Modify: [crates/gestalt-runtime/src/extension/mcp_component.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/mcp_component.rs)
- Test: [crates/gestalt-runtime/tests/extension_manager_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/extension_manager_tests.rs)
- Test: [crates/gestalt-runtime/tests/runtime_snapshot_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_snapshot_tests.rs)

- [ ] Add failing tests for per-reuse-key single-flight launch, generation lease retention, deferred retirement, in-flight drain timeout, and managed MCP reuse/retirement.
- [ ] Implement `ManagedExtensionResource` ownership in the manager for processes, MCP resources, and observer workers.
- [ ] Implement stable reuse keys and created-versus-reused tracking that the activation pipeline can consume.
- [ ] Add generation retirement coordination so `Retired` generations remain callable while leases exist and only later enter draining/shutdown for non-reused resources.
- [ ] Separate immutable activation diagnostics in snapshots from mutable operational health in manager state.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test extension_manager_tests \
  --test runtime_snapshot_tests
```

## Task 4: Enforce Permissions Across Every Backend

**Files:**
- Modify: [crates/gestalt-runtime/src/extension/launcher.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/launcher.rs)
- Modify: [crates/gestalt-runtime/src/extension/command_tool.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/command_tool.rs)
- Modify: [crates/gestalt-runtime/src/extension/mcp_component.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/mcp_component.rs)
- Modify: [crates/gestalt-runtime/src/process_extension.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/process_extension.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/process_client.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/process_client.rs)
- Test: [crates/gestalt-runtime/tests/runtime_permissions_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_permissions_tests.rs)
- Test: [crates/gestalt-runtime/tests/package_mcp_component_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/package_mcp_component_tests.rs)
- Test: [crates/gestalt-runtime/tests/command_tool_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/command_tool_tests.rs)

- [ ] Add failing tests proving permissions are enforced for legacy processes, lifecycle processes, command tools, stdio MCP, HTTP MCP, and skill/package resource boundaries.
- [ ] Introduce `HostLaunchContext` with event bus, workspace root, effective permissions, timeout values, message limits, pending-request limits, environment policy, and package source root.
- [ ] Apply effective permissions to every backend, not only `LocalProcessLauncher`.
- [ ] Ensure command tools and MCP components resolve working directories and relative paths against the package source root, never implicitly against the workspace root.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test runtime_permissions_tests \
  --test package_mcp_component_tests \
  --test command_tool_tests
```

## Task 5: Implement Unified Component Activation

**Files:**
- Modify: [crates/gestalt-runtime/src/activation.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/activation.rs)
- Modify: [crates/gestalt-runtime/src/builder.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/builder.rs)
- Modify: [crates/gestalt-runtime/src/discovery.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/discovery.rs)
- Modify: [crates/gestalt-runtime/src/extension/package.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/package.rs)
- Modify: [crates/gestalt-runtime/src/process_extension.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/process_extension.rs)
- Test: [crates/gestalt-runtime/tests/runtime_builder_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_builder_tests.rs)
- Test: [crates/gestalt-runtime/tests/lifecycle_process_client_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/lifecycle_process_client_tests.rs)
- Test: [crates/gestalt-runtime/tests/runtime_process_extension_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_process_extension_tests.rs)

- [ ] Add failing tests showing startup and reload both call the same activation pipeline and that normal package discovery still launches legacy v1 packages.
- [ ] Activate `CommandTool`, `LegacyProcess`, `GestaltLifecycle`, and `McpServer` through one shared pipeline.
- [ ] Make candidate ownership explicit: newly created resources are owned by `ActivationCandidate` until commit, then transferred to manager ownership.
- [ ] Treat optional-component activation failures as degradations with explicit diagnostics rather than aborting the entire build.
- [ ] Validate all namespaces before publication: tool runtime names, canonical tool IDs, MCP server names, context source IDs, lifecycle capability IDs, skill IDs, and client descriptor IDs.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test runtime_builder_tests \
  --test lifecycle_process_client_tests \
  --test runtime_process_extension_tests
```

## Task 6: Make Lifecycle Plans Executable and Specify Semantics

**Files:**
- Modify: [crates/gestalt-runtime/src/lifecycle/plan.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/plan.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/policy_guard.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/policy_guard.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/turn_router.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/turn_router.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/verifier.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/verifier.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/event_observer.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/event_observer.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/process_client.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/process_client.rs)
- Modify: [crates/gestalt-runtime/src/runtime.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/runtime.rs)
- Test: [crates/gestalt-runtime/tests/lifecycle_composition_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/lifecycle_composition_tests.rs)
- Test: [crates/gestalt-runtime/tests/runtime_run_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_run_tests.rs)

- [ ] Add failing tests for ordering, reducer behavior, routing conflicts, verifier failure modes, observer boundedness, timeout handling, and data-scope filtering.
- [ ] Implement these execution rules:
  - ordering: priority descending, then canonical component ID ascending;
  - policy: base policy always runs and extension guards can only tighten it;
  - policy reducer: `Deny > RequireApproval > Annotate > Abstain`;
  - context: extensions return data, not trusted `Message::System`;
  - routing: highest-priority stop wins, else highest-priority route, equal-priority conflicts are ignored and traced;
  - verification: collect reports and honor declared failure mode on transport failure;
  - observers: bounded, non-authoritative, non-blocking;
  - timeouts: per capability registration;
  - failure-mode defaults: explicit per capability kind;
  - data scope: host filters payloads before invocation.
- [ ] Replace generic lifecycle payloads with versioned request/response DTOs.
- [ ] Acquire a `RuntimeSnapshotLease` at run start and execute all lifecycle behavior against the pinned snapshot only.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test lifecycle_composition_tests \
  --test runtime_run_tests
```

## Task 7: Implement Transactional Startup and Reload

**Files:**
- Modify: [crates/gestalt-runtime/src/activation.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/activation.rs)
- Modify: [crates/gestalt-runtime/src/control.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/control.rs)
- Modify: [crates/gestalt-runtime/src/extension/manager.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/manager.rs)
- Modify: [crates/gestalt-runtime/src/builder.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/builder.rs)
- Test: [crates/gestalt-runtime/tests/extension_reload_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/extension_reload_tests.rs)
- Test: [crates/gestalt-runtime/tests/extension_manager_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/extension_manager_tests.rs)

- [ ] Add failing tests for `dry_run`, `force`, `instance_id`, candidate rollback, deferred retirement, and unchanged active generation on candidate failure.
- [ ] Define reload semantics exactly:
  - `dry_run`: build, launch, and negotiate candidate as needed; never publish; tear down newly created resources afterward;
  - `force`: bypass reuse for selected components even when fingerprints match;
  - `instance_id`: rebuild only the targeted instance; unknown instance is an error; other instances are retained from the active composition;
  - no target: rediscover and rebuild full composition.
- [ ] Expand reload reporting to include:

```rust
pub struct ReloadExtensionsReport {
    pub previous_generation: RuntimeGeneration,
    pub candidate_generation: RuntimeGeneration,
    pub candidate_fingerprint: RuntimeFingerprint,
    pub published: bool,
    pub added: Vec<ComponentInstanceId>,
    pub removed: Vec<ComponentInstanceId>,
    pub replaced: Vec<ComponentInstanceId>,
    pub reused: Vec<ComponentInstanceId>,
    pub degraded: Vec<ComponentDiagnostic>,
    pub errors: Vec<ComponentDiagnostic>,
}
```

- [ ] Make startup and reload both call `ExtensionActivationPipeline`, and only publish after collision validation, capability negotiation, and resource readiness succeed.
- [ ] On commit, retire the previous generation, keep leased retired generations callable, and only drain non-reused resources after the last lease releases.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test extension_reload_tests \
  --test extension_manager_tests
```

## Task 8: Finalize Host Control, Health, and Approval Flow

**Files:**
- Modify: [crates/gestalt-runtime/src/control.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/control.rs)
- Modify: [crates/gestalt-runtime/src/orchestration.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/orchestration.rs)
- Modify: [crates/gestalt-runtime/src/artifact_store.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/artifact_store.rs)
- Modify: [crates/gestalt-runtime/src/runtime.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/runtime.rs)
- Modify: [crates/gestalt-runtime/src/extension/manager.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/manager.rs)
- Test: [crates/gestalt-runtime/tests/runtime_control_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_control_tests.rs)
- Test: [crates/gestalt-runtime/tests/runtime_orchestration_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_orchestration_tests.rs)

- [ ] Add failing tests for shared events, artifacts, steering, approval response, shared generation, and combined health reporting.
- [ ] Make `RuntimeHost` own session registry, extension manager, artifact store, steering queues, event bus, and approval broker.
- [ ] Define `extension_health()` as the combination of immutable activation diagnostics from the active snapshot and live operational health from the manager.
- [ ] Remove `panic!` and “unsupported” behavior from the host control API; use structural separation instead.
- [ ] Verify with:

```bash
cargo test -p gestalt-runtime \
  --test runtime_control_tests \
  --test runtime_orchestration_tests
```

## Task 9: Harden Fingerprints, Cancellation, and Full Documentation

**Files:**
- Modify: [crates/gestalt-runtime/src/extension/package.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/extension/package.rs)
- Modify: [crates/gestalt-runtime/src/process_extension.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/process_extension.rs)
- Modify: [crates/gestalt-runtime/src/lifecycle/process_client.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/src/lifecycle/process_client.rs)
- Modify: [crates/gestalt-runtime/REFERENCE.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/REFERENCE.md)
- Modify: [docs/adrs/ADR-002-runtime-snapshot-reload.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/adrs/ADR-002-runtime-snapshot-reload.md)
- Modify: [docs/adrs/ADR-003-lifecycle-protocol-v2.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/adrs/ADR-003-lifecycle-protocol-v2.md)
- Modify: [docs/feature-spec/product-neutral-extension-architecture.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/feature-spec/product-neutral-extension-architecture.md)
- Modify: [docs/extension-development-guide.md](/home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/extension-development-guide.md)
- Test: [crates/gestalt-runtime/tests/runtime_snapshot_tests.rs](/home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-runtime/tests/runtime_snapshot_tests.rs)
- Test: workspace verification commands

- [ ] Add failing tests for protocol-v2 cancellation declaration, non-file entrypoint fingerprinting (`python -c`, `python -m`), secret-safe trust/config fingerprints, and negotiated descriptor fingerprint changes.
- [ ] Ensure fingerprints cover protocol version, declared cancellation support, trust state, optional/failure semantics, lifecycle descriptors, direct MCP configuration, declared and resolved entrypoints, and non-file arguments without exposing raw secrets.
- [ ] Update `REFERENCE.md` for every API and invariant change in this plan, including `RuntimeHost`, `ExtensionActivationPipeline`, leases, retirement rules, permission schema, lifecycle reducers, reload semantics, and health model.
- [ ] Update docs so they claim only implemented behavior.
- [ ] Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Acceptance Checklist

- [ ] `RuntimeHost` is the single host control boundary for one workspace and one extension-generation lineage.
- [ ] Startup and reload both use one `ExtensionActivationPipeline`.
- [ ] `ActivationCandidate` owns rollback cleanup until commit.
- [ ] `RuntimeSnapshotLease` and deferred retirement are implemented before transactional reload publication.
- [ ] Effective permissions are enforced as `manifest request ∩ instance grant ∩ host policy`.
- [ ] V2 packages declare requested permissions explicitly enough to compute the intersection.
- [ ] Command tools, legacy processes, lifecycle processes, stdio MCP, HTTP MCP, and skill/package resources all honor effective permissions.
- [ ] Package-relative entrypoints and paths resolve against package source roots and are reflected in fingerprints and diagnostics.
- [ ] Legacy v1 packages discovered through normal package discovery are still launched and exposed.
- [ ] `GestaltLifecycle` components negotiate capabilities into executable lifecycle plans.
- [ ] Lifecycle execution semantics are specified and implemented exactly as planned.
- [ ] `reload_extensions()` preserves the active generation on candidate failure and honors `dry_run`, `force`, and `instance_id`.
- [ ] Generation retirement includes processes, MCP resources, and long-lived observer workers.
- [ ] Immutable activation diagnostics are kept separate from mutable live operational health.
- [ ] Protocol-v2 cancellation semantics are explicit and tested.
- [ ] All public API and contract changes are reflected in `crates/gestalt-runtime/REFERENCE.md`.

## Execution Order

1. Task 1
2. Task 2
3. Task 3
4. Task 4
5. Task 5
6. Task 6
7. Task 7
8. Task 8
9. Task 9

Reasoning: contract decisions and host ownership must land first; permission and trust contracts must be defined before enforcement; resource lifetime and lease semantics must exist before reload; backend enforcement must precede unified activation; executable lifecycle plans depend on unified activation; transactional startup and reload depend on all of the above; host control finalization follows once the manager and reload model are stable; documentation and full verification are last.
