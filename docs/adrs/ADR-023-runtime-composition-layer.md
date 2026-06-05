# ADR-023: Runtime Composition Layer

**Status:** Accepted

## Context
Downstream applications embedding the Gestalt agent harness needed a unified, stable API to manage tools, hooks, events, and sessions. Prior to this, CLI execution paths duplicated the setup of provider, tools, context pipeline, policy, approval, and tracing, which forced developers to copy CLI wiring.

## Decision
Introduce the `gestalt-runtime` crate as the primary orchestration and composition shell above the pure kernel (`gestalt-core`).
- Introduce `AgentRuntimeBuilder` and a deterministic `RuntimeRegistry`.
- Introduce `RuntimeEventBus` and `RuntimeEvent` to provide auditability above `AgentEvent`.
- Define `AgentRuntimeHandle` and `Orchestrator` traits to support multi-agent/session coordination and artifact handoff.

## Consequences
- Clean separation of CLI/UX rendering from agent loop logic.
- Simplified API surface for embedding the harness in third-party services and products.
- Preserved purity of `gestalt-core` which remains agnostic to runtime orchestration, extensions, or workflow systems.
