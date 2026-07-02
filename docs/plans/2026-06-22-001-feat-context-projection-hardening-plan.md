---
title: "feat: Context projection hardening"
type: feat
status: proposed
date: 2026-06-22
origin: docs/feature-spec/context-projection-hardening.md
related:
  - docs/feature-spec/context-projection-hardening.md
  - docs/feature-spec/context-tool-compaction.md
  - docs/gestalt-harness-architecture.md
  - docs/adrs/ADR-023-runtime-composition-layer.md
  - docs/adrs/ADR-026-cache-aware-prompt-assembly.md
---

# Context Projection Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make canonical session history fully append-only and move all clearing, compaction, and omission behavior into provider-visible projections backed by durable core-owned projection state.

**Architecture:** `gestalt-core` will own canonical message identity and durable `ContextProjectionState`, `gestalt-runtime` will own planning, compactor invocation, artifact I/O, and transactional state commits, `gestalt-context` will provide pure selection/assembly/validation algorithms, and `gestalt-trace` will persist session checkpoints and projection manifests without inventing canonical identity.

**Tech Stack:** Rust workspace crates (`gestalt-core`, `gestalt-context`, `gestalt-runtime`, `gestalt-trace`, `gestalt-cli`), serde-serializable domain types, existing compaction/checkpoint infrastructure, existing runtime tests and replay fixtures.

---

## Summary

This feature replaces the current partially split context-management architecture with a request-based, transactional pipeline. The most important structural changes are:

* `Session.history` becomes `Vec<SessionMessage>` with stable `MessageId` values.
* `Session` owns durable `ContextProjectionState`.
* Context preparation returns `PreparedContext` plus `ContextStateDelta` and does not mutate session state during planning.
* Tool-result clearing is driven by a namespaced retention snapshot rather than hard-coded tool names.
* Checkpoints, tombstones, manifests, resume, and branching all key off stable message identity.

No migration work is required beyond updating in-repo code and tests. Treat the workspace as greenfield for checkpoint compatibility.

---

## Scope Boundaries

* Do not introduce a new event-sourcing store or database.
* Do not implement semantic retrieval, vector memory, or automatic artifact rehydration.
* Do not preserve the old `ContextPipeline` contract as the long-term architecture.
* Do not let `gestalt-context` query runtime registries or artifact stores directly.

---

## Implementation Units

### U1. Add core-owned canonical message envelopes

**Goal:** Introduce stable session-scoped message identity and switch canonical history to `Vec<SessionMessage>`.

**Requirements:** Stable IDs, append-only canonical history, provider-neutral projection rendering.

**Dependencies:** None

**Files:**
- Modify: `crates/gestalt-core/src/session.rs`
- Modify: `crates/gestalt-core/src/lib.rs`
- Modify: `crates/gestalt-core/src/agent.rs`
- Modify: `crates/gestalt-core/src/event.rs`
- Modify: `crates/gestalt-core/src/hook.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/composition_hooks.rs`
- Test: `crates/gestalt-core/tests/session_queue_tests.rs`
- Test: `crates/gestalt-core/tests/agent_queue_drain_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_hooks_tests.rs`

**Approach:**
- Add `SessionId`, `MessageId`, `SessionMessage`, and optional `MessageMetadata` support in `gestalt-core`.
- Change `Session.history` to `Vec<SessionMessage>`.
- Add a single append API on `Session` or a small helper used by the agent loop and runtime so message ID allocation is centralized.
- Update all history readers to use `session_message.message` when they need provider-visible content and `session_message.id` when they need canonical references.
- Keep provider request rendering on plain `Message`.

**Patterns to follow:**
- Existing pure-domain ownership in `gestalt-core`.
- Existing session lifecycle and checkpoint emission flow in `crates/gestalt-core/src/agent.rs`.

**Test scenarios:**
- Appending user, assistant, and tool-result messages allocates monotonically increasing `MessageId.sequence` values per session.
- Checkpoint events serialize the new canonical envelope structure without losing message content.
- Hooks and runtime consumers continue to see canonical history in deterministic order.

**Verification:**
- `cargo test -p gestalt-core`
- `cargo test -p gestalt-runtime runtime_hooks_tests session_queue_tests`

---

### U2. Add durable `ContextProjectionState` and checkpoint persistence

**Goal:** Move durable projection-control state into `Session` and persist it through checkpoints and resume.

**Requirements:** Resumable checkpoint refs, cleared-result refs, prompt snapshot refs, context epoch, policy fingerprint.

**Dependencies:** U1

