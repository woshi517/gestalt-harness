# Plan: H1A Runtime Control DTOs and Semantics

## 1. Purpose

Implement the H0B-approved product-neutral control contract as narrow capability interfaces and versioned DTOs, without implementing a product adapter or transport.

## 2. Requirement IDs Covered

RUNTIME-001, RUNTIME-002, RUNTIME-003, RUNTIME-004, RUNTIME-005; POL-001, POL-002 where represented in DTOs; Section 18 remote-readiness constraints.

## 3. Current-State Evidence

- `crates/gestalt-runtime/src/control.rs` exposes broad `RuntimeControl`/`HostControl` traits using raw `RuntimeConfig`, `RunResult`, `RuntimeEvent`, `ApprovalDecision`, and `QueueAck`.
- Subscription exposes `tokio::sync::broadcast::Receiver`; artifact reads return unbounded `Vec<u8>` and identify artifacts by session/name.
- Existing steering behavior lives in `gestalt-core/src/session_queue.rs` and `gestalt-runtime/src/session_queue.rs`.
- Approval models in `gestalt-core/src/approval.rs` do not by themselves define client expiry, duplicate, late, edit-hash, or bounded-grant semantics.

## 4. ADR / Spec Constraints

- **Blocked until H0B accepts** stable exports, operation semantics, raw-model/DTO boundary, and the minimum client event representation.
- Preserve the existing steering queue; do not create a second message path.
- Capability traits are semantic in-process contracts, not a wire protocol.
- Public artifact references use logical IDs/URIs and relative display paths; secrets and provider-native values never cross the boundary.

## 5. In Scope

- `SessionControlV1`, `RunQueryV1`, `ApprovalControlV1`, `EventSourceV1`, `ArtifactAccessV1`, `RuntimeInspectionV1`, and optional `RuntimeControlV1` façade.
- Versioned IDs, requests, acknowledgements, responses, errors, cursors, event projections, policy decisions, approvals, and artifact DTOs.
- Explicit start/continue/resume/branch, queue, idempotency, concurrency, cancellation-race, cursor, and bounded-read semantics.

## 6. Out of Scope

- Local/mock implementations and app-service migration (H1B).
- Full event/trace persistence formats (H2A), remote transport, extension administration stabilization, Workbench types, and UI rendering.
- Exposing registries, artifact stores, raw sessions/events/config, or provider models.

## 7. Dependencies and Blockers

Depends on 000, accepted H0B control/event/API decisions, and H0C deprecated-API ownership. Coordinate identity/event metadata with H2A and context references with H2B. Do not begin dependent code while H0B decisions are proposed.

## 8. Proposed Changes

### Functional criteria

- **H1A-F01:** Add the H0B-approved client/control module exporting `SessionControlV1`, `RunQueryV1`, `ApprovalControlV1`, `EventSourceV1`, `ArtifactAccessV1`, `RuntimeInspectionV1`, and only the approved aggregate façade.
- **H1A-F02:** Define non-interchangeable versioned newtypes for session, run, turn, message, approval, tool-call, artifact, correlation, idempotency, and cursor identity. Serialization is stable and cursors are opaque outside their owning event stream.
- **H1A-F03:** Define request/response DTOs for start, continue, resume, branch, submit message, cancel, inspect/list runs and sessions, respond to approval, subscribe/resume events, and list/describe/read artifact ranges. Each DTO states required/optional fields and host- versus caller-assigned IDs.
- **H1A-F04:** Define one `ControlErrorV1` classification with stable code, human message, retryability, optional redacted details, and optional correlation ID; enumerate codes for validation, conflict, queue-full, lagged/expired cursor, not-found, unauthorized/policy, cancelled, unavailable, and internal failure.
- **H1A-F05:** Define policy projections containing tool-call ID, canonical tool ID, input hash, risk, execution mode, decision, reason, matched rule, and source.
- **H1A-F06:** Define approval projections and responses containing approval/tool-call correlation, summary, editable-input rules, original/edited hashes, expiry, cancellation state, and bounded session-grant terms.
- **H1A-F07:** Define artifact metadata with logical ID/URI, relative display path, size, media type, integrity metadata, and range/chunk response fields; no operation returns an unbounded byte vector.

