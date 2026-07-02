---
title: H0C Greenfield Removal Ledger and Absence Checks
status: completed
type: plan
target: v0.1
owners:
  - release-management
---

# Plan: H0C Greenfield Removal Ledger and Absence Checks

## 1. Purpose

Create the authoritative pre-hardening removal ledger, assign every compatibility path to a phase, and define proof that removed behavior is absent.

## 2. Requirement IDs Covered

CLEAN-001, CLEAN-002, CLEAN-003, CLEAN-004, CLEAN-005, CLEAN-006, CLEAN-007.

## 3. Baseline Evidence at Planning

- Legacy `.gestalt/config.toml` and `policies.toml` are created in app, CLI, runtime, and TUI tests.
- `tests/fixtures/config/v1/full_valid.json` and workspace fixtures contain `workspace_file`, `memory_file`, `yolo_allow`, and related aliases.
- `crates/gestalt-runtime/src/manifest.rs` warns and falls back to protocol 1.0 when versions are missing.
- `crates/gestalt-runtime/src/lib.rs` uses crate-wide `allow(deprecated)`; `runtime_run_tests.rs` does likewise.
- Active READMEs and the roadmap still describe `RuntimeRegistry`, `GestaltExtension`, legacy TOML, or migration behavior.
- `docs/migrations/extension-manifest-v1-to-v2.md` is still in the active migrations area.

## 4. ADR / Spec Constraints

- ADR-031 authorizes removal and rejects migration of pre-hardening formats.
- `gestalt.extension.toml` remains supported only as manifest V2; it must not be flagged as legacy harness configuration.
- Unsupported persistence uses the incompatible-version/error path, not migration.
- H0B owns stable-export decisions; this ledger owns removal tracking, not replacement architecture.

## 5. In Scope

- Create `docs/plans/v0.1-hardening/pre-hardening-removal-ledger.md` as the required tracking ledger. Despite its location, this is not another implementation plan: it contains no independent scope or architecture and only tracks removal work owned by H0A–H5.
- Inventory production code, exports, dependencies, schemas, fixtures, diagnostics, docs, ADR clauses, deprecated APIs, and persistence readers.
- Assign phase owner and executable absence check to every row.

## 6. Out of Scope

- Performing removals.
- Deleting history without classification/redirects.
- Defining replacement runtime, event, config, CLI, or extension contracts.

## 7. Dependencies and Blockers

Depends on 000. Coordinate stable/deprecated API classification with H0B and document disposition with H0A. Unclassified behavior blocks G0 and all owning domain plans.

## 8. Proposed Changes

### Functional criteria

- **H0C-F01:** Create `pre-hardening-removal-ledger.md` with a short statement that the file is an execution tracker subordinate to this plan, ADR-031, and the owning phase plans.
- **H0C-F02:** Add one row for every independently removable compatibility behavior found in production code, public exports, Cargo dependencies/features, schemas, fixtures, CLI/TUI/app diagnostics, tests, deprecation allowances, active docs, migrations, examples, and superseded ADR clauses.
- **H0C-F03:** Each row contains the following fields:

| Field | Meaning |
|---|---|
| ID/category/path/symbol | uniquely locates the compatibility behavior |
| current behavior/evidence | what accepts, converts, exports, or advertises it |
| required disposition | delete, reject, archive, invert test, or narrow export |
| owner plan | exactly one of H0A–H5, with reason |
| dependency | H0B/ADR decision if any |
| absence check | exact search, test, schema assertion, or compile check |
| status/evidence link | open, blocked, removed, verified |

- **H0C-F04:** Seed and exhaustively search legacy harness TOML; config aliases, legacy secret/trust forms; manifest/protocol V1; deprecated Rust APIs/adapters; pre-hardening trace/run/context/checkpoint readers; compatibility-only dependencies; fixtures; active docs/migrations/examples; and `allow(deprecated)`.
- **H0C-F05:** Provide an executable command or named test for every row. Search-based checks include the exact path scope and allowed historical exceptions; test-based checks name the suite and expected rejection/error.
- **H0C-F06:** Add a summary grouped by owner and status so 000 and H5 can determine whether a phase or release gate is blocked without reinterpreting individual rows.