**Files:**
- Modify: `crates/gestalt-core/src/session.rs`
- Modify: `crates/gestalt-core/src/context.rs`
- Modify: `crates/gestalt-trace/src/context_artifacts.rs`
- Modify: `crates/gestalt-trace/src/resume.rs`
- Modify: `crates/gestalt-trace/src/lib.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Test: `crates/gestalt-trace/tests/replay_tests.rs`
- Test: `crates/gestalt-runtime/tests/context_management_tests.rs`

**Approach:**
- Add `ContextProjectionState`, `ArtifactRef`, `CompactionCheckpointRef`, `PromptSnapshotRef`, and `ClearedToolResultRef` as serializable core types.
- Extend checkpoint persistence to store `SessionCheckpointV2` with `history`, `context_state`, `token_budget`, and `latest_projection_id`.
- Remove private runtime ownership of active checkpoint state and source it from `Session.context_state`.
- Update resume to restore `ContextProjectionState` directly from persisted checkpoints.

**Patterns to follow:**
- Existing trace persistence APIs in `crates/gestalt-trace/src/context_artifacts.rs`.
- Existing resume analyzer contract in `crates/gestalt-trace/src/resume.rs`.

**Test scenarios:**
- A checkpoint with an active compaction reference round-trips through trace persistence and resume.
- Cleared tool-result references and prompt snapshot refs survive serialization.
- Resume restores context epoch and latest projection ID without reconstructing state from provider-visible messages.

**Verification:**
- `cargo test -p gestalt-trace replay_tests`
- `cargo test -p gestalt-runtime context_management_tests`

---

### U3. Replace the context contract with request-based transactional preparation

**Goal:** Change the context pipeline boundary from implicit history processing to explicit request/result contracts with a state delta.

**Requirements:** Request-based contract, transactional delta, no direct session mutation during planning.

**Dependencies:** U1, U2

**Files:**
- Modify: `crates/gestalt-core/src/context.rs`
- Modify: `crates/gestalt-core/src/agent.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-runtime/src/builder.rs`
- Test: `crates/gestalt-runtime/tests/context_management_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_builder_tests.rs`

**Approach:**
- Define `ContextPreparationRequest`, `PreparedContext`, and `ContextStateDelta` in `gestalt-core`.
- Update the `ContextPipeline` trait to accept a request and return a prepared result.
- Make `RuntimeContextPipeline` build the prepared result without mutating `Session`.
- Add a small runtime commit path that applies `ContextStateDelta` only after artifact persistence, validation, and manifest persistence have all succeeded.
- Keep commit logic in runtime rather than inside `gestalt-context`.

**Patterns to follow:**
- Existing runtime composition ownership from ADR-023.
- Existing explicit `Result`-based lifecycle boundaries in `gestalt-core`.

**Test scenarios:**
- Failed compaction validation returns an error and leaves `Session.context_state` unchanged.
- Successful preparation commits the returned delta and updates active checkpoint or cleared-result state exactly once.
- The agent loop uses the prepared packet and manifest without performing hidden secondary mutation.

**Verification:**
- `cargo test -p gestalt-runtime context_management_tests runtime_builder_tests`

---

### U4. Split planning from assembly and make compaction projection-only

**Goal:** Move all policy decisions into runtime planning and reduce `MinimalContextPipeline` to pure assembly.

**Requirements:** Sole runtime policy ownership, chronological provider output, projection-only checkpoint application, no hidden trimming.

**Dependencies:** U3

**Files:**
- Modify: `crates/gestalt-context/src/lib.rs`
- Modify: `crates/gestalt-context/src/compaction.rs`
- Modify: `crates/gestalt-context/src/checkpoint_validation.rs`
- Modify: `crates/gestalt-context/src/accounting.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Modify: `crates/gestalt-runtime/src/compaction.rs`
- Test: `crates/gestalt-context/tests/context_pipeline_tests.rs`
- Test: `crates/gestalt-runtime/tests/context_management_tests.rs`

**Approach:**
- Introduce `ContextPlan` over `SessionMessage` identities rather than history indices.
- Narrow `MinimalContextPipeline` into a pure assembler that resolves `PlannedMessage` values, preserves chronology, and computes final token estimates without selecting or dropping messages on its own.
- Apply checkpoints only while assembling the provider-visible projection.
- Remove any remaining projected pseudo-history replacement logic from runtime.

**Patterns to follow:**
- Existing trust-boundary rendering and prompt snapshot assembly in `gestalt-context`.
- Existing compaction-range and validation helpers, rewritten to accept stable IDs.

**Test scenarios:**
- Hard pressure produces a checkpoint plus recent canonical history without mutating stored canonical history.
- Chronological output remains oldest-to-newest even when the planner selects from newest backward.
- Projection validation rejects orphan tool exchanges and invalid checkpoint refs before provider calls.

**Verification:**
- `cargo test -p gestalt-context`
- `cargo test -p gestalt-runtime context_management_tests`

---

### U5. Add namespaced retention snapshots and artifact-backed tombstones

**Goal:** Replace hard-coded tool-name clearing with immutable retention snapshots keyed by canonical tool IDs.

**Requirements:** Conservative defaults, namespaced tool IDs, artifact-backed cleared results, manifest fingerprinting.

**Dependencies:** U3, U4

