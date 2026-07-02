---
title: "Gestalt App Service Reports v1"
status: published
type: version-contract
target: v0.1
owners:
  - gestalt-app
authority: implementation-contract
---

# Gestalt App Service Reports v1

`gestalt-app` is a product-neutral service boundary. CLI, TUI, Workbench, and
other clients consume structured reports and own all presentation.

## Report invariant

`ServiceReportV1<T>` has `value`, ordered `diagnostics`, `error`, and optional
`correlation_id` fields.

- Success contains `value` and no `error`.
- Failure contains `error` and no `value`.
- Diagnostics may accompany either outcome and retain production order.

`AppDiagnosticV1` contains severity (`warning` or `error`), a stable code,
message, optional correlation ID, and optional structured details.
`AppErrorProjectionV1` contains a stable code, message, retryability flag, and
optional structured details.

## Runtime construction

`build_app_runtime_with_report` returns runtime-construction warnings and
failures without writing presentation output. Its diagnostic codes include:

- `provider_resolution_warning`, `config_warning`, and
  `auth_resolution_warning`;
- `skill_configuration_error` and `skill_trust_error`;
- `extension_rejected`, `untrusted_activation`,
  `extension_activation_warning`, and `extension_activation_error`.

Harness failures project to stable uppercase error codes, including
`AUTH_FAILED`, `POLICY_DENIED`, `SKILL_CONFIGURATION_ERROR`,
`EXTENSION_REJECTED`, `TOOL_PERMISSION_DENIED`, and `CANCELLED`.

The contract is enforced by
[report_contract_tests.rs](../../crates/gestalt-app/tests/report_contract_tests.rs).
