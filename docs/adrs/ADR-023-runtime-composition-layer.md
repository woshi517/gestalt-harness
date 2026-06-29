# ADR-023: Runtime Composition Layer

**Status:** Accepted

## Context
CLI execution paths duplicated the setup of provider, tools, context pipeline, policy, approval, and tracing. Downstream developers wanting to embed the harness were forced to copy CLI wiring.

## Decision
Introduce the `gestalt-runtime` crate. Construct `AgentRuntime` via `AgentRuntimeBuilder`, registry, and composition hooks without changing the core purity constraint or single-agent `AgentLoop`. Migrate CLI, TUI, chat, and sessions to run turns using `AgentRuntime::run_session` or `AgentRuntime::run_prompt`.

## Consequences
Complete separation of CLI rendering/UX from loop execution. Clear boundary for in-process extensions, and reusable APIs for SDK embedding.
