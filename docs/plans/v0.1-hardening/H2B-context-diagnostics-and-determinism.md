# Plan: H2B Context Diagnostics and Determinism

## 1. Purpose

Expose versioned context-build diagnostics and persist all declared/captured inputs required for deterministic explanation and replay.

## 2. Requirement IDs Covered

CTX-001, CTX-002, CTX-003, CTX-004; CLEAN-005 for pre-hardening context-artifact readers.

## 3. Current-State Evidence

- `gestalt-core/src/context.rs` publicly exposes `PreparedContext`, `ContextPacket`, planning, projection, and manifest types without a client-facing diagnostic boundary.
- Runtime context work is split across `crates/gestalt-runtime/src/context/` and `workspace_context.rs`.
- Projection/checkpoint artifacts are persisted in `crates/gestalt-runtime/src/trace/context_artifacts.rs` using current internal structs.
- Best-effort context persistence now returns structured diagnostics, but the runtime linkage and replay evidence remain incomplete.
- Existing context fixtures focus on internal projection/compaction, not a stable `ContextBuildReportV1`.

## 4. ADR / Spec Constraints

- Depends on the accepted H2A version/identity rules and the H0B classification of context traits/types.
- Stable contracts cover invariants and diagnostics, not compaction/token-estimation algorithms.
- Dynamic contributor outputs are captured and replayed; replay does not repeat external side effects by default.
- Contributors are bounded, ordered, attributed, authority-aware, and cannot mutate canonical history.

## 5. In Scope

- `ContextBuildReportV1`-style DTO with session/run/turn, packet/pipeline identity, tokenizer/estimate, ordered sources/omissions, trust/authority, pressure, captured hashes, and references.
- Deterministic source ordering/conflict rules and context packet/pipeline fingerprints.
- Captured dynamic contributions and versioned persisted context artifacts.
- Replay behavior using captured outputs and structured diagnostics.

## 6. Out of Scope

- Stabilizing `PreparedContext`, planners, compaction heuristics, cache structs, token algorithms, or summarization strategy.
- Re-running live contributors during deterministic replay, UI rendering, or mutating canonical history.

## 7. Dependencies and Blockers

Depends on 000, the accepted H0B API/SPI inventory, H2A IDs/versioning/reader policy, and H0C context-reader rows. Block publication until the remaining H2A identity split, replay capture coverage, and fixture matrix are complete.

## 8. Proposed Changes

### Functional criteria

- **H2B-F01:** Add `ContextBuildReportV1` and supporting DTOs at the H0B-approved client boundary, separate from `PreparedContext`, `ContextPacket`, projection plans, and compaction internals.
- **H2B-F02:** The report contains session/run/turn IDs, packet/pipeline IDs, token estimate/tokenizer identity, pressure classification, ordered sources, omissions, captured-contribution hashes, and prompt/projection artifact references where present.
- **H2B-F03:** Each source record contains contributor identity/version, configuration/output hash, deterministic ordering key, stability, trust, requested/effective authority, token contribution, and capture reference. Each omission contains a stable reason code and affected source/range.
- **H2B-F04:** Compute packet identity from canonical ordered projected content and pipeline identity from context policy, model-capability snapshot, runtime/tool fingerprints, workspace snapshot, and ordered contributor identity/config/output.
- **H2B-F05:** Persist versioned context build reports and bounded dynamic contribution content or logical artifact references with size and integrity metadata; link them from the H2A run/trace records.
- **H2B-F06:** Define a documented per-contribution and aggregate bound using the accepted configuration/constant owner; report the applied bound in diagnostics.

### Behavioral criteria

- **H2B-B01:** Identical declared inputs and captured outputs produce identical source order, packet ID, pipeline ID, omissions, and pressure classification regardless of contributor registration order.
- **H2B-B02:** A dynamic contributor executes once during live construction; deterministic replay verifies and consumes its captured output without repeating the external side effect.
- **H2B-B03:** Missing/tampered captured output, unsupported artifact version, or hash mismatch fails deterministic replay with a structured reason. Explicit live replay is marked non-deterministic in report and trace metadata.
- **H2B-B04:** Over-bound contributions are rejected or omitted according to the accepted contributor policy, with a stable reason; they cannot allocate or persist beyond the bound.
- **H2B-B05:** Contributors cannot mutate canonical session history, and projection/compaction never deletes it.
- **H2B-B06:** Best-effort persistence failure returns a structured diagnostic; required durability failure aborts according to policy. Neither path writes presentation output directly.

## 9. Public API / Schema / CLI Impact

Adds a versioned context diagnostic DTO and persisted artifact schema. Removes any need for clients to parse raw checkpoints, `ContextPacket`, `PreparedContext`, or projection plans. CLI formatting is H3B-owned.

## 10. Failure, Security, and Compatibility Semantics

- Over-limit, conflicting, invalid-authority, failed, or omitted contributors produce stable reason codes.
- Required context durability follows fail behavior; best-effort failures become structured diagnostics, never direct presentation output.
- Captured content is redacted and bounded; secrets are represented only by safe handles/hashes.
- Hash mismatch or missing captured contribution makes deterministic replay fail explicitly.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H2B-F*` and `H2B-B*` criterion to a schema, determinism, capture/replay, bound, history, or durability test.
- Same declared/captured inputs produce identical source order, packet hash, pipeline hash, and report.
- Contributor registration permutations still resolve to documented deterministic order.
- Dynamic side effect executes once; replay uses captured output and verifies hash.
- Bounded contribution, conflict, omission, trust/authority, pressure class, and secret-redaction cases.
- Persist/read round trips, unknown additive field behavior, unsupported/pre-hardening version rejection.
- Canonical history remains unchanged through projection/compaction.
- Compile/schema checks ensure client DTOs do not embed raw projection internals.

## 12. Documentation Updates

Publish the implemented context diagnostic and persisted-artifact contracts under `docs/v0.1/`; consolidate active context specs around the same invariants and update fixture READMEs.

## 13. Execution Steps

1. Add deterministic-order, identity, capture/replay, and schema tests.
2. Define context report/source/omission/pressure DTOs.
3. Add identity computation and bounded captured-contribution records.
4. Persist/read reports through H2A versioned infrastructure; remove old readers.
5. Route diagnostics through app/client projections and update docs/fixtures.

## 14. Exit Criteria

- [ ] Reports explain all included, omitted, trusted, authoritative, and dynamic inputs.
- [ ] Determinism tests prove stable ordering and identities.
- [ ] Replay consumes verified captured contributions by default.
- [ ] Clients require no raw context internals.
- [ ] Context artifacts are independently versioned and H0C rows are verified.

## 15. Rollback / Partial Completion Handling

Do not publish an incomplete context schema or claim deterministic replay without capture verification. If persistence is incomplete, retain diagnostics as internal and keep H2B/H5 blocked.
