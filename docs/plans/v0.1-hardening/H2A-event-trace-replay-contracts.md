# Plan: H2A Event, Trace, and Replay Contracts

## 1. Purpose

Implement one metadata-bearing event source with separate lossless trace and stable typed client projections, then freeze stable-v0.1 replay formats with golden fixtures.

## 2. Requirement IDs Covered

EVT-001, EVT-002, EVT-003, EVT-004, EVT-005; CLEAN-005 where it applies to trace/run/checkpoint readers; Section 22 trace/run versioning criteria.

## 3. Current-State Evidence

- `crates/gestalt-runtime/src/trace/mod.rs::EventEnvelope` stores raw `gestalt_core::AgentEvent`.
- Sequence/timestamp/session/run/turn metadata is added inside `JsonlTraceSink`, while `RuntimeEvent` follows a separate event-bus path.
- `crates/gestalt-runtime/src/trace/run_manifest.rs::RunManifest` uses `v` but directly deserializes current Rust structures.
- Context artifacts load current projection/checkpoint structs directly.
- Golden JSONL fixtures exist under `tests/fixtures/traces/`, but coverage and unknown-field/kind policies are incomplete.

## 4. ADR / Spec Constraints

- H0B has accepted the event-classification decisions this plan depends on.
- All representations originate from one canonical producing path.
- H1A owns subscription semantics; this plan owns event projection and persistence compatibility.
- ADR-031 removes only pre-hardening readers; stable v0.1 begins a new compatibility obligation.

## 5. In Scope

- Canonical metadata-bearing internal record.
- Lossless versioned trace envelope and typed stable client projection.
- Independent trace, run-manifest, client-event, context-projection, and compaction-checkpoint version rules.
- Replay/continue/resume/branch compatibility, lineage, generation metadata, and unknown additive field/kind behavior.
- Golden fixtures and removal of H0C-listed pre-hardening readers.

## 6. Out of Scope

- Client subscription mechanics (H1A/H1B), context contribution semantics (H2B), or changing core execution behavior.
- Provider-native events, UI timeline models, live remote transport, or migration of unreleased formats.

## 7. Dependencies and Blockers

Depends on H1A shared IDs/errors and H0C persistence rows. Trace schema publication remains blocked on the remaining canonical record/client projection split, full replay fixture matrix, and the new contract evidence needed to prove them.

## 8. Proposed Changes

### Functional criteria

- **H2A-F01:** Introduce one internal canonical event record containing sequence, timestamp, runtime generation/fingerprint, session, run, turn, correlation, redaction state, and canonical event data before any trace or client projection.
- **H2A-F02:** Define a lossless versioned trace envelope and a separate typed client event envelope. Every stable event kind has a named payload DTO/schema; neither envelope embeds raw `AgentEvent` or `RuntimeEvent`.
- **H2A-F03:** Route trace persistence and client projection from the same canonical record instance so sequence, identity, generation, and terminal metadata cannot diverge.
- **H2A-F04:** Give trace envelope, run manifest, context projection manifest, compaction checkpoint, and client event envelope independent version discriminators and reader dispatch.
- **H2A-F05:** Persist run kind, parent run, base checkpoint, session lineage, runtime/tool/policy/context compatibility fingerprints, and generation metadata needed to validate continue/resume/branch.
- **H2A-F06:** Remove every H0C-listed pre-hardening reader/migration branch and retain readers only for formats published by stable v0.1.
- **H2A-F07:** Add normalized golden fixtures for every EVT-005 scenario, each declaring which fields are normalized and why.

### Behavioral criteria

- **H2A-B01:** Trace projection is lossless for all canonical event fields required for audit/replay; client projection may omit internal detail only according to its documented per-kind schema.
- **H2A-B02:** Readers accept additive fields and handle unknown kinds exactly as the H0B decision specifies while preserving sequence safety; unsupported versions return the stable incompatible-version error.
- **H2A-B03:** Redaction occurs before both projections. Secrets, provider-native objects, unrestricted host paths, and raw internal error chains never reach persisted or client payloads.
- **H2A-B04:** Continue appends to the existing lineage; resume starts from a compatible checkpoint; branch creates a new lineage with parent/base metadata. None mutates source canonical history.
- **H2A-B05:** A compatibility fingerprint mismatch fails before execution or history mutation and reports the mismatched dimensions.
- **H2A-B06:** Trace write/flush failure follows the accepted core fail/degrade policy and produces an observable diagnostic/terminal state rather than disappearing.

## 9. Public API / Schema / CLI Impact

Replaces raw event exposure in stable trace/client contracts with versioned DTOs. Run/context/checkpoint formats gain explicit independent compatibility rules. CLI consumers must use these readers/projections but CLI envelopes remain H3B-owned.

## 10. Failure, Security, and Compatibility Semantics

- Redaction occurs before persistence/client projection; secrets and provider-native payloads are forbidden.
- Unsupported versions fail deterministically without best-effort migration.
- Unknown fields/kinds follow the accepted per-format rule while preserving sequence/terminal safety.
- Trace write failure follows the H0B/core fail/degrade policy and is observable.
- Resume/branch rejects incompatible fingerprints without mutating source history.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H2A-F*` and `H2A-B*` criterion to a schema, reader, golden, lineage, redaction, or failure-policy test.
- Golden traces: text; allowed/denied/approval-gated tool; pressure/compaction; spillover; cancellation in each phase; extension rejection; continue/resume/branch.
- Round-trip losslessness from canonical record to trace reader and required metadata projection to client event.
- Unknown additive field and unknown event-kind fixtures for every documented behavior.
- Unsupported/pre-hardening version rejection fixtures.
- Manifest lineage/fingerprint compatibility and append-only history checks.
- Normalization helper tests for time, IDs, paths, hashes, and ordering.
- Absence checks for raw `AgentEvent`/`RuntimeEvent` in public trace/client DTOs and removed readers.

## 12. Documentation Updates

Publish test-backed event, trace, run-manifest, and replay contracts under `docs/v0.1/`; update `tests/fixtures/traces/README.md`, trace CLI references, and crate READMEs.

## 13. Execution Steps

1. Add failing schema/reader/golden tests from the accepted decision.
2. Introduce the canonical record and single projection path.
3. Add typed trace/client envelopes and versioned readers.
4. Implement replay/lineage checks and remove pre-hardening readers.
5. Populate normalized golden fixtures and update consumers/documentation.

## 14. Exit Criteria

- [ ] Raw runtime/core event enums are absent from stable client and trace schemas.
- [ ] Trace/run/context/checkpoint/client versions and reader rules are documented and tested.
- [ ] Golden matrix and unknown-field/kind behavior pass.
- [ ] Continue/resume/branch preserve identity and reject incompatibility safely.
- [ ] H0C persistence rows and absence checks are verified.

## 15. Rollback / Partial Completion Handling

Do not write a new stable format until its reader, golden fixtures, and contract docs are complete. If projection migration is partial, keep it internal and continue writing only the pre-freeze development format without compatibility claims.
