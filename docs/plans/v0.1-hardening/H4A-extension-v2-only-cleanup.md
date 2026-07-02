# Plan: H4A Extension V2-Only Cleanup

## 1. Purpose

Remove all manifest/protocol V1 and deprecated extension compatibility so stable v0.1 has only manifest V2 and Lifecycle Protocol V2 paths.

**Implementation status:** in progress. The runtime and tests enforce the V2-only cleanup path, but the phase still has open verification work around the remaining matrix and acceptance boundaries.

## 2. Requirement IDs Covered

CLEAN-003; CLEAN-004 as applied to extensions; EXT-001.

## 3. Current-State Evidence

- `crates/gestalt-runtime/src/discovery.rs` rejects missing and non-2 manifests before activation, and `process_extension.rs` negotiates Lifecycle Protocol V2 only.
- `crates/gestalt-runtime/src/lib.rs` no longer re-exports `GestaltExtension`, and production no longer carries a crate-wide `allow(deprecated)` for the extension path.
- `docs/extension-development-guide.md`, runtime README, architecture doc, manifest schema, and JSON-RPC protocol now describe the V2-only contract.
- V2 tests/fixtures exist in `extension_manifest_v2_tests.rs`, `lifecycle_protocol_v2_tests.rs`, and `tests/fixtures/protocol-v2/`.

## 4. ADR / Spec Constraints

- ADR-028 defines package components; ADR-030 as superseded by ADR-031 makes Lifecycle Protocol V2 exclusive.
- ADR-031 removes the `GestaltExtension` compatibility abstraction where replaced by `RuntimeModule`/V2 components.
- Internal process transport may remain only if used by current V2 capabilities.
- Component stability is classified independently; V1 is outside the matrix.

## 5. In Scope

- Require manifest version 2 and Lifecycle Protocol V2 before activation.
- Remove V1 parsing, inference, conversion, component variants, builders, discovery, launch, brokers/adapters, activation paths, deprecated extension APIs, and broad deprecation allowances.
- Replace V1 compatibility tests with rejection and absence tests.
- Publish the component-level stability matrix and update active V2 docs/fixtures.

## 6. Out of Scope

- Activation criticality, trust-by-integrity, reload preservation, generation pinning, and canonical tool-origin conformance (H4B).
- New extension capabilities, registries, marketplaces, client code execution, remote launchers, or package locks.
- Removing V2 process transport merely because it shares implementation code with old V1.

## 7. Dependencies and Blockers

Depends on 000, H0C extension/deprecation ledger rows, H0B stable export/SPI inventory, and accepted ADR-028/030/031. Coordinate the final matrix with H4B; do not classify behavior whose H0B decision is unresolved.

## 8. Proposed Changes

### Functional criteria

- **H4A-F01:** Make `manifest_version = 2` a required manifest discriminator and require Lifecycle Protocol V2 negotiation; define structured errors for absent, malformed, V1, and unsupported versions.
- **H4A-F02:** Validate version discriminators before component conversion, process launch, broker creation, capability registration, or generation publication.
- **H4A-F03:** Remove every H0C-listed V1 parser, protocol default, V1-to-V2 converter, legacy component variant, builder/discovery/launch method, broker/adapter, activation branch, public export, compatibility test, and V1-only fixture.
- **H4A-F04:** Remove the deprecated `GestaltExtension` compatibility trait/bridge and extension-specific deprecated APIs assigned by H0B/H0C; remove broad `allow(deprecated)` from production and tests. Deprecated provider APIs outside the extension path remain with their ledger owner.
- **H4A-F05:** Remove Cargo dependencies/features that have no caller after V1 cleanup, while retaining process transport required by current Lifecycle V2 capabilities.
- **H4A-F06:** Publish a component matrix with separate rows and maturity for package/manifest validation, configured instances, Lifecycle V2, command tools, MCP-backed tools, context providers, policy guards, reload, client descriptors, package locks, and remote launchers.

### Behavioral criteria

- **H4A-B01:** Absent/malformed/V1/unsupported versions fail before launch or activation, publish no capability/generation, and return the supported V2 version in remediation details.
- **H4A-B02:** No input is inferred, converted, normalized, or routed through protocol 1.x compatibility.
- **H4A-B03:** Valid V2 packages and Lifecycle V2 fixtures retain their existing supported behavior after shared V1 code is removed.
- **H4A-B04:** Client/product descriptors remain inventory-only and the matrix cannot label them runtime-executable.
- **H4A-B05:** Historical V1 documents may remain only under H0A historical/archive classification; active docs and fixtures contain no supported migration or compatibility claim.

## 9. Public API / Schema / CLI Impact

Manifest V2 and Lifecycle V2 are the only supported extension contracts. Deprecated Rust extension APIs disappear without adapters. CLI extension output changes only as needed to report rejection through existing experimental/admin surfaces.

## 10. Failure, Security, and Compatibility Semantics

- Absent, malformed, V1, or unsupported manifest/protocol versions fail before process launch/activation.
- Rejection is structured and identifies the supported V2 contract without converting input.
- Removal cannot weaken permission, validation, policy, or protocol message limits.
- Historical V1 docs may remain archived but cannot advertise a supported migration path.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H4A-F*` and `H4A-B*` criterion to a V2 conformance, pre-launch rejection, compile/search absence, dependency, or documentation check.
- Valid manifest V2 and Lifecycle V2 conformance remain green.
- Rejection fixtures: absent version, malformed version, manifest 1, protocol 1.x, mismatched negotiation, V1-only component.
- Assert rejected packages launch no process and publish no generation/capability.
- Compile/search absence checks for V1 types/converters/builders/adapters, `GestaltExtension`, broad `allow(deprecated)`, and deprecated extension exports.
- Cargo dependency/feature audit and active-doc/fixture scans.

## 12. Documentation Updates

Publish the component stability matrix and V2-only authoring/protocol references; update extension guide/schema, JSON-RPC reference, runtime README/reference, crate READMEs, migrations disposition, and H0C ledger.

## 13. Execution Steps

1. Add V1/absent/malformed rejection and no-launch tests.
2. Enforce required V2 discriminators and negotiation.
3. Remove V1 conversions/adapters/builders/exports and deprecated bridges.
4. Remove unused dependencies/allowances and invert compatibility fixtures.
5. Update the stability matrix and active documentation; run absence checks.

## 14. Exit Criteria

- [ ] Only manifest V2 and Lifecycle Protocol V2 are accepted.
- [ ] V1 and missing versions fail before activation with structured errors.
- [ ] No V1 conversion/adapter/activation path or deprecated extension API remains.
- [ ] Production and tests contain no broad deprecation allowance.
- [ ] Active docs/fixtures describe only greenfield V2 behavior.

## 15. Rollback / Partial Completion Handling

Remove compatibility as one reviewable slice with rejection tests first. If a V2 path still depends on shared legacy code, keep the ledger row open and refactor only that dependency; never temporarily re-enable V1 fallback.
