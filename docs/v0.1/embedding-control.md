---
title: Gestalt Embedding and Runtime Control v1
status: active
type: version-contract
target: v0.1
owners:
  - gestalt-runtime
---

# Gestalt v0.1: Stable Embedding and Runtime Control Contract

This contract specifies the v0.1 embedding interface and runtime control semantics for the Gestalt harness. H1B's local and mock host implementations pass the same conformance suite.

These specifications are constrained by the accepted [H0B Architectural Decisions](../plans/v0.1-hardening/h0b-architectural-decisions.md).

---

## 1. Stable Capability Interfaces & Façade

All embedding client integrations interact with the harness through the unified `RuntimeControlV1` façade, which is an aggregate of six narrow capabilities:

1. **`SessionControlV1`**: Start, continue, resume, branch, queue, cancel, and inspect active sessions.
2. **`RunQueryV1`**: List and inspect physical runs.
3. **`ApprovalControlV1`**: List and respond to approvals and inspect policy projections.
4. **`EventSourceV1`**: Poll retained events or resume from a stream-scoped cursor.
5. **`ArtifactAccessV1`**: List, create, describe, and read bounded ranges of session-generated artifacts.
6. **`RuntimeInspectionV1`**: Query runtime status, active snapshot generation, and extension health.

All operations consume and produce stable, versioned DTOs to avoid leaking raw internal models.

---

## 2. Strong Versioned Identities

To prevent type-confusion across logical and physical abstractions, the control API uses distinct newtypes for all entities:

* **`SessionIdV1`**: Identifies a logical, conversational stream.
* **`RunIdV1`**: Identifies a physical execution path of the runtime.
* **`TurnIdV1`**: Identifies a single turn (user request + assistant response).
* **`MessageIdV1`**: Unique identifier for enqueued steering messages.
* **`ApprovalIdV1`**: Unique identifier for pending policy approvals.
* **`ToolCallIdV1`**: Maps tool-call executions to policies/approvals.
* **`ArtifactIdV1`**: Uniquely references a stored artifact within the session.
* **`CorrelationIdV1`**: Identifies request-response or operation spans for trace alignment.
* **`IdempotencyKeyV1`**: Caller-assigned key for duplicate execution prevention.
* **`CursorV1`**: Opaque token used to navigate or resume event streams.

---

## 3. Stable Error Semantics (`ControlErrorV1`)

All fallible operations return the unified `ControlErrorV1` payload containing a classification code, friendly description, retry advisory, optional redacted details, and correlation tags:

| Error Code | Meaning | Retryable |
|---|---|---|
| `VALIDATION` | Invalid input parameters, wrong ID formatting, or malformed JSON payload. | No |
| `CONFLICT` | Lineage conflict, concurrent session execution, or idempotency key payload mismatch. | No |
| `QUEUE_FULL` | Backpressure: the steering or message queue has hit its maximum bounds. | Yes |
| `LAGGED_CURSOR` | Cursor has fallen out of the event stream retention window. | No (requires new cursor) |
| `EXPIRED_CURSOR` | Cursor is expired. | No |
| `NOT_FOUND` | Target session, run, artifact, or approval does not exist. | No |
| `UNAUTHORIZED_POLICY` | Action blocked by active policy rules. | No |
| `CANCELLED` | Operation was explicitly terminated via user cancellation. | No |
| `UNAVAILABLE` | Crate or downstream service is temporarily unavailable. | Yes |
| `INTERNAL_FAILURE` | An unhandled error occurred inside the harness. Details are redacted. | No |

---

## 4. Behavioral Rules

### 4.1 Queue & Idempotency (H1A-B01, H1A-B02)
* Submitting a message to the steering queue returns a queue acknowledgement (`acknowledged: true`) independently of execution completion.
* Repeating a request with the same `IdempotencyKeyV1` returns the exact cached result. Reusing it with a different payload returns `CONFLICT` and executes no new operations.

### 4.2 Concurrency & Backpressure (H1A-B03)
* Each logical session restricts execution to a single-active-writer to prevent state race conditions.
* Concurrent message submissions enforce a strict queue bound. When exceeded, the system returns a `QUEUE_FULL` error rather than blocking the caller indefinitely.

### 4.3 Cancellation Races (H1A-B04)
* Cancelling a run targets the specific `RunIdV1`.
* Cancellation halts in-flight tool subprocesses and network calls immediately.
* It resolves races deterministically: if a run is already completed/terminal, cancellation returns `cancelled: false` and never rewrites or trims committed event history.

### 4.4 Event Resume (H1A-B05)
* Event polling uses opaque stream cursors to preserve order.
* If a cursor falls behind retention limits, the API returns a `LAGGED_CURSOR` error containing the newest safe resumption cursor within the error's `details`.

### 4.5 Bounded Artifact Reads (H1A-B07)
* Artifact range reads reject directory traversal sequences (e.g. `../`) and block cross-session resource reads.
* The API enforces a 1 MiB maximum chunk size by default. Requests above this limit or specifying invalid bounds are rejected immediately with a `VALIDATION` error, preventing unbounded memory allocations.
