# ADR-023: Runtime Composition Layer

**Status:** Accepted

## Context
Downstream applications embedding the Gestalt agent harness needed a unified, stable API to manage tools, hooks, events, and sessions. Prior to this, CLI execution paths duplicated the setup of provider, tools, context pipeline, policy, approval, and tracing, which forced developers to copy CLI wiring.

## Decision
Introduce the `gestalt-runtime` crate as the primary orchestration and composition shell above the pure kernel (`gestalt-core`).
- Introduce `AgentRuntimeBuilder` and a deterministic `RuntimeRegistry`.
- Introduce `RuntimeEventBus` and `RuntimeEvent` to provide auditability above `AgentEvent`.
- Define `AgentRuntimeHandle` and `Orchestrator` traits to support multi-agent/session coordination and artifact handoff.
- Introduce `CompositionHooks` with five lifecycle interception points (before_context_build, after_context_build, before_tool_policy, after_tool_result, on_event) and hook outcome chaining via `ComposedCompositionHooks`.
- Introduce `RuntimeConfig` to centralize execution parameters (workspace_root, execution_mode, max_turns, model, provider, token budgets, timeouts, network access, environment).
- Introduce `RuntimePolicyEngine` to wrap the base `PolicyEngine` with hook-aware pre-policy blocking (fail-closed on hook errors).
- Introduce `RuntimeContextPipeline` to inject context contributor patches into the assembled prompt.
- Introduce `RuntimeInspect` for introspection snapshots (provider, tool schema hashes, policy fingerprints, hook contract hashes).
- Introduce `ComposedToolCatalog` to merge base tools with extension-registered tools, with collision detection and deterministic ordering.
- Introduce `ArtifactStore` (in-memory and filesystem-backed) for session artifact handoff between orchestrator-managed agents.

## Consequences
- Clean separation of CLI/UX rendering from agent loop logic.
- Simplified API surface for embedding the harness in third-party services and products.
- Preserved purity of `gestalt-core` which remains agnostic to runtime orchestration, extensions, or workflow systems.
