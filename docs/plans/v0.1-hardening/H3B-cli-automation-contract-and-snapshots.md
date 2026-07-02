# Plan: H3B CLI Automation Contract and Snapshots

## 1. Purpose

Freeze a deliberately small automation-oriented CLI subset with one JSON envelope, stable errors/exit codes, and normalized contract snapshots.

## 2. Requirement IDs Covered

CLI-001, CLI-002, CLI-003, CLI-004; POL-004 where policy reports are exposed through stable commands.

## 3. Current-State Evidence

- `crates/gestalt-cli/src/output.rs` defines `JsonEnvelope { schema_version, kind, data }` and `CliErrorPayload { code, message, details }`.
- Command tests are spread across config, workspace, provider/model, runs, trace, policy, tools, runtime, and feature-matrix test files.
- Existing `tests/fixtures/cli-golden/` holds the legacy text goldens; the stable JSON command snapshots now live under `crates/gestalt-cli/tests/snapshots/`.
- Reusable app reports exist, but command-specific stdout/stderr and exit mappings are not centrally frozen.

## 4. ADR / Spec Constraints

- **Blocked until H0B accepts** the stable command inventory and current-versus-richer envelope decision.
- H3A owns config error semantics; H1/H2 own DTO/event/report semantics.
- Stable JSON output must not introduce a second envelope.
- Interactive, destructive, TUI, extension admin, skills, and verification commands may remain experimental.

## 5. In Scope

- Stable-command table based on automation value and feature availability.
- One final success/error JSON envelope, stable kind names, error codes, process exits, and stdout/stderr rules.
- Feature-disabled behavior and normalized JSON snapshot/semantic tests.
- Config/workspace/provider/profile/model/run/trace/policy/tool/runtime/noninteractive-run commands only where H0B marks them stable.

## 6. Out of Scope

- Stabilizing every command, interactive rendering, TUI, shell completion, extension administration, or changing underlying domain contracts.
- Compatibility with an unaccepted pre-freeze JSON draft beyond H0B's explicit decision.

## 7. Dependencies and Blockers

Depends on accepted H0B CLI decision, H3A structured config/provenance errors, H1A stable errors, H2A trace/event readers, H2B context reports, and H4B inspection reports for any selected command. A command remains experimental if its dependency is not frozen.

## 8. Proposed Changes

### Functional criteria

- **H3B-F01:** Produce a command matrix listing every CLI command/subcommand with maturity, automation rationale, required features, stable options, success `kind` and data schema, error codes, exit codes, stdout/stderr contract, and snapshot/test path.
- **H3B-F02:** Mark only the H0B-selected config, workspace, provider/profile/model, run/trace, policy, tool/runtime inspection, and non-interactive run commands stable; mark every unselected command experimental in both matrix and user documentation.
- **H3B-F03:** Implement one output dispatcher for all stable commands using exactly the accepted success and error envelopes and schema version.
- **H3B-F04:** Define the accepted JSON error fields—stable code, message, retryability, optional redacted details, and optional correlation ID—and a total mapping from stable domain errors to CLI errors.
- **H3B-F05:** Define one process exit value for each H0B-approved category: success, usage, config/validation, not-found/conflict, execution, permission/policy, unavailable/feature-disabled, and internal.
- **H3B-F06:** Add one normalized success snapshot and representative error snapshot for every stable command and build-feature combination.

### Behavioral criteria

- **H3B-B01:** In JSON mode a stable command emits exactly one JSON document, no ANSI/progress text, and no trailing diagnostic records; success and failure use the H0B-approved streams.
- **H3B-B02:** Warnings are carried in the accepted structured location and never corrupt the machine document; text mode may render them without changing code or exit mapping.
- **H3B-B03:** Stable domain codes, including `UNSUPPORTED_LEGACY_CONFIG`, are preserved rather than collapsed into a generic CLI code.
- **H3B-B04:** A disabled optional feature returns the documented structured unavailable error and exit code; it does not silently omit a stable command or panic.
- **H3B-B05:** Snapshot normalization changes only declared timestamps, generated IDs, hashes, and temporary paths. Secret values, ANSI, and unexpected absolute paths fail the test rather than being normalized away.
- **H3B-B06:** Broken-pipe termination follows the accepted CLI behavior and is not reported as a harness execution failure.

## 9. Public API / Schema / CLI Impact

Freezes only rows marked stable in the command matrix. Their command names/options, envelope/kind/data schema, errors, exits, and stream behavior become v0.1 contracts; all others are explicitly experimental.

## 10. Failure, Security, and Compatibility Semantics

- Domain error codes, especially `UNSUPPORTED_LEGACY_CONFIG`, survive projection unchanged.
- Feature-disabled stable commands fail with a structured unavailable code and documented exit, never disappear ambiguously.
- JSON output contains no ANSI, progress, secrets, raw error chains, or absolute paths where contracts require logical/relative references.
- Broken-pipe behavior must not be reported as a harness execution failure.

## 11. Tests and Fixtures

- Maintain a criterion-to-evidence matrix mapping every `H3B-F*` and `H3B-B*` criterion to a command-matrix row, snapshot, exit/stream assertion, feature build, or security scan.
- One normalized success and representative failure snapshot per stable command.
- Exact envelope version/kind, JSON error fields, exit code, stdout/stderr, and no-extra-bytes assertions.
- Feature-disabled builds for every stable conditional command.
- Unknown option/usage, domain validation, policy denial, cancellation, not-found, conflict, and internal-failure mappings.
- Secret/ANSI/absolute-temp-path scans.
- Snapshot normalizers independently tested for timestamps, IDs, hashes, and paths.

## 12. Documentation Updates

Publish the stable command matrix and automation contract under `docs/v0.1/`; update CLI README/help examples and `tests/fixtures/cli-golden/README.md`. Label excluded commands experimental.

## 13. Execution Steps

1. Materialize the accepted stable-command matrix and failing contract tests.
2. Centralize envelope/error/exit/stream handling.
3. Migrate selected commands to reusable app/domain reports.
4. Add feature-disabled and normalized snapshots.
5. Update help/docs and verify no experimental command is implied stable.

## 14. Exit Criteria

- [x] Every selected stable command has frozen success/error/exit/stream behavior.
- [x] One and only one JSON envelope is emitted.
- [x] Feature-disabled behavior and all snapshots pass.
- [x] Domain stable codes are preserved and sensitive/nondeterministic data is normalized or absent.
- [x] Non-selected commands are clearly experimental.

## 15. Rollback / Partial Completion Handling

Keep a command experimental until all contract dimensions and snapshots pass. If envelope migration is selected, change the chosen subset atomically; do not support two undocumented envelopes.
