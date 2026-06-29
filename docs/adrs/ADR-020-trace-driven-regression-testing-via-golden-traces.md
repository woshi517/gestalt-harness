# ADR-020: Trace-Driven Regression Testing via Golden Traces

**Status:** Accepted

## Context
Testing the behavior of the policy engine, event sequence, and tool execution required running real provider calls, which is slow, non-deterministic, and expensive.

## Decision
Create a regression harness using offline `TraceFixture` and `GoldenTrace` files. Implement `GoldenTraceRunner` to replay sessions against mock provider responses and assert event schemas, ordering, and tool parameters. Define a `TraceEvaluator` trait as an evaluation extension point.

## Consequences
Allows testing policy and loop logic in CI without API keys. Simplifies regression detection for policy changes and provides a hook for future LLM-as-judge evaluation.
