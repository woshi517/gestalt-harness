---
title: Pre-Hardening Removal Ledger
status: active
type: tracker
target: v0.1
owners:
  - release-management
---

# Pre-Hardening Removal Ledger

This execution tracker is subordinate to the
[H0C plan](./H0C-greenfield-removal-ledger-and-absence-checks.md),
[ADR-031](../../adrs/ADR-031-v0-1-greenfield-compatibility-cutoff.md), and the
owning phase plans. It records removal evidence; it does not define replacement
architecture.

## Summary

| Owner | Total | Open | Removed | Verified |
|---|---:|---:|---:|---:|
| H0A | 1 | 0 | 0 | 1 |
| H0C | 1 | 0 | 0 | 1 |
| H2A | 1 | 0 | 0 | 1 |
| H3A | 2 | 0 | 0 | 2 |
| H4A | 3 | 0 | 0 | 3 |
| **Total** | **8** | **0** | **0** | **8** |

## Rows

| ID / behavior | Current evidence | Required disposition | Owner and reason | Dependency | Exact absence/rejection check | Status / evidence |
|---|---|---|---|---|---|---|
| CLEAN-001 / legacy harness TOML | app config discovery detects legacy workspace/global paths without parsing | reject as `UNSUPPORTED_LEGACY_CONFIG`; never parse, merge, seed, rename, delete, or migrate | H3A: config authority | ADR-031 | `cargo test -p gestalt-app --test config_tests legacy_config_is_rejected_without_parsing` | verified: named test passes |
| CLEAN-002 / removed config aliases | strict schema rejects `workspace_file`, `memory_file`, legacy bash aliases, and unrecognized trust/credential fields | remove from input model/schema; reject as unknown | H3A: schema ownership | ADR-031 | `cargo test -p gestalt-app --test config_schema_tests removed_alias_is_rejected_as_unknown` | verified: named test passes |
| CLEAN-003 / manifest and protocol V1 | discovery requires manifest V2; lifecycle negotiation selects V2 only | reject absent/malformed/V1 before activation; no converter or V1 adapter | H4A: extension protocol | ADR-030, ADR-031 | `cargo test -p gestalt-runtime --test extension_manifest_v2_tests discovery_rejects_v1_explicit_manifests` and `cargo test -p gestalt-runtime --test lifecycle_protocol_v2_tests protocol_version_negotiation_prefers_v2_and_rejects_unknown_versions` | verified: named tests pass |
| CLEAN-003 / V1 compatibility symbols | runtime contains only V2 package/lifecycle types; generic V2 process transport remains | remove V1 converter, legacy component, broker adapter, and version inference symbols | H4A: extension API | ADR-031 | `! rg -n 'GestaltExtension|LegacyProcess|from_v1_manifest|protocol 1\\.0' crates/gestalt-runtime/src` | verified: zero hits |
| CLEAN-004 / deprecated Rust APIs and allowances | `middleware`, `RuntimeRegistry`, legacy provider constructors, and broad deprecation allowances removed | expose only current names; no deprecated shim or broad allowance | H4A: cleanup spans runtime extension/composition surface | H0B API inventory, ADR-031 | `! rg -n '#\\[deprecated|#!\\[allow\\(deprecated\\)\\]' crates --glob '*.rs'` | verified by `scripts/check-hardening-docs.sh` |
| CLEAN-005 / pre-hardening persistence readers | trace, run-manifest, context-artifact, and checkpoint readers reject unsupported versions and legacy hash layouts | fail as incompatible; never migrate development formats | H2A: persisted contracts | H0B event decision, ADR-031 | `cargo test -p gestalt-runtime --test trace_contract_tests` and `cargo test -p gestalt-runtime --test context_management_tests checkpoint_rejects_source_hash_used_as_artifact_hash` | verified: version and legacy-layout rejection tests pass |
| CLEAN-006 / active claims and fixtures | current contracts describe rejection; legacy workspace fixture is retained only to test rejection | remove support/migration claims; keep only rejection fixtures and clearly historical records | H0A: documentation lifecycle | ADR-031 | `bash scripts/check-hardening-docs.sh` and `cargo test -p gestalt-app --test config_tests legacy_config_is_rejected_without_parsing` | verified: inventory and rejection checks pass |
| CLEAN-007 / ledger completeness | this ledger covers CLEAN-001 through CLEAN-007 and every row has one owner/check | fail H0/H5 when a category or documentation entry is absent | H0C: removal governance | hardening specification | `bash scripts/check-hardening-docs.sh` | verified: mechanical completeness check |

## Status Rules

- `open`: disposition, ownership, cleanup, or evidence is incomplete.
- `removed`: implementation is absent but all rejection/documentation/dependency
  evidence has not passed.
- `verified`: implementation, rejection/absence check, documentation, and
  dependency cleanup pass.
- A newly discovered compatibility behavior reopens the owning category and
  fails the H5 release gate until this ledger and its check are updated.

Historical mentions may remain in ADRs, audits, solutions, migrations, and
plans when their inventory status is historical/superseded or the text clearly
records rejection. They are not current support contracts.
