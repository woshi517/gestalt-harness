---
title: H0A Documentation Authority and Contract Map
status: completed
type: plan
target: v0.1
owners:
  - docs-group
---

# Plan: H0A Documentation Authority and Contract Map

## 1. Purpose

Make repository documentation authority, lifecycle, ownership, and v0.1 contract publication rules unambiguous before implementation begins.

## 2. Requirement IDs Covered

DOC-001, DOC-002, DOC-003, DOC-004, DOC-005, DOC-006, DOC-007, DOC-008.

## 3. Baseline Evidence at Planning

- `docs/README.md` contains an initial authority hierarchy, domain map, disposition list, and migration workflow.
- `docs/v0.1/README.md` does not exist.
- `docs/gestalt-harness-prd.md` and `docs/gestalt-harness-architecture.md` contain source-level types and legacy terms.
- `docs/gestalt-harnes-implementation-roadmap.md` is a cross-domain historical plan with a filename typo.
- Crate READMEs vary in boundary accuracy; `crates/gestalt-runtime/README.md` still presents `RuntimeRegistry` and `GestaltExtension` as active public paths.
- The release spec is stored as `docs/feature-spec/v0.1-hardening.md`, not the longer path used in the planning request.

## 4. ADR / Spec Constraints

- Follow the authority order in specification Section 2.
- Extract embedded architecture decisions into ADRs before removing their source sections.
- `docs/v0.1/` may link only implemented, test-backed contracts.
- Migration must follow D0–D5 and preserve redirects where required.

## 5. In Scope

- Inventory and classify active/superseded/historical/proposed documents.
- Harden `docs/README.md`; create the `docs/v0.1/README.md` contract map.
- Scope PRD and architecture-monolith reduction without bulk deletion.
- Update all crate READMEs with ownership, non-ownership, supported entry points, maturity, construction, failure/cancellation, and feature gates.
- Reconcile the spec filename/link mismatch.

## 6. Out of Scope

- Deciding Rust APIs, event shapes, generation leases, or CLI envelopes.
- Rewriting every historical document or publishing unimplemented v0.1 contracts.
- Moving the PRD/architecture wholesale or deleting embedded ADR text before extraction.

## 7. Dependencies and Blockers

Depends on 000. H0B must supply final API/SPI and decision classifications before crate README stable-entry-point sections can be finalized. Documents affected by unresolved decisions remain `proposed`, with links to the blocking H0B item.

## 8. Proposed Changes

### Functional criteria

- **H0A-F01:** Produce a documentation inventory in which every maintained Markdown file has path, type, lifecycle status, target, owner, domain, authority rank, canonical replacement if any, and required action.
- **H0A-F02:** `docs/README.md` defines the allowed type/status combinations, required metadata per type, authority resolution, one canonical active document per domain, owner responsibilities, and archive/redirect rules.
- **H0A-F03:** `docs/v0.1/README.md` contains sections for embedding/control, app services, context, trace/run, config, policy/approval, extensions, and CLI; each entry names its implementation and enforcing tests or is visibly marked unpublished.
- **H0A-F04:** `docs/gestalt-harness-prd.md` contains only vision, users/problems, product principles, scope boundaries, priorities, and success measures; source snapshots, Rust contracts, schemas, ADR text, and task backlogs are removed or replaced with canonical links.
- **H0A-F05:** `docs/gestalt-harness-architecture.md` contains current crate/system boundaries, canonical execution flow, trust boundaries, component relationships, and ADR/domain links. Embedded ADR-001–ADR-022 content is removed only after equivalent standalone ADR files exist.
- **H0A-F06:** Every crate README states ownership/non-ownership, supported stable entry points from H0B, experimental/internal areas, a compiling construction example, failure/cancellation behavior, and feature-gate effects.
- **H0A-F07:** Resolve the hardening-spec filename mismatch by choosing one canonical path through the documentation lifecycle rules and updating all repository links without maintaining two authoritative copies.

### Behavioral criteria

- **H0A-B01:** A document that conflicts with an accepted ADR is not labeled normative or implemented; it is corrected, marked superseded/proposed, or blocked on an ADR change.
- **H0A-B02:** A `docs/v0.1/` entry is publishable only when its implementation and named tests exist and pass; plans and proposed specs cannot satisfy this gate.
- **H0A-B03:** Moving an externally or internally referenced document leaves a redirect at the old path; unreferenced historical material may be archived without a redirect only after link validation.
- **H0A-B04:** Status values are type-valid: ADRs use proposed/accepted/superseded; plans use proposed/active/completed/abandoned; audits remain dated historical evidence.
- **H0A-B05:** Documentation reduction preserves accepted decisions and useful history; bulk deletion is rejected if extraction, replacement, or link evidence is missing.

## 9. Public API / Schema / CLI Impact

Documentation only. The plan must describe existing or H0B-accepted contracts and must not itself stabilize an export, schema field, or command.

## 10. Failure, Security, and Compatibility Semantics

- Conflicting docs defer to accepted ADRs and are marked superseded or proposed.
- Broken moved links require a redirect, not silent deletion.
- Security/trust statements must link to accepted ADRs and enforcing tests.
- Historical compatibility examples cannot appear as current supported behavior.

## 11. Tests and Fixtures

| Criteria | Evidence |
|---|---|
| H0A-F01, F02, B04 | `documentation-inventory.md`, `docs/README.md`, `scripts/check-hardening-docs.sh` |
| H0A-F03, B01, B02 | `docs/v0.1/README.md` publication states and named tests |
| H0A-F04, F05, B05 | scoped PRD and architecture overview; standalone ADR-001 through ADR-031 |
| H0A-F06 | root/crate README boundary, construction, failure/cancellation, and feature sections |
| H0A-F07, B03 | canonical `docs/feature-spec/v0.1-hardening.md`; inventory records redirects/history |

- Run `bash scripts/check-hardening-docs.sh`.
- Validate required metadata for active specs, ADRs, plans, audits, guides, and version contracts.
- Assert one canonical active authority per documented domain.
- `rg` active docs for stale consolidated crate names, legacy TOML support claims, V1 extension support, and removed API examples.
- Verify each `docs/v0.1/` link points to implemented behavior and named tests.

## 12. Documentation Updates

Primary outputs: `docs/README.md`, `docs/v0.1/README.md`, simplified PRD/architecture, crate READMEs, document metadata, redirects, and an inventory artifact under `docs/`.

## 13. Execution Steps

1. Inventory every Markdown document and assign type/status/owner/domain.
2. Resolve the hardening-spec canonical path and update inbound links.
3. Harden `docs/README.md` and create the gated v0.1 contract map.
4. Extract embedded ADRs before reducing PRD/architecture content.
5. Update crate READMEs using H0B classifications.
6. Archive or relabel superseded material incrementally and validate links.

## 14. Exit Criteria

- [x] DOC-001–DOC-008 each have concrete evidence.
- [x] Every active document has valid metadata and one authority classification.
- [x] `docs/v0.1/README.md` exists and contains no unimplemented contract claim.
- [x] PRD and architecture have durable, non-duplicative roles.
- [x] Every crate README states supported and unsupported boundaries.
- [x] Link, metadata, stale-name, and legacy-claim checks pass.

## 15. Rollback / Partial Completion Handling

Land inventory and metadata before moves. If extraction or link validation fails, retain original content, mark the reduction step incomplete, and do not redirect or archive the source document.
