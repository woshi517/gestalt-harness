# ADR-005: ApprovalProvider as an Injectable Interface

**Status:** Accepted

## Context
The state machine requires suspension and resumption for `confirm` mode. The loop cannot embed CLI-specific prompt logic.

## Decision
`ApprovalProvider` is a trait injected into `AgentLoop`. CLI, TUI, headless, and test implementations are separate.

## Consequences
The loop is UI-independent. Tests use `AutoApprovalProvider`. Dry-run uses `DenyApprovalProvider`. Future GUI approval flows plug in without changing the core.
