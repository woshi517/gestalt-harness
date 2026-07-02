# Plan: H5 Release Conformance, Documentation Freeze, and Cutover

## 1. Purpose

Perform the final v0.1 contract freeze: verify all domain conformance, close the removal ledger, publish only test-backed contracts, and prepare release notes without adding scope.

## 2. Requirement IDs Covered

Section 21 full test matrix; Section 22 v0.1 release criteria; CLEAN-006, CLEAN-007 final verification; all requirements by release evidence.

## 3. Current-State Evidence

- `docs/release-checklist.md` predates the complete hardening criteria.
- Contract evidence is distributed across core/runtime/app/CLI/TUI tests and `tests/fixtures/`.
- `docs/v0.1/` is not yet present at planning time.
- Current production/runtime code and tests contain deprecated APIs/allowances, compatibility paths, and active legacy documentation that earlier plans must remove.
- `CHANGELOG.md` exists but has no final hardening release-note entry.

## 4. ADR / Spec Constraints

- No new architecture or feature expansion during freeze.
- Accepted ADRs and implemented tests govern; proposed plans/spec text cannot be promoted as released behavior.
- ADR-031 requires complete greenfield removal and absence proof.
- `docs/v0.1/` contains only implemented, test-backed contracts.

## 5. In Scope

- Execute the complete Section 21 matrix and all cross-plan fixtures.
- Close every H0C removal-ledger row with evidence.
- Freeze stable Rust/API/SPI, trace/context/config/CLI/extension contracts.
- Validate documentation authority, active-doc greenfield language, crate READMEs, links, metadata, schemas, examples, and feature matrices.
- Update final release checklist and release-notes outline.

## 6. Out of Scope

- New capabilities, contract redesign, migration support, Workbench/remote features, broad refactors, or accepting flaky/unverified behavior as stable.
- Fixing a failed gate by weakening tests or relabeling unimplemented behavior stable.

## 7. Dependencies and Blockers

Step 5. Depends on completed H0A/H0B/H0C, H1A/H1B, H2A/H2B, H3A/H3B, H4A/H4B, cross-plan fixture integration, and accepted ADRs. Any open ledger row, blocked plan, failing stable feature combination, or proposed decision blocks cutover.

## 8. Proposed Changes

### Functional criteria

- **H5-F01:** Produce a release evidence table with one row per Section 22 criterion containing requirement IDs, implementation paths/symbols, test/fixture commands, version-contract documentation, result, and reviewer/date.
- **H5-F02:** Run and record focused crate suites, workspace tests, doctests/examples, schema parity/generation, stable API-surface checks, CLI minimal/default/full-feature matrices, and the shared cross-plan fixture suite.
- **H5-F03:** Re-read every stable trace/run/context/config/CLI/extension fixture with the released reader/implementation and compare using only its declared normalizer.
- **H5-F04:** Execute every H0C row’s exact absence/rejection check and record its output/evidence link; require zero `open`, `blocked`, `removed-but-unverified`, or unclassified rows.
- **H5-F05:** Validate internal Markdown links, required metadata, unique canonical domain authority, accepted ADR references, redirects, crate README boundaries, and the implementation/test links for every `docs/v0.1/` contract.
- **H5-F06:** Freeze the accepted stable Rust exports/SPIs, persisted schemas, config schema, CLI command matrix/envelopes, and extension contracts using the prior-plan API/schema/snapshot checks.
- **H5-F07:** Update `docs/release-checklist.md`, `docs/README.md`, `docs/v0.1/README.md`, root/crate READMEs, `CHANGELOG.md` or the selected release-note file, feature-spec status, plan statuses, and H0C ledger status.

### Behavioral criteria

- **H5-B01:** Cutover fails if any Section 21 test, supported feature matrix, Section 22 row, documentation check, security/redaction check, or removal-ledger row fails.
- **H5-B02:** A failing old fixture is not repaired by restoring a deprecated shim, permissive reader, legacy alias, V1 adapter, or broad deprecation allowance.
- **H5-B03:** `docs/v0.1/` contains only behavior with an implementation path and passing enforcing test; proposed/deferred behavior remains outside released contracts.
- **H5-B04:** Any contract-affecting fix discovered during freeze returns to its owning plan, reopens dependent gates, and reruns affected integration evidence before cutover.
- **H5-B05:** Release notes distinguish stable, experimental, internal, removed, and deferred behavior and state that compatibility begins at the stable v0.1 cutover revision.
- **H5-B06:** The final revision contains no deprecated production API, broad `allow(deprecated)`, legacy harness TOML acceptance/migration, deprecated config alias, manifest/protocol V1 path, pre-hardening persistence reader, or active support claim for removed behavior.

## 9. Public API / Schema / CLI Impact

No new surface. This plan records and freezes the exact surfaces implemented by prior plans. Any discovered contract change returns to its owning plan and repeats the relevant gate.

## 10. Failure, Security, and Compatibility Semantics

- Release is all-or-blocked: critical conformance, security, redaction, rollback, compatibility, and absence failures prevent cutover.
- No deprecated shim or permissive reader may be restored to make old fixtures pass.
- Secret scans and fail-closed extension/policy cases are mandatory.
- Stable v0.1 compatibility begins only at the accepted cutover revision.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H5-F*` and `H5-B*` criterion to a recorded release command, fixture result, absence check, documentation validation, or freeze artifact.
- Run all Section 21 core, runtime-control, event/trace/context, config/CLI, extension, removal, and documentation checks.
- Run workspace tests, doctests/examples, schema generation/parity, API surface checks, minimal/default/full feature matrices, and CLI feature-disabled tests.
- Re-read every golden fixture with the stable reader and compare normalized output.
- Validate internal Markdown links, metadata, unique canonical domain ownership, ADR references, and redirects.
- Run repository-wide absence and secret scans; attach command output to the release evidence table.

## 12. Documentation Updates

- Finalize `docs/v0.1/README.md` and linked contracts.
- Update `docs/release-checklist.md`, `CHANGELOG.md` or release-note draft, root/crate READMEs, feature-spec status, plan completion statuses, H0C ledger, and archived redirects.
- Release notes outline: contract scope, stable surfaces, greenfield cutoff/removals, embedding/control, trace/context/config/CLI/extensions, known experimental/deferred areas, and upgrade expectations.

## 13. Execution Steps

1. Confirm every predecessor exit criterion and ADR status.
2. Run focused domain suites, then cross-plan fixtures and workspace/feature matrices.
3. Execute every ledger absence check and resolve unclassified hits in the owning plan.
4. Validate/freeze schemas, snapshots, API exports, examples, and v0.1 docs.
5. Complete the release evidence table, checklist, statuses, and release-note outline.

## 14. Exit Criteria

- [ ] Every Section 22 criterion links to passing evidence.
- [ ] The H0C ledger has no open, blocked, or unclassified row.
- [ ] All Section 21 tests and supported feature matrices pass.
- [ ] No deprecated production API, broad `allow(deprecated)`, legacy compatibility, or stale active claim remains.
- [ ] `docs/v0.1/` contains only implemented, tested contracts and all documentation validation passes.
- [ ] Release checklist and notes accurately distinguish stable, experimental, internal, removed, and deferred behavior.

## 15. Rollback / Partial Completion Handling

Do not cut a partial release. Reopen the owning plan for any failure, unfreeze only its affected contract documents, rerun downstream integration, and preserve the last known complete evidence set until all gates pass again.
