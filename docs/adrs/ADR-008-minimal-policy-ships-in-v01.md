# ADR-008: Minimal Policy Ships in v0.1

**Status:** Accepted

## Context
The PRD listed the policy engine as a v0.2 non-goal, but shipped BashTool, WriteTool, and WebFetchTool in v0.1. This is unsafe.

## Decision
v0.1 includes a minimal policy engine covering: workspace path allow/deny, network on/off, bash command allow/confirm/deny, medium/high-risk confirmation, output size cap, and execution timeout.

## Consequences
v0.1 is safe to use. The complete policy grammar and advanced MCP/skill permissions are v0.2.