### Behavioral criteria

- **H0C-B01:** A row moves from `open` to `removed` only after implementation is gone, and to `verified` only after its rejection/absence test, documentation update, and dependency cleanup all pass.
- **H0C-B02:** Any discovered compatibility behavior without a row fails G0; any row without exactly one owner or absence check remains `open`.
- **H0C-B03:** Known legacy harness config is tracked as reject-with-`UNSUPPORTED_LEGACY_CONFIG`, not delete/migrate; `gestalt.extension.toml` is explicitly excluded from that legacy category.
- **H0C-B04:** Pre-hardening persisted formats and V1 extensions are tracked as rejection/removal work, never as compatibility-reader or converter work.
- **H0C-B05:** Historical mentions may remain only in documents classified historical/archive and excluded explicitly by path in active-doc absence checks.

## 9. Public API / Schema / CLI Impact

The ledger records planned removal impact. It must preserve stable error code `UNSUPPORTED_LEGACY_CONFIG` across app and CLI projections and distinguish removed APIs from H0B-approved replacements.

## 10. Failure, Security, and Compatibility Semantics

- Known legacy config paths fail before parse/merge/seed/mutate with path and supported target.
- V1/absent/malformed extension versions fail before activation.
- Removed persisted formats fail as incompatible; they are not upgraded.
- Security-sensitive legacy trust aliases must not normalize into current grants.

## 11. Tests and Fixtures

| Criteria | Evidence |
|---|---|
| H0C-F01, F03, F06 | ledger statement, fixed row schema, owner/status summary |
| H0C-F02, F04, B02-B05 | CLEAN-001 through CLEAN-007 rows and exact scoped evidence |
| H0C-F05, B01 | named cargo tests and `scripts/check-hardening-docs.sh` |

Repository-wide checks cover:

- a criterion-to-evidence matrix mapping every `H0C-F*` and `H0C-B*` criterion to a ledger query, baseline search, rejection test, or G0 check;
- no production `#[deprecated]` and no broad `allow(deprecated)`;
- no legacy config parsers/fallback/seeding/migration;
- removed aliases absent from Rust models and JSON Schema;
- V1 conversion/adapters/builders/activation absent and V1 rejected;
- pre-hardening persistence migration branches absent;
- active docs/fixtures contain no support claims;
- no unclassified ledger search hit remains.

## 12. Documentation Updates

Create and maintain the ledger; link it from `docs/README.md`, this program plan, ADR-031, and H5. Archive migration material only under H0A rules.

## 13. Execution Steps

1. Search production, tests, fixtures, schemas, Cargo files, and docs for seeded categories.
2. Create the tracking ledger with its subordinate-artifact statement, fixed row schema, and status definitions.
3. Add one row per independently removable behavior, including every search hit or documented exception.
4. Assign exactly one owner and any H0B/ADR dependency with H0A/H0B.
5. Add and run an executable baseline absence/rejection check for every row; expected current failures establish the removal backlog.
6. Review for unclassified search hits and freeze the initial ledger at G0.

## 14. Exit Criteria

- [x] The ledger exists and covers all CLEAN-001–CLEAN-007 categories.
- [x] The ledger identifies itself as a tracker, not an implementation plan, and contains no independent architectural decisions.
- [x] Every row has an owner, dependency, disposition, and absence check.
- [x] No repository search hit is unclassified.
- [x] Domain plans reference their ledger rows.
- [x] H5 can mechanically determine whether every row is verified.

## 15. Rollback / Partial Completion Handling

Never mark a row complete from code deletion alone. If its rejection test, documentation update, dependency cleanup, or absence check is incomplete, keep the row open and the owning phase blocked from release.
