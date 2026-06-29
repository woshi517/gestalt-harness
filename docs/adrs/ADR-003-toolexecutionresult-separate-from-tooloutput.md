# ADR-003: ToolExecutionResult Separate from ToolOutput

**Status:** Accepted

## Context
Tool implementations need rich output types (text, JSON, artifact). The agent loop needs a single normalized type for history and event emission.

## Decision
`ToolOutput` is the rich internal type. `ToolExecutionResult` is the normalized loop-facing type. `ToolOutput::into_execution_result()` converts between them.

## Consequences
Tool implementations have expressive outputs. The loop has a single `content: String, is_error: bool` contract. No field access on an enum variant in the loop.
