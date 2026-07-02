# Plan: v0.1 Hardening Program Coordination and Phase Gates

## 1. Purpose

Coordinate the contract-maturity program defined by `docs/feature-spec/v0.1-hardening.md`. This plan owns sequencing, gates, traceability, and shared conventions; it does not implement a domain contract.

## 2. Requirement IDs Covered

All v0.1 requirements by coordination: DOC-001–DOC-008, CLEAN-001–CLEAN-007, CORE-001–CORE-004, RUNTIME-001–RUNTIME-006, APP-001–APP-002, CTX-001–CTX-004, CFG-001–CFG-005, EXT-001–EXT-005, EVT-001–EVT-005, POL-001–POL-004, CLI-001–CLI-004, PROV-001–PROV-002, TOOL-001–TOOL-003, Sections 18, 21, and 22.

## 3. Current-State Evidence

- The authoritative release-scope specification is currently named `docs/feature-spec/v0.1-hardening.md`; the requested longer filename is absent.
- `docs/README.md` already defines an authority order, but `docs/v0.1/` and the phase plans do not exist yet.
- Accepted ADR-028 through ADR-031 constrain extension and greenfield behavior; ADR-029 still conflicts with the turn-level proposal noted by EXT-004.
- Evidence spans `crates/*/src`, crate READMEs, `docs/schemas/gestalt.schema.json`, and shared fixtures under `tests/fixtures/`.

## 4. ADR / Spec Constraints

- Accepted ADRs outrank version contracts, the hardening spec, domain specs, and plans.
- Plans describe execution only. Any new architectural choice must be routed to H0B and recorded by an accepted ADR amendment/supersession before dependent work.
- ADR-031 makes pre-hardening behavior removable rather than migratable.
- A proposed contract must not be published under `docs/v0.1/` until implementation and tests exist.

## 5. In Scope

- Maintain the dependency graph, requirement-to-plan map, phase gates, shared fixture rules, and shared documentation rules.
- Record each blocked plan and its exact unresolved decision.
- Coordinate cross-plan fixtures after the domain plans land.

## 6. Out of Scope

- Production code, schemas, fixtures, ADR decisions, and released contract prose.
- Product-specific adapters, remote execution, schema v2, marketplaces, and other Section 23 deferred work.

## 7. Dependencies and Blockers

No predecessor. Master graph:

```text
Step 1: 000
Step 2: H0A || H0B || H0C
Step 3: H1A || H2A || H2B || H3A || H3B || H4A || H4B
Step 4: H1B + cross-plan fixture integration
Step 5: H5
```

Step 3 plans may start only after their named H0B decisions and H0C ownership entries are accepted. H5 is blocked until every earlier exit criterion and integration check passes.

## 8. Proposed Changes

### Functional criteria

| ID | Required result |
|---|---|---|
| **PC-F01** | Maintain a requirement map in which every spec requirement has one primary owner and any supporting plans; duplicate primary ownership is an error. |
| **PC-F02** | Maintain a dependency record for every plan using `not_started`, `blocked`, `ready`, `in_progress`, or `complete`, with evidence links for `ready` and `complete`. |
| **PC-F03** | Record G0–G3 gate results with date, reviewer, commands/evidence, failures, and affected downstream plans. |
| **PC-F04** | Define one shared fixture registry containing fixture path, owning plan, schema/version, normalization rules, and consuming suites. |
| **PC-F05** | Define one documentation convention covering metadata, authority links, version-contract publication, redirects, and plan status updates. |

The dependency record must contain at least:

