---
title: Gestalt Trace and Client Event Contracts
status: published
type: version-contract
target: v0.1
---

# Trace and Client Event Contracts

Gestalt has three distinct event representations:

```text
gestalt_core::AgentEvent                 internal live runtime event
gestalt_runtime::unstable::TraceEventV1 persisted audit/replay event
gestalt_runtime::api::v1::ClientEventPayloadV1
                                         product-facing event
```

They are not interchangeable contracts.

## Persisted trace

Each JSONL line is an `EventEnvelope` containing `v`, `session_id`, `run_id`,
`turn_id`, `seq`, `ts`, `event`, and `redacted`. Trace schema version 1 is
lossless enough for replay and recovery, so trace events can contain internal
checkpoint history, projection state, token budgets, tool mappings, and
structured failure data. Trace files are therefore private run artifacts, not
client API responses.

The trace reader rejects unsupported schema versions and malformed known
events. It skips unknown future event kinds while preserving known-event order.
Core-to-trace and trace-to-core conversions are fallible; runtime paths do not
panic when schemas diverge.

## Client projection

`ClientEventRecordV1` preserves only envelope ordering and identity:

```text
v, session_id, run_id, turn_id, seq, ts, payload, redacted
```

`ClientEventPayloadV1` exposes product-neutral run, message, context, model,
tool, policy, approval, artifact, usage, stop, error, and lifecycle events.
Checkpoint history, context projection state, token budgets, raw messages,
queued messages, tool mappings, history ranges, provider request hashes,
working directories, artifact paths, and internal failure structures are
omitted.

Known textual fields pass through trace redaction again during projection.
Checkpoint and other internal-rich events become metadata-only context or
lifecycle payloads. A future event kind projected directly from a raw trace
line becomes:

```json
{"type":"unknown","kind":"future_kind"}
```

Use `project_client_event_line` for JSONL-to-client projection. CLI JSON tail
output uses this path; text rendering may skip unknown trace kinds.

## Enforcement

`crates/gestalt-runtime/tests/trace_contract_tests.rs` enforces:

- known trace event round trips;
- bad-version rejection and documented unknown trace behavior;
- fallible, non-panicking core/trace conversion;
- client redaction and internal checkpoint omission;
- session/run/turn/sequence/timestamp preservation;
- forward-compatible unknown client payloads.
