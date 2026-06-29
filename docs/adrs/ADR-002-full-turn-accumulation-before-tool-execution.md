# ADR-002: Full Turn Accumulation Before Tool Execution

**Status:** Accepted

## Context
Provider APIs may emit multiple tool calls in a single assistant turn. Executing on partial streamed input violates provider semantics and makes parallel execution unsafe.

## Decision
The `TurnAccumulator` collects all events until `Stop { ToolUse }` or `Stop { EndTurn }`. Only then does execution proceed.

## Consequences
Slightly higher latency before first tool execution. Enables batch policy evaluation and parallel safe execution. Required for correctness on providers that expect all tool results before the next message.
