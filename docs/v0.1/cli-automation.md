---
title: "CLI Automation Contract v1"
status: published
type: version-contract
target: v0.1
owners:
  - gestalt-cli
authority: implementation-contract
---

# CLI Automation Contract v1

## Envelope

Stable JSON commands emit exactly one document to stdout on success or stderr
on failure:

```json
{
  "schema_version": 1,
  "status": "success",
  "kind": "config.validate",
  "data": {},
  "error": null,
  "warnings": []
}
```

Errors set `status` to `error`, `data` to `null`, and provide `code`,
`message`, `retryable`, optional redacted `details`, and optional
`correlation_id`.

`warnings` contains app-service diagnostics that did not prevent the command
from succeeding. Each warning has `severity`, `code`, `message`, optional
`correlation_id`, and optional redacted `details`. Reports with no diagnostics
emit an empty array.

## Exit Codes

| Exit | Category |
|---:|---|
| 0 | success |
| 2 | command usage |
| 3 | configuration or validation |
| 4 | not found or conflict |
| 5 | execution |
| 6 | permission, policy, or approval |
| 7 | unavailable or feature-disabled |
| 1 | internal |

Broken stdout pipes terminate without being reclassified as execution failures.

## Stable Commands

All use `--format json`; errors use the codes and exits above.

| Command | Kind | Rationale | Features | Evidence |
|---|---|---|---|---|
| `config validate` | `config.validate` | CI validation | base | `main_cli_contract_tests`, `config_cli_tests` |
| `config show` | `config.show` | redacted effective configuration | base | `config_cli_tests` |
| `config explain` | `config.explain` | provenance audit | base | `config_cli_tests` |
| `workspace info` | `workspace.info` | workspace inspection | base | `workspace_contract_tests` |
| `providers list`, `providers inspect` | `providers.list`, `providers.inspect` | provider discovery | providers | `main_cli_contract_tests` |
| `profiles list`, `profiles inspect` | `profiles.list`, `profiles.inspect` | profile discovery | providers | `main_cli_contract_tests` |
| `models list`, `models inspect` | `models.list`, `models.inspect` | model discovery | providers | `main_cli_contract_tests` |
| `policy explain` | `policy.explain` | policy audit | tools | `policy_cli_tests` |
| `tools list`, `tools inspect` | `tools.list`, `tools.inspect` | tool inspection | tools | `tools_cli_tests` |
| `connect` | `connect` | provider credential setup | providers | `main_cli_contract_tests` |
| `runs list`, `runs inspect` | `runs.list`, `runs.inspect` | run metadata inspection | trace | `runs_cli_tests` |
| `trace inspect` | `trace.inspect` | trace summary inspection | trace | `trace_cli_tests` |
| `context explain` | `context.explain` | bounded context diagnostics | providers | `context_cli_tests` |
| `extension validate` | `extension.validate` | V2 manifest validation | base | `runtime_cli_tests`, `cli_automation_contract_tests` |

## Experimental Commands

`run`, `chat`, `tui`, `replay`, `cost`, `auth`, `disconnect`, profile/model
mutation, `init`, `status`, workspace mutation/doctor/snapshot, run mutation,
trace replay/validation/analysis, sessions, export, verify, policy
test/validate, tool classification, runtime inspection, extension
administration other than validation, skills, and doctor are experimental.

## Dependency Status

H0B accepted the envelope and stable-candidate command inventory. The subset is
published with normalized success and failure snapshots for the supported
feature combinations.

## Conformance Evidence

| Criteria | Evidence |
|---|---|
| H3B-F03, B01, B02 | Central `handle_result` dispatcher, machine-readable usage errors, and `main_cli_contract_tests::test_cli_json_usage_errors_are_machine_readable` |
| H3B-F04, F05, B03 | Central domain-error/exit mapping and legacy-config CLI assertion |
| H3B-B04 | `minimal_feature_cli_tests` with `--no-default-features`, including disabled-command and usage-error assertions |
| H3B-B06 | `write_stdout` treats `BrokenPipe` as successful termination |
| H3B-F01, F02 | `STABLE_COMMANDS_V1` and the stable/experimental command tables above |
| H3B-F06, B05 | `crates/gestalt-cli/tests/h3b_snapshot_tests.rs` and `crates/gestalt-cli/tests/snapshots/` |
| Warning projection | `cli_automation_contract_tests::cli_json_warnings_include_app_diagnostics` and report-specific warning projection |
