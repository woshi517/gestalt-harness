---
title: "Context Build Report v1"
status: proposed
type: version-contract
target: v0.1
owners:
  - gestalt-runtime
authority: implementation-contract
---

# Context Build Report v1

`ContextBuildReportV1` is the proposed stable diagnostic boundary for context construction.
Clients must not depend on `PreparedContext`, projection plans, or compaction
implementation types.

## Determinism

- `packet_id` is the hash of the canonical projected packet.
- `pipeline_id` hashes the context policy, model capabilities, runtime, tools,
  workspace snapshot, and canonically ordered contributor records.
- Sources are ordered by kind, label, trust, and authority. Omissions are ordered
  by source and reason code, independent of contributor registration order.
- Deterministic replay reads `CapturedContributionV1` and verifies its SHA-256
  hash and byte size. It does not invoke the contributor again.
- A live replay sets `deterministic` to `false`.

## Bounds And Persistence

Only redacted contribution content may be passed to `capture_redacted`; each
capture is limited to 256 KiB and all captures in one report are limited to
1 MiB. The applied limits are included in every report.

Reports are stored as `context_report_<report_id>.json`. Readers reject missing
or unsupported versions and corrupted captures. Required persistence returns an
error; best-effort persistence returns `CONTEXT_REPORT_PERSISTENCE_FAILED` as a
structured diagnostic and never writes presentation output.

## Evidence

| Criteria | Evidence |
|---|---|
| H2B-F01-F04, H2B-B01 | `context_report_contract_tests::report_identity_is_independent_of_source_registration_order` |
| H2B-B03, H2B-B06 | `context_report_contract_tests::persisted_report_round_trips_and_rejects_unsupported_version`, `context_report_contract_tests::best_effort_persistence_returns_structured_diagnostic`, `context_report_contract_tests::replay_rejects_tampered_capture` |
| H2B-F06, H2B-B04 | `CapturedContributionV1::capture_redacted`, report bound fields |
| H2B-B02 | `context_report_contract_tests::deterministic_replay_does_not_repeat_contributor_side_effect` |
| H2B-B05 | Existing context projection and compaction history-preservation tests |

The H2B-F05 H2A linkage remains in runtime wiring and should be covered by a dedicated integration test before this contract is published.
