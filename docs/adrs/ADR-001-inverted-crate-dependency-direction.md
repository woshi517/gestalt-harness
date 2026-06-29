# ADR-001: Inverted Crate Dependency Direction

**Status:** Accepted

## Context
The PRD showed `gestalt-core` depending on `gestalt-models`, `gestalt-tools`, and `gestalt-trace`. This would make core non-pure and force downstream library users to inherit the full stack.

## Decision
All concrete crates depend on core. Core defines only traits and types. `gestalt-runtime` acts as the composition layer, and concrete applications like `gestalt-cli` orchestrate it.

## Consequences
Library consumers can use `gestalt-runtime` to embed a fully composed agent loop. Core stays pure and under 7 direct dependencies.
