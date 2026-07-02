---
title: H0B API/SPI Inventory and Architectural Decisions
status: completed
type: plan
target: v0.1
owners:
  - gestalt-core
  - gestalt-runtime
---

# Plan: H0B API/SPI Inventory and ADR Decisions

## 1. Purpose

Enumerate the intended stable v0.1 surface and resolve the architectural decisions that later implementation plans are forbidden to make.

Status: completed. The authoritative outputs are [`api-spi-inventory.md`](./api-spi-inventory.md) and [`h0b-architectural-decisions.md`](./h0b-architectural-decisions.md); later plans consume these decisions and must not reopen them without an accepted amendment.

## 2. Requirement IDs Covered

CORE-001, CORE-002, CORE-003, CORE-004; RUNTIME-001, RUNTIME-002, RUNTIME-003, RUNTIME-004, RUNTIME-005, RUNTIME-006; EVT-001, EVT-002, EVT-003, EVT-004, EVT-005; CLI-001, CLI-002, CLI-003, CLI-004; EXT-004; PROV-001, PROV-002; TOOL-001, TOOL-002, TOOL-003; Sections 18 and 19.

## 3. Baseline Evidence at Planning

- `crates/gestalt-core/src/lib.rs` re-exports raw orchestration, context, provider, policy, tool, and event types without stability tiers.
- `crates/gestalt-runtime/src/lib.rs` publicly exposes many implementation modules and begins with crate-wide `#![allow(deprecated)]`.
- `crates/gestalt-runtime/src/control.rs` splits `RuntimeControl` and `HostControl`, returns raw runtime/core types, exposes a Tokio broadcast receiver, and reads artifacts into `Vec<u8>`.
- `crates/gestalt-runtime/src/trace/mod.rs` persists raw `AgentEvent` in `EventEnvelope`.
- `crates/gestalt-cli/src/output.rs` already defines `{schema_version, kind, data}` and a separate error payload.
- ADR-029 selects a run-level `RuntimeSnapshotLease`; the broader extension spec proposes turn-level adoption.
- `RuntimeHost` startup and activation paths require an explicit required/optional failure contract.

## 4. ADR / Spec Constraints

- Freeze execution invariants before raw model representations.
- The runtime-control Rust façade is not a wire protocol, but DTOs must remain serializable and transport-neutral.
- Events use one canonical producing path with distinct internal, persisted, and client forms.
- ADR-028–ADR-031 remain authoritative unless explicitly amended/superseded.
- Greenfield compatibility removes deprecated alternatives; it does not require adapters.

## 5. In Scope

- Inventory all public exports in `gestalt-core`, `gestalt-runtime`, and relevant `gestalt-app` reports.
- Classify provider/tool/policy/approval/context/trace traits as stable external SPI, experimental extension point, or internal.
- Decide raw-model/client-DTO boundary and minimum stable tool/provider authoring path.
- Decide runtime-control operation semantics, event representations, CLI envelope, generation lease unit, and extension startup failure behavior.
- Draft and accept required ADR amendments/supersessions and stability-contract tables.

## 6. Out of Scope

- Implementing DTOs, narrowing exports, rewriting traces, or changing activation.
- Stabilizing entire crates or remote transport/task bundles.
- Treating current `pub` visibility as a compatibility promise.

## 7. Dependencies and Blockers

Depends on 000. Coordinate with H0A authority classifications and H0C removal inventory. H1A, H2A, H3B, and H4B remain blocked until their respective decisions below are accepted.

## 8. Proposed Changes

### Functional criteria

- **H0B-F01:** Explicitly inventory every stable-candidate module, re-export, trait, constructor, and DTO/report in `gestalt-core`, `gestalt-runtime`, and `gestalt-app`; classify all other technically public items experimental/internal by default. Stable rows record path, audience, class, gate, owner, failure/panic contract, disposition, and enforcing test.
- **H0B-F02:** Classify provider, tool, policy, approval, context, and trace traits independently as stable external SPI, experimental extension point, or internal interface; list the exact methods/types exposed for every stable SPI.
- **H0B-F03:** Accept a runtime-control decision specifying ID assignment, start/continue/resume/branch lineage, queue acknowledgement/completion, idempotency, concurrency, bounds/backpressure, cancellation races, cursor retention/lag, and bounded artifact reads.
- **H0B-F04:** Accept an event decision defining the metadata-bearing internal record, lossless trace representation, typed client projection, per-kind schema ownership, version locations, and unknown-field/unknown-kind reader behavior.
- **H0B-F05:** Accept a CLI decision selecting the stable command subset and exactly one success/error envelope, including its compatibility disposition.
- **H0B-F06:** Accept a generation decision specifying pinning unit, acquisition/release boundary, multi-turn `run_prompt`, reload visibility, and trace/inspection metadata. Amend or supersede ADR-029 if the accepted behavior differs.
- **H0B-F07:** Accept an activation decision specifying construction result shape and discovery/validation/launch/initialize failure behavior for required-security, required-general, and optional components.
- **H0B-F08:** For every decision, record chosen option, rejected alternatives, consequences, affected plans/files, compatibility impact, and conformance tests; update the ADR index and downstream blocker links.

