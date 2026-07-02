---
title: "Context Build Report v1"
status: published
type: version-contract
target: v0.1
owners:
  - gestalt-runtime
authority: implementation-contract
---

# Context Build Report v1

`ContextBuildReportV1` is the stable diagnostic boundary for context construction.
Clients must not depend on `PreparedContext`, projection plans, or compaction
implementation types.

## Determinism

- `packet_id` is the hash of the canonical projected packet.
- `pipeline_id` hashes the context policy, model capabilities, runtime, tools,
  workspace snapshot, and canonically ordered contributor records.
- Sources are ordered by kind, label, trust, and authority. Omissions are ordered
  by source and reason code, independent of contributor registration order.
- Deterministic replay is available only for explicit `full_for_replay`
  captures. It verifies original and captured SHA-256 hashes and byte sizes,
  and does not invoke the contributor again.
- A live replay sets `deterministic` to `false`.

## Bounds And Persistence

Capture policy is part of `context.management.capture`:

| Mode | Stored data | Replay |
|---|---|---|
| `disabled` | no contribution record | unavailable |
| `hash_only` | original byte size and SHA-256 only | unavailable |
| `redacted` | redacted content plus original/captured hashes | unavailable |
| `full_for_replay` | raw content plus hashes | available |

The default is `hash_only`. `full_for_replay` must be selected explicitly and
can persist secrets; use it only for controlled replay artifacts.
`capture_redacted` performs redaction itself, including API keys, tokens,
authorization headers, `.env`-style secrets, provider credentials, keychain
references, common provider-token prefixes, JWT-like values, and URL
credentials.

Each source contribution is limited to 256 KiB and all source contributions in
one report are limited to 1 MiB. The applied limits and capture mode are
included in every report.

Reports are stored as `context_report_<report_id>.json`. Readers reject missing
or unsupported versions and corrupted captures. Required persistence returns an
error; best-effort persistence returns `CONTEXT_REPORT_PERSISTENCE_FAILED` as a
structured diagnostic and never writes presentation output.

`DurabilityMode::Disabled` writes no report. `BestEffort` returns the structured
`CONTEXT_REPORT_PERSISTENCE_FAILED` diagnostic on failure. `Required` returns a
hard trace write error.

`context_report_contract_tests` enforces capture modes, secret removal,
single/aggregate bounds, replay integrity, deterministic ordering, and all
three durability outcomes. `context_management_tests` proves the runtime
pipeline uses the configured policy and only replays an explicit full capture.