| Plan | Primary requirements | Readiness condition |
|---|---|---|
| H0A | DOC-001–DOC-008 | 000 accepted |
| H0B | CORE, RUNTIME, EVT, CLI, EXT-004, Section 19 | 000 accepted |
| H0C | CLEAN-001–CLEAN-007 | 000 accepted |
| H1A | RUNTIME-001–005, POL-001–002, Section 18 | named H0B control/API decisions accepted |
| H1B | RUNTIME-006, APP-001–002 | H1A plus shared domain DTOs complete |
| H2A | EVT-001–005 | H0B event decision accepted; H0C persistence rows assigned |
| H2B | CTX-001–004 | H0B API classification accepted; H2A identities fixed |
| H3A | CFG-001–005, CLEAN-001–002 | H0C config rows assigned |
| H3B | CLI-001–004 | H0B CLI decision accepted; H3A error model fixed |
| H4A | CLEAN-003–004, EXT-001 | H0C rows assigned; ADR-028/030/031 accepted |
| H4B | EXT-001–005, POL-003 | H0B generation/activation decisions accepted; H4A complete |
| H5 | Sections 21–22 | all prior exits and integration checks complete |

### Behavioral criteria

- **PC-B01:** A plan with an unresolved decision is marked `blocked`, names the decision and authority owner, and performs no work that assumes an outcome.
- **PC-B02:** G0 fails if any contract, removal, or decision lacks an owner; G1 fails if focused contract tests fail; G2 fails if local/mock or shared fixtures diverge; G3 fails if any release, absence, or documentation check fails.
- **PC-B03:** A gate cannot be waived by editing a later plan, weakening a test, or publishing proposed behavior as a v0.1 contract.
- **PC-B04:** Fixture normalization removes only declared nondeterminism; unexpected differences remain failures.
- **PC-B05:** When a predecessor reopens, all dependent `ready`, `in_progress`, or `complete` states revert to `blocked` until impact is reassessed.

## 9. Public API / Schema / CLI Impact

None directly. Any impact is owned by the mapped domain plan and must cite its accepted H0B decision.

## 10. Failure, Security, and Compatibility Semantics

- A blocked plan records `Blocked: <decision and owning ADR/H0B section>` and does no dependent implementation.
- A gate fails on an unclassified compatibility path, unresolved security behavior, fixture conflict, or undocumented public-contract change.
- Greenfield removal never silently parses, migrates, or accepts pre-hardening data.

## 11. Tests and Fixtures

Shared conventions:

- Maintain a criterion-to-evidence matrix mapping every `PC-F*` and `PC-B*` criterion to a gate check, command, fixture, or reviewed artifact.
- Put reusable golden inputs under `tests/fixtures/<domain>/v1/`; crate-local protocol fixtures may remain beside crate integration tests.
- Each fixture README states owner, schema/version, normalization fields, and expected reader behavior.
- Normalize timestamps, generated IDs, temp paths, and nondeterministic ordering explicitly.
- Prefer semantic assertions for additive fields; use snapshots for stable serialized surfaces.
- Every removal ledger entry names an `rg`, schema assertion, compile-fail check, or rejection fixture proving absence.
- Cross-plan matrix must cover tool allow/deny/approval, cancellation, context pressure, artifact spillover, extension rejection, continue/resume/branch, and unknown event fields/kinds.

## 12. Documentation Updates

- Every implementation PR updates its plan status, relevant `docs/v0.1/` contract only after tests pass, and affected crate README.
- Active docs use repository-relative links and metadata from H0A.
- Historical moves retain redirects when links would break.
- No plan may be cited as architectural authority.

## 13. Execution Steps

1. Review H0A/H0B/H0C together and populate all owners and decision links.
2. Mark Step 3 plans blocked or ready from the H0 outputs.
3. At G1, reconcile shared ID, error, cursor, event, and fixture vocabulary across H1A/H2A/H2B/H3A/H3B/H4.
4. Run H1B conformance and cross-plan fixture integration at Step 4.
5. Hand the completed gate record and removal ledger to H5.

## 14. Exit Criteria

- [x] Every requirement maps to at least one owning plan.
- [x] Every plan has one bounded domain and explicit non-touch scope.
- [x] All unresolved choices have an H0B/ADR owner and dependent plans are marked blocked.
- [x] Shared fixture and documentation conventions are used consistently.
- [x] G0–G3 have named, testable evidence and no plan silently creates architecture.

## 15. Rollback / Partial Completion Handling

Keep completed plans and accepted decisions intact. Revert only the failing domain slice, mark its gate incomplete, list downstream plans as blocked, and do not publish partial behavior as a v0.1 contract.