### Behavioral criteria

- **H0B-B01:** No inventory row is classified stable solely because its Rust visibility is `pub`.
- **H0B-B02:** Stable DTOs contain no raw `Session`, `AgentEvent`, `RuntimeEvent`, `RuntimeConfig`, provider-native, registry, broadcast receiver, artifact-store, absolute-path, or serialized internal-error-chain field.
- **H0B-B03:** Every stable method explicitly documents each validation, policy, approval, provider, tool, trace, context, cancellation, concurrency, retry, and panic class it can encounter, and marks the remaining classes non-applicable.
- **H0B-B04:** Expected input and activation failures are represented as results/reports and never as panics; required security behavior cannot select fail-open.
- **H0B-B05:** A decision remains unresolved until its ADR/contract is accepted. Draft prose or an inventory recommendation does not unblock dependent plans.

## 9. Public API / Schema / CLI Impact

This plan records the proposed stable surface and compatibility class; later plans perform changes. Inventory must explicitly separate Rust embedding API, authoring SPI, client DTOs, persisted schemas, CLI contracts, and internals.

## 10. Failure, Security, and Compatibility Semantics

- Stable methods document validation, policy, approval, provider/tool/trace/context errors, cancellation, concurrency, and panic behavior.
- Expected input or extension activation failure must not panic.
- Security/policy components cannot default to fail-open.
- DTOs and persisted records exclude secrets, provider-native types, absolute public paths, and serialized Rust error chains.

## 11. Tests and Fixtures

| Criteria | Evidence |
|---|---|
| H0B-F01, F02, B01, B02 | `api-spi-inventory.md` allowlist, forbidden-boundary list, exact SPI methods |
| H0B-F03, B03 | runtime-control decision and `control_conformance` |
| H0B-F04 | event decision; H2A owns publication fixtures |
| H0B-F05 | accepted command subset/envelope; `main_cli_contract_tests`; H3B owns snapshots |
| H0B-F06 | generation decision, ADR-029, H4B lease/reload tests |
| H0B-F07, B04 | activation decision and H4B failure-mode tests |
| H0B-F08, B05 | accepted decision record's rejected alternatives, consequences, owners, compatibility, and evidence |

- Add planned compile-time public-surface assertions or API snapshots for stable exports.
- Name conformance tests for every stable SPI and control capability.
- Define schema/golden fixtures for chosen event and CLI envelopes.
- Define negative checks for raw `Session`, `AgentEvent`, `RuntimeEvent`, provider-native, registry, and unbounded artifact types crossing stable DTO boundaries.
- Inventory all current `#[deprecated]`/`allow(deprecated)` occurrences for H0C.

## 12. Documentation Updates

Publish accepted decisions under `docs/adrs/`, update `docs/adrs/README.md`, feed stable classifications to crate READMEs, and link decisions from every dependent plan. Do not publish contract prose under `docs/v0.1/` yet.

## 13. Execution Steps

1. Generate and review the Rust export and trait inventory.
2. Trace current control, event, CLI, snapshot, and activation behavior to tests.
3. Write focused decision proposals with alternatives and consequences.
4. Accept/amend/supersede ADRs through repository review.
5. Mark each downstream plan `ready` or `blocked` with an exact decision reference.

## 14. Exit Criteria

- [x] Every candidate stable export and SPI has one stability class and owner.
- [x] All six decisions in Section 8 are accepted and linked.
- [x] No stable promise depends on accidental visibility.
- [x] Downstream plans can implement without selecting architecture.
- [x] API, event, CLI, generation, and activation test obligations are explicit.

## 15. Rollback / Partial Completion Handling

Unaccepted proposals remain proposed and cannot unblock implementation. Preserve current behavior until a decision is accepted; mark each affected plan blocked rather than selecting a local workaround.
