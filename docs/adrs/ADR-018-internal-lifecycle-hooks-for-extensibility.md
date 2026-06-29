# ADR-018: Internal Lifecycle Hooks for Extensibility

**Status:** Accepted

## Context
Custom behaviors (logging, auditing, evaluation, tracing) needed to run at specific points in the agent loop. Introducing a dynamic plugin system in v0.1 would overcomplicate the codebase and destabilize the harness API.

## Decision
Implement internal, static Rust trait hooks (`SessionHook`, `ContextHook`, `ModelHook`, `ToolHook`, `VerificationHook`, `TraceHook`) invoked at lifecycle seams. Change the runner to abort on trace emission failures rather than swallowing them.

## Consequences
Crate-private extensibility is locked in without public plugin API commitments. Robust trace persistence is guaranteed by failing the run when the JSONL trace sink cannot write.
