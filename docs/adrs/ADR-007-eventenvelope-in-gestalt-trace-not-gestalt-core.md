# ADR-007: EventEnvelope in gestalt-trace, Not gestalt-core

**Status:** Accepted

## Context
The PRD's `EventEnvelope` used `chrono::DateTime<Utc>`, but `chrono` was not in the gestalt-core dependency budget. Timestamps and session IDs are trace concerns, not runtime concerns.

## Decision
`AgentEvent` in core has no timestamps. `EventEnvelope` in gestalt-trace adds `ts`, `session_id`, `turn_id`, `seq`.

## Consequences
Core stays pure. Trace format can evolve without touching core. Tests of the loop emit raw `AgentEvent`s without needing a timestamp.
