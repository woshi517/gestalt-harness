# Plan: H1B Runtime Host/App Boundary and Conformance

## 1. Purpose

Implement H1A capabilities at the session-owning local host boundary, provide a mock implementation, and route reusable app services through structured product-neutral reports.

## 2. Requirement IDs Covered

RUNTIME-006; APP-001, APP-002.

## 3. Current-State Evidence

- `AgentRuntime` implements `RuntimeControl` in `crates/gestalt-runtime/src/control.rs` even though session/run/artifact hosting spans higher layers.
- `RuntimeHost` is in `crates/gestalt-runtime/src/orchestration.rs`; app run/session services live in `crates/gestalt-app/src/run.rs`, `runs.rs`, and `sessions.rs`.
- CLI/TUI modules consume internal events and presentation paths directly.
- `crates/gestalt-app/examples/embed_app.rs` and `crates/gestalt-runtime/examples/embed_runtime.rs` are existing embedding examples to update or supersede.
- Reusable paths still contain direct `eprintln!` calls, including context persistence and extension/config diagnostics.

## 4. ADR / Spec Constraints

- H1A owns all control semantics; H1B only implements them.
- Full session/run/artifact capabilities belong to session-owning hosts; `AgentRuntime` may retain only narrow execution/inspection capabilities.
- `gestalt-app` remains product-neutral and emits diagnostics as data.
- No Workbench, Tauri, Ratatui, terminal, or remote-platform type enters a stable contract.

## 5. In Scope

- Local host implementation and deterministic mock/conformance implementation.
- Shared conformance suite for start/send/events/approval/cancel/artifacts/inspection.
- Product-neutral app-service requests/reports and diagnostic data/sink boundary.
- Removal or isolation of direct presentation writes in reusable paths.
- A minimal third-party Rust embedding example using only supported entry points.

## 6. Out of Scope

- Changing H1A DTO semantics, event persistence, CLI snapshots, or extension generation policy.
- Workbench adapters, UI rendering, remote host/client transport, and registry mutation APIs.

## 7. Dependencies and Blockers

Depends on completed H1A. H1B owns the minimum client event projection required
for host conformance; H2A/H2B may enrich persistence and context projections
without blocking this boundary.

## 8. Proposed Changes

### Functional criteria

- **H1B-F01:** Implement every H1A capability on the H0B-approved session-owning local host; delegate runtime inspection narrowly without making `AgentRuntime` own sessions, runs, approvals, or artifacts it does not host.
- **H1B-F02:** Implement an in-memory mock with controllable event retention, approval outcomes, cancellation points, artifact contents, and failures, using the same public traits and DTOs as the local host.
- **H1B-F03:** Create one conformance suite parameterized only by a host factory. It runs unchanged against local and mock implementations and covers every H1A behavioral criterion.
- **H1B-F04:** Define reusable app-service result/report types carrying typed data, ordered warnings/diagnostics, correlation IDs, and stable error projections. Reports contain no presentation formatting or UI-specific type.
- **H1B-F05:** Replace direct reusable-service stdout/stderr writes with returned diagnostics or a caller-provided diagnostic sink whose callback receives the same structured diagnostic type.
- **H1B-F06:** Remove or make internal the superseded broad control implementation according to H0B/H0C; do not introduce a deprecated compatibility shim.
- **H1B-F07:** Provide a compiling product-neutral embedding example that constructs a host, starts a session, submits a message, observes acknowledgement/terminal events, responds to approval, cancels a run, and performs a bounded artifact read using only supported imports.

### Behavioral criteria

- **H1B-B01:** Local and mock hosts produce the same stable result/error codes and event ordering for equivalent conformance inputs.
- **H1B-B02:** Reusable app services write zero bytes to stdout/stderr unless the caller explicitly supplies a presentation sink; collecting or dropping diagnostics does not alter execution.
- **H1B-B03:** Expected config, construction, activation, provider, policy, approval, trace, artifact, and cancellation failures return structured data and do not panic.
- **H1B-B04:** Feature-disabled capabilities remain present where the stable trait requires them and return the documented unavailable error; they do not disappear or leak feature-specific internals.
- **H1B-B05:** The embedding example imports no registry, raw event, raw session, artifact-store implementation, CLI/TUI, or product-specific module.

## 9. Public API / Schema / CLI Impact

Implements H1A stable Rust traits and app-service report types. CLI/TUI remain presentation clients and may adapt later; no Workbench-specific API is added.

## 10. Failure, Security, and Compatibility Semantics

- Local and mock hosts return equivalent stable codes and terminal event ordering.
- Expected construction, activation, config, provider, policy, approval, trace, and cancellation failures do not panic.
- Diagnostics are redacted data; presentation sinks cannot alter execution authority.
- Host-derived artifact and tool authority is not delegated to the caller.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H1B-F*` and `H1B-B*` criterion to the shared conformance suite, output-capture test, feature test, or compiling example.
- One conformance suite runs unchanged against local and mock controls.
- Reference flow: start, send, observe ack/completion, approve, cancel, list/describe/read artifact, inspect.
- Continue/resume/branch lineage and idempotency parity.
- Diagnostic capture tests proving reusable service stdout/stderr remains empty.
- Feature-matrix tests for optional capabilities and stable unavailable errors.
- Compile/check embedding example using only supported modules.
- Cross-plan golden scenario integration from 000.

## 12. Documentation Updates

Publish test-backed embedding/control and app-service contracts under `docs/v0.1/`; update `gestalt-runtime`/`gestalt-app` READMEs and the product-neutral embedding example.

## 13. Execution Steps

1. Build the reusable conformance harness from H1A acceptance cases.
2. Implement the mock and make the suite pass.
3. Implement the local session-owning host and make the same suite pass.
4. Convert reusable presentation writes to structured diagnostics.
5. Update and compile the embedding example; run cross-plan fixtures.

## 14. Exit Criteria

- [x] Local and mock implementations pass the identical conformance suite.
- [x] A host flow completes using only supported APIs.
- [x] Reusable services return structured diagnostics and do not write presentation output directly.
- [x] No product-specific adapter/type exists in stable harness code.
- [x] Version-contract docs and embedding example match tested behavior.

## 15. Rollback / Partial Completion Handling

Keep old callers on their existing internal path until the complete local conformance suite passes. If migration is partial, do not claim the host façade stable and record remaining callers in H0C/H5.