**Files:**
- Modify: `crates/gestalt-core/src/tool_descriptor.rs`
- Modify: `crates/gestalt-core/src/context.rs`
- Modify: `crates/gestalt-context/src/tool_clearing.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Modify: `crates/gestalt-runtime/src/tool_catalog.rs`
- Modify: `crates/gestalt-runtime/src/registry.rs`
- Modify: `crates/gestalt-trace/src/context_artifacts.rs`
- Test: `crates/gestalt-runtime/tests/context_management_tests.rs`
- Test: `crates/gestalt-runtime/tests/runtime_registry_tests.rs`

**Approach:**
- Add `ToolRetention` and `ToolRetentionRegistrySnapshot` types, using `CanonicalToolId` keys and a deterministic fingerprint.
- Build the snapshot from the active composed catalog in runtime and pass it into context preparation.
- Update tool clearing to resolve retention policy from the request snapshot and default to non-clearable behavior when no policy exists.
- Persist artifact references for cleared results and include the retention fingerprint in projection manifests.

**Patterns to follow:**
- Existing canonical tool ID and annotation support in `crates/gestalt-core/src/tool_descriptor.rs`.
- Existing artifact persistence primitives and tool execution artifact handling.

**Test scenarios:**
- Built-in readable tools can be tombstoned when policy allows it.
- Mutating or unknown tools remain fully retained by conservative default.
- Manifest records which retention snapshot fingerprint governed the clear action.

**Verification:**
- `cargo test -p gestalt-runtime context_management_tests runtime_registry_tests`

---

### U6. Finalize branching, explainability, and golden fixtures

**Goal:** Finish branch filtering rules, explainability artifacts, and full regression coverage for the hardened lifecycle.

**Requirements:** Branch-safe projection state, explainable manifests, stable cache behavior, end-to-end replay/resume coverage.

**Dependencies:** U1-U5

**Files:**
- Modify: `crates/gestalt-runtime/src/orchestration.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Modify: `crates/gestalt-trace/src/context_artifacts.rs`
- Modify: `crates/gestalt-cli/src/main.rs`
- Modify: `crates/gestalt-cli/tests/context_cli_tests.rs`
- Test: `crates/gestalt-runtime/tests/context_management_tests.rs`
- Test: `crates/gestalt-trace/tests/golden_trace_tests.rs`
- Test: `crates/gestalt-trace/tests/replay_tests.rs`

**Approach:**
- Define branch filtering logic for inherited checkpoint refs, cleared-result refs, and prompt snapshots.
- Expand projection manifests to use stable message IDs throughout omission and selection records.
- Add or complete `gestalt context explain` using persisted manifest data rather than recalculating policy decisions.
- Add golden fixtures for resume-after-compaction, branch-before-compaction, stable-prefix determinism, and manifest completeness.

**Patterns to follow:**
- Existing orchestration and artifact inspection surfaces in `gestalt-runtime` and `gestalt-cli`.
- Existing trace golden fixtures and replay test patterns.

**Test scenarios:**
- Branching from a point before a checkpoint drops out-of-scope projection state and rebuilds cleanly.
- `context explain` surfaces checkpoint refs, clear actions, omitted message IDs, and stable prefix fingerprint data from the manifest.
- Golden traces distinguish canonical session history from provider-visible projections.

**Verification:**
- `cargo test -p gestalt-runtime context_management_tests`
- `cargo test -p gestalt-trace golden_trace_tests replay_tests`
- `cargo test -p gestalt-cli context_cli_tests`

---

## System-Wide Impact

* **Core API impact:** `Session.history`, context trait contracts, and checkpoint payloads all change together. This is the main structural cut and should land before behavioral hardening work.
* **Runtime behavior impact:** `RuntimeContextPipeline` becomes the only policy owner and gains explicit commit responsibility for `ContextStateDelta`.
* **Trace impact:** Checkpoints and manifests become more explicit and more faithful to canonical ownership, while replay and resume stop depending on implicit runtime state.
* **Testing impact:** Existing context, replay, runtime, and CLI tests need coordinated updates because the canonical history type changes across crate boundaries.

---

## Risks & Mitigations

| Risk | Mitigation |
| --- | --- |
| The `SessionMessage` cut touches many call sites and can create broad compile churn. | Land U1 first and keep provider-facing rendering on `Message` to minimize deeper rewrites. |
| Transactional preparation may accidentally reintroduce hidden mutation through helper state. | Keep `ContextStateDelta` as the only writable projection-state output and assert unchanged session state on failure paths. |
| Tool-retention snapshot wiring could become coupled to runtime internals. | Pass only immutable snapshots into context preparation and keep `gestalt-context` free of registry access. |
| Branch semantics can drift from checkpoint/source-range reality. | Add explicit branch fixtures that validate source-range inclusion and artifact/hash checks. |

---

## Acceptance Check

The implementation is ready to merge when:

* canonical history is envelope-based and append-only;
* projection state lives on `Session` and round-trips through checkpoints;
* context preparation is request-based and transactional;
* compaction and clearing affect only provider-visible projections;
* hard-coded tool-name clearing is removed;
* manifests, replay, resume, and branching all key off stable message IDs;
* existing runtime, trace, and CLI tests are green with new lifecycle fixtures added.
