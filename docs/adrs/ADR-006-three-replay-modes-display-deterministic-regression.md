# ADR-006: Three Replay Modes (Display, Deterministic, Regression)

**Status:** Accepted

## Context
The PRD said replay "reproduces outputs offline without provider calls" without clarifying that recorded events and re-execution are different operations.

## Decision
Three distinct modes with explicit semantics. Default is `display` (event replay only). Re-execution requires opt-in.

## Consequences
`gestalt replay` is safe by default — no side effects, no API calls. Users who need re-execution choose the appropriate mode.
