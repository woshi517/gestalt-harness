---
title: "CLI Automation Contract v1"
status: active
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
| `config explain` | `config.explain` | provenance audit | base | `config_cli_tests` |
| `workspace info` | `workspace.info` | workspace inspection | base | `workspace_contract_tests` |
| `providers list`, `providers inspect` | `providers.list`, `providers.inspect` | provider discovery | providers | `main_cli_contract_tests` |
| `profiles list`, `profiles inspect` | `profiles.list`, `profiles.inspect` | profile discovery | providers | `main_cli_contract_tests` |
| `models list`, `models inspect` | `models.list`, `models.inspect` | model discovery | providers | `main_cli_contract_tests` |
| `policy explain` | `policy.explain` | policy audit | tools | `policy_cli_tests` |
| `tools list`, `tools inspect` | `tools.list`, `tools.inspect` | tool inspection | tools | `tools_cli_tests` |

## Experimental Commands

`run`, `chat`, `tui`, `replay`, `cost`, `auth`, `connect`, `disconnect`,
profile/model mutation, `init`, `status`, workspace mutation/doctor/snapshot,
all run and trace commands, sessions, export, verify, policy test/validate,
context diagnostics, tool classification, runtime inspection, extension
administration, skills, and doctor are experimental. Run, trace, and context
commands remain experimental until H2A/H2B are frozen; runtime inspection
remains experimental until H4B is frozen.

## Dependency Status

H0B accepted the envelope but did not materialize the stable-command inventory
required by H0B-F05 and H3B-F02. The table above is therefore a conservative
provisional subset. Its implemented envelope and error behavior are usable,
but the subset is not frozen until H0B records the inventory and each selected
command has the H3B-F06 snapshot set.

## Conformance Evidence

| Criteria | Evidence |
|---|---|
| H3B-F03, B01, B02 | Central `handle_result` dispatcher and `main_cli_contract_tests` |
| H3B-F04, F05, B03 | Central domain-error/exit mapping and legacy-config CLI assertion |
| H3B-B04 | `minimal_feature_cli_tests` with `--no-default-features` |
| H3B-B06 | `write_stdout` treats `BrokenPipe` as successful termination |
| H3B-F01, F02, F06, B05 | Blocked on the missing H0B command inventory and per-command normalized snapshots |
