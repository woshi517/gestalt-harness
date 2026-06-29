# ADR-004: PolicyRequest Struct Over name/input Pair

**Status:** Accepted

## Context
The PRD's loop called `policy.evaluate(&name, &input, &session.mode)`. This omits risk, paths, and workspace context that the policy engine needs.

## Decision
The loop computes `tool.risk(&input)` and packages all context into a `PolicyRequest` struct before calling the engine.

## Consequences
Policy evaluation is self-contained and testable. Risk classification is separated from policy logic. The policy engine can make path-aware decisions without reimplementing risk classification.