### Behavioral criteria

- **H1A-B01:** Message submission returns a queue acknowledgement independently of completion; completion or failure is observed as a terminal event.
- **H1A-B02:** Repeating an idempotency key with the same normalized request returns the original acknowledgement/result; reusing it with a different request returns conflict and performs no second submission.
- **H1A-B03:** Concurrent sends follow the exact H0B queue/serialization rule, enforce a documented bound, and return a stable backpressure error rather than blocking without limit.
- **H1A-B04:** Cancellation identifies its target and has deterministic outcomes for pre-acceptance, queued, in-flight, and already-terminal races; cancellation never rewrites committed history.
- **H1A-B05:** Event resume preserves ordering within the documented scope; expired/lagged cursors return the stable cursor error and newest safe resumption information if approved by H0B.
- **H1A-B06:** Duplicate, late, expired, or cancelled approval responses cannot execute a tool; edited input is revalidated and re-evaluated by policy.
- **H1A-B07:** Artifact reads reject traversal, cross-session access, invalid ranges, and requests above the documented maximum without allocating the requested unbounded size.

## 9. Public API / Schema / CLI Impact

Adds the H0B-approved stable Rust client-control module and serializable v1 DTOs. Existing raw types remain internal/experimental and are not aliases for the DTOs. No CLI shape changes are owned here.

## 10. Failure, Security, and Compatibility Semantics

- Invalid, duplicate, late, expired, unauthorized, unavailable, lagged, conflict, cancelled, and resource-limit outcomes use stable codes.
- Edited approvals retain original/edited hashes; bounded session grants cannot silently broaden authority.
- Artifact reads enforce maximum range/chunk size and never expose arbitrary absolute host paths.
- Events and details are redacted before boundary projection; serialized errors exclude internal chains and secrets.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H1A-F*` and `H1A-B*` criterion to a serialization, compile-boundary, or behavioral contract test.
- Serialization round trips and additive-field behavior for every DTO family.
- Session start/continue/resume/branch lineage; caller/host ID collision tests.
- Same-key duplicate, same-key/different-payload conflict, concurrent sends, queue full/backpressure.
- Cancellation before acceptance, while queued, in-flight, and after terminal completion.
- Cursor resume, filtering, lag/retention expiry, terminal/reconnect behavior.
- Approval duplicate/late/expired/edit/grant-bound tests.
- Artifact list/describe/ranged read, oversize rejection, traversal/redaction tests.
- Compile-time boundary checks preventing raw internal types in public DTO fields.

## 12. Documentation Updates

Draft the embedding/control and policy/approval contracts for later publication under `docs/v0.1/`; update crate README only after tests pass. Link all semantics to the accepted H0B decision.

## 13. Execution Steps

1. Convert accepted H0B semantics into failing DTO serialization and behavior-contract tests.
2. Add IDs, errors, policy/approval/event/artifact DTOs.
3. Add capability traits and façade without local implementation.
4. Add compatibility and boundary tests.
5. Review every method for concurrency, cancellation, panic, and security documentation.

## 14. Exit Criteria

- [x] All six stable capabilities and their v1 DTOs compile without internal model leakage.
- [x] Session, queue, idempotency, cancellation, event, approval, and artifact semantics are test-covered.
- [x] Stable errors and redaction behavior are documented.
- [x] No second steering path or unbounded artifact API is introduced.
- [x] H1B can implement the traits without making a semantic decision.

## 15. Rollback / Partial Completion Handling

Keep the new module experimental/private until the complete capability family and conformance contract are ready. Do not expose a partially stable façade or retain compatibility aliases for rejected DTO drafts.
