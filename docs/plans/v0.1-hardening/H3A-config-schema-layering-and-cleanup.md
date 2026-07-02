# Plan: H3A Config Schema, Layering, and Cleanup

## 1. Purpose

Make strict `gestalt.json` schema version 1 the only harness configuration contract, with tested model/schema parity, provenance, and safe legacy-file rejection.

## 2. Requirement IDs Covered

CFG-001, CFG-002, CFG-003, CFG-004, CFG-005; CLEAN-001, CLEAN-002.

## 3. Current-State Evidence

- `docs/schemas/gestalt.schema.json` and fixtures under `tests/fixtures/config/v1/` exist, but current valid fixtures still contain deprecated context aliases.
- `crates/gestalt-app/src/config.rs` and config tests retain split TOML discovery/layering paths.
- CLI, runtime, and TUI tests create `.gestalt/config.toml` and `policies.toml`.
- Workspace fixtures contain `workspace_file`, `memory_file`, `yolo_allow`, and `always_deny`.
- `RuntimeConfig` carries hash-pinned `trusted_extension_pins`, which H0C must classify against integrity-aware trust.

## 4. ADR / Spec Constraints

- ADR-025 is superseded by ADR-031 for legacy TOML fallback, seeding, migration, aliases, and compatibility windows.
- Precedence is defaults < global JSON < workspace JSON < supported environment < CLI flags.
- Unknown fields are errors; schema version 1 remains canonical.
- `gestalt.extension.toml` is not legacy harness config.
- No no-op migration command and no Workbench namespace.

## 5. In Scope

- Strict schema/Rust model parity and section/field stability labels.
- Layering and per-value provenance with secret-safe reporting.
- Unsupported schema-version errors and future migration-registration documentation.
- Detect all known legacy harness TOML paths during discovery/load/mutation/diagnostics and return `UNSUPPORTED_LEGACY_CONFIG`.
- Remove deprecated aliases, legacy secret references, and H0C-classified trust/config aliases.

## 6. Out of Scope

- Schema version 2, migration command, automatic rewrite/deletion, product configuration, extension manifest TOML, CLI envelope formatting, or unrelated runtime configuration refactors.

## 7. Dependencies and Blockers

Depends on 000 and H0C config/trust ledger rows. Any trust alias whose replacement depends on H0B/H4B must remain blocked with that exact decision; do not infer a replacement here.

## 8. Proposed Changes

### Functional criteria

- **H3A-F01:** Establish one canonical serializable Rust model for `gestalt.json` schema version 1 and generate or mechanically compare `docs/schemas/gestalt.schema.json` from it; every modeled field appears once with matching type, requiredness, default, and unknown-field policy.
- **H3A-F02:** Remove all CLEAN-002 aliases from Rust fields, deserializers, normalization, schema, fixtures, and active examples, including legacy secret and trust aliases assigned to this plan.
- **H3A-F03:** Label every top-level section and any mixed-maturity field as stable, experimental, client-specific, or internal in the v0.1 config contract; explicitly label `tui` client-specific.
- **H3A-F04:** Produce an effective-config report for each resolved value containing logical field path, winning layer, source location/type, and whether the value is defaulted/overridden/redacted.
- **H3A-F05:** Implement shared legacy-path detection used before discovery, load, mutation, doctor/diagnostics, and CLI/TUI config operations for the three CLEAN-001 path classes.
- **H3A-F06:** Define a structured unsupported-legacy error with code `UNSUPPORTED_LEGACY_CONFIG`, encountered path, supported `gestalt.json` path, and remediation text; app and CLI projections preserve the code.
- **H3A-F07:** Document a future version-reader/migration registration interface and test obligations without adding a command or dormant schema-v2 implementation.

### Behavioral criteria

- **H3A-B01:** Resolution order is defaults < global JSON < workspace JSON < supported environment < CLI flags; invalid higher-precedence input fails instead of falling back.
- **H3A-B02:** Unknown fields, removed aliases, missing/invalid version, and unsupported versions fail with distinct structured errors.
- **H3A-B03:** Detection of a known legacy harness file occurs before file content is read; malformed, huge, unreadable, or malicious content yields the same `UNSUPPORTED_LEGACY_CONFIG` result and is never parsed, merged, seeded, renamed, deleted, or migrated.
- **H3A-B04:** `gestalt.extension.toml` is never classified as legacy harness config and continues through manifest V2 handling.
- **H3A-B05:** Effective-config reports expose secret handles/source metadata only; raw secret values are absent from reports, errors, logs, and snapshots.
- **H3A-B06:** No Workbench/product namespace or no-op migration command is added.

## 9. Public API / Schema / CLI Impact

Freezes `gestalt.json` schema version 1 and structured config/provenance errors. Deprecated fields disappear from Rust/JSON Schema. App/CLI must preserve `UNSUPPORTED_LEGACY_CONFIG`; H3B owns serialized envelope and exit code.

## 10. Failure, Security, and Compatibility Semantics

- Known legacy files take precedence as an explicit unsupported error and are never parsed, merged, seeded, renamed, deleted, or migrated.
- Unknown fields and unsupported versions fail deterministically with distinct codes.
- Secret values never appear in effective-config diagnostics, errors, logs, or snapshots.
- Invalid higher-precedence input does not silently fall back to a lower layer.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H3A-F*` and `H3A-B*` criterion to a schema, config fixture, provenance, rejection, redaction, or absence test.
- Schema/Rust parity and all valid v1 fixtures.
- Unknown top-level/nested/deprecated alias rejection and aliases absent from generated schema.
- Unsupported version, missing/invalid version, and future-version errors.
- Full precedence matrix and source provenance per field.
- Secret provenance redaction.
- Each legacy global/workspace config/policy path returns `UNSUPPORTED_LEGACY_CONFIG` for discover/load/mutate/doctor; unreadable/malformed contents prove no parse.
- `gestalt.extension.toml` is not rejected by harness-config detection.
- Absence searches for TOML fallback/seeding/migration and removed aliases.

## 12. Documentation Updates

Publish the tested v1 config, stability labels, layering/provenance, errors, and future migration policy under `docs/v0.1/`; update schema README, config guides, crate READMEs, and H0C ledger.

## 13. Execution Steps

1. Add failing alias/version/layering/provenance/legacy-path tests.
2. Remove aliases from the model/schema and restore parity.
3. centralize strict version/layer processing and safe provenance.
4. Implement non-parsing legacy-path rejection across all entry points.
5. Remove fallback/seeding dependencies and update active fixtures/docs.

## 14. Exit Criteria

- [x] Schema v1 and Rust model are strict and parity-tested.
- [x] Layering/provenance is deterministic and secrets remain redacted.
- [x] All known legacy paths fail with `UNSUPPORTED_LEGACY_CONFIG` without parsing/migration.
- [x] Deprecated aliases are absent from models, schema, active fixtures, and docs.
- [x] No migration command or product namespace is added.

## 15. Rollback / Partial Completion Handling

Do not remove a fallback until every entry point detects and reports the legacy file safely. If cleanup is partial, keep the branch unreleased, leave ledger rows open, and do not weaken strict validation to accommodate old fixtures.
