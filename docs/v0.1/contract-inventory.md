---
title: Gestalt v0.1 Contract Inventory
status: active
type: version-contract
target: v0.1
owners:
  - gestalt-core
  - gestalt-runtime
  - gestalt-app
  - gestalt-cli
---

# Gestalt v0.1 Contract Inventory

This inventory is deny-by-default: an exported Rust item is experimental unless
the published table names it. Rust visibility alone is not a compatibility
promise.

| Status | Meaning |
|---|---|
| published | Implemented, documented, and enforced by the named tests |
| unpublished | A versioned target exists, but a required gate is incomplete |
| experimental | Publicly reachable without a v0.1 compatibility promise |
| internal | Private or `pub(crate)` implementation detail |
| deprecated/removed | Unsupported; retained only as rejection or historical evidence |

## Published

| Contract | Specification | Implementation | Enforcing tests |
|---|---|---|---|
| `gestalt.json` version 1 | [configuration.md](./configuration.md) | `gestalt_app::config` and `docs/schemas/gestalt.schema.json` | `config_schema_tests`, `config_tests` |
| Trace envelope and client event version 1 | [trace-events.md](./trace-events.md) | `TraceEventV1`, `ClientEventPayloadV1`, and `project_client_event_line` | `trace_contract_tests` |
| Context build report version 1 | [context-build-report.md](./context-build-report.md) | `ContextBuildReportV1` and `ContextCaptureMode` | `context_report_contract_tests`, `context_management_tests` |
| CLI automation envelope and stable command matrix | [cli-automation.md](./cli-automation.md) | `JsonEnvelope`, `CliReport::diagnostics`, and `STABLE_COMMANDS_V1` | `cli_automation_contract_tests`, `h3b_snapshot_tests` |
| Extension package/activation contract | [extensions.md](./extensions.md) | V2 package discovery, trust pins, explicit instances, effective permissions, and generation leases | extension manifest, instance, permission, manager, reload, command-tool, and runtime CLI tests |
| Policy and approval | [policy-approval.md](./policy-approval.md) | `ApprovalControlV1`, policy/approval projections, runtime approval broker, and bounded session grants | `control_conformance`, `runtime_control_real_run`, and core approval tests |

## Unpublished

| Contract target | Missing gate | Current evidence |
|---|---|---|
| Runtime control and artifact access | Remaining artifact-security and event-projection gates | `api::v1` and its compile-fail boundary checks are enforced by `public_api_contract`; `RuntimeBackedControlHost` passes shared conformance plus `runtime_control_real_run`; in-memory and mock hosts are test support |
| App service reports | Runtime-factory diagnostics and complete value/error contract tests | `report_contract_tests` |

## Experimental and Internal Rust Surface

All other public items in `gestalt-core`, `gestalt-runtime`, `gestalt-app`, and
`gestalt-cli` are experimental until added to the published table.
The deliberate Rust boundary is `gestalt_runtime::api::v1`; it is documented in
[runtime-api.md](./runtime-api.md). `AgentRuntimeBuilder` state is private and
the crate root exports no runtime types. Raw events, registries, queues,
planners, snapshots, activation internals, and every item under
`gestalt_runtime::unstable` are experimental. Private and `pub(crate)` items are
internal.

## Deprecated and Removed

Legacy TOML configuration, removed configuration aliases, `secret:` auth
references, and extension manifest/protocol V1 are unsupported. Rejection
tests are compatibility evidence; they do not make those formats active
contracts.
