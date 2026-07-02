# Plan: H4B Extension Activation, Trust, and Generation

## 1. Purpose

Make V2 extension construction, activation, reload, trust, generation adoption, and tool authority structured, fail-safe, and observable.

**Implementation status:** in progress. Hash-pinned trust and typed constructor failures are in place, but activation reporting, generation metadata, and tool-origin coverage still need follow-through.

## 2. Requirement IDs Covered

EXT-001, EXT-002, EXT-003, EXT-004, EXT-005; POL-003; Section 21 extension matrix.

## 3. Current-State Evidence

- Activation types exist in `crates/gestalt-runtime/src/activation.rs`; `RuntimeExtensionSnapshot` carries generation, health, diagnostics, resources, and negotiated protocol.
- ADR-029 and `RuntimeSnapshotLease` currently pin a generation for a run.
- `RuntimeExtensionSnapshot::context_plan_from_registry` defaults context contributors to `FailOpen`, while policy guards use `FailClosed`.
- `RuntimeControl::reload_extensions` preserves the active generation on failure and returns a structured report.
- Config/runtime paths now derive hash-pinned trust decisions from discovered manifests, and the trust/reload paths are covered by conformance tests.
- Tool origins span built-ins, command tools, package components, and MCP registries.

## 4. ADR / Spec Constraints

- H0B has accepted generation pinning/adoption and startup required/optional failure decisions, including ADR-029's turn-level lease boundary.
- H4A must remove V1/deprecated paths first.
- Effective authority is package request ∩ grant ∩ host policy ∩ managed policy ∩ runtime policy.
- Required security/policy components fail closed; failed reload preserves the active generation.
- Client descriptors remain inventory only and execute no client code.

## 5. In Scope

- Final component stability matrix integration.
- Structured host construction and activation reports.
- Required-security, required-general, and optional component failure behavior.
- Integrity-aware trust diagnostics and removal of ID-only trust behavior assigned here.
- Atomic reload failure preservation and accepted generation lease behavior.
- Canonical validation/policy/approval/execution/output/trace path across all tool origins.

## 6. Out of Scope

- Changing the accepted pinning unit or startup policy locally.
- Manifest/protocol V1 cleanup (H4A), client code execution, registries/marketplaces, remote launchers, broad sandbox expansion, or product reload UX.

## 7. Dependencies and Blockers

Depends on accepted H0B generation and activation decisions, H4A completion, H1A inspection/error DTOs, H2A generation metadata, H2B contribution identity, and H3A trust config cleanup. The dependency gate is now satisfied; remaining work is implementation and conformance closure.

## 8. Proposed Changes

### Functional criteria

- **H4B-F01:** Implement the H0B-approved host construction result so success returns the host plus activation report and failure returns a structured report containing stage, component/package/instance identity, criticality, stable code, redacted cause, and diagnostics.
- **H4B-F02:** Represent resolved criticality for required-security/policy, required-general, and optional components and apply the accepted failure action at discovery, integrity validation, launch, initialize, describe, and capability-plan stages.
- **H4B-F03:** Compute effective authority as package request ∩ user/workspace grant ∩ host policy ∩ managed policy when present ∩ runtime execution policy; expose requested, granted, denied, and source information in redacted trust diagnostics.
- **H4B-F04:** Bind trust to package/component identity and verified integrity hash; remove ID-only trust entries/normalization assigned by H3A/H0C.
- **H4B-F05:** Construct and validate reload candidates without mutating the active snapshot, then atomically publish only a fully accepted candidate; track candidate, active, retired, and drained generations/resources.
- **H4B-F06:** Acquire/release `RuntimeSnapshotLease` at the accepted ADR boundary and record generation, fingerprint, lease/adoption boundary, and reload visibility in trace and inspection.
- **H4B-F07:** Execute built-in, command, extension, and MCP tools through the same ordered stages: identity/schema validation, policy, approval when required, execution, bounded output/materialization, trace, and canonical result recording.

### Behavioral criteria

- **H4B-B01:** Required security/policy failure is fail-closed; required-general failure prevents host construction; optional failure degrades only with a structured diagnostic and cannot grant partial authority.
- **H4B-B02:** Expected activation failures return reports and never panic. Reports and traces contain no secret values or raw child-process/internal error chains.
- **H4B-B03:** Hash mismatch or changed content under the same ID removes trust and blocks/degrades according to criticality; it never falls back to ID-only trust.
- **H4B-B04:** Failed or dry-run reload leaves active generation, fingerprint, capabilities, and callability unchanged and cleans up candidate-only resources.
- **H4B-B05:** Successful reload becomes visible only at the accepted adoption boundary; existing leases retain one generation consistently and retired resources drain only after the final lease referencing that generation is released.
- **H4B-B06:** Tool origin cannot skip or reorder validation, policy, approval, bounded output, trace, or canonical history recording.
- **H4B-B07:** Client descriptors cannot execute code, tools, or provider calls; mutate history/snapshots; or bypass policy.

## 9. Public API / Schema / CLI Impact

Adds H0B-approved construction/activation/reload/inspection reports and trust/generation fields. Stable client DTOs receive logical metadata only; internal manager/snapshot types remain internal or experimental.

## 10. Failure, Security, and Compatibility Semantics

- Required security/policy failure denies activation/execution; required general failure prevents host construction.
- Optional failure may degrade only with structured health/diagnostic records.
- Hash mismatch revokes trust and cannot fall back to ID-only trust.
- Failed/dry-run reload never changes active generation; retired resources drain only under accepted lease rules.
- Tool origin never bypasses validation, policy, approval, bounded output, redaction, or trace.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H4B-F*` and `H4B-B*` criterion to an activation, trust, reload, generation, tool-origin, redaction, or descriptor test.
- Matrix for required security, required general, and optional discovery/launch/initialize/describe/invoke failures.
- Construction returns structured errors and never panics on expected failures.
- Trust-by-hash success, hash mismatch, changed package same ID, reduced grants, and secret-redacted diagnostics.
- Failed/dry-run reload preserves generation/fingerprint/callability; successful reload observes accepted adoption boundary and resource drain.
- Multi-turn run/reload trace metadata matches accepted pinning.
- Parameterized tool-origin conformance: valid, invalid input, allow, deny, approval, cancellation, spillover, trace.
- Client descriptor non-execution and prohibited-access tests.

## 12. Documentation Updates

Publish activation/trust/generation contracts and final stability matrix under `docs/v0.1/`; update ADR-029 links, extension/permissions guides, runtime inspection docs, crate READMEs, and fixture documentation.

## 13. Execution Steps

1. Encode accepted criticality and generation rules as failing tests.
2. Add structured construction/activation/reload reports.
3. Enforce integrity-aware effective authority and diagnostics.
4. Implement atomic candidate/publish/retire behavior at the accepted lease boundary.
5. Run the tool-origin conformance matrix and update version contracts.

## 14. Exit Criteria

- [ ] Expected activation failures never panic library consumers.
- [ ] Required security/general and optional failure semantics match the accepted decision.
- [ ] Trust is integrity-aware and diagnostics expose safe effective authority.
- [ ] Failed reload preserves the active generation; adoption matches the accepted ADR.
- [ ] Every tool origin passes the same policy/approval/trace conformance suite.

## 15. Rollback / Partial Completion Handling

Keep the previous active generation and trust policy on any candidate failure. Do not publish a candidate or expose a stable construction API until the complete criticality and rollback matrix passes.
