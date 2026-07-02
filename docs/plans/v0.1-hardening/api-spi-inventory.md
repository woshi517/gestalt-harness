---
title: Stable v0.1 API and SPI Inventory
status: accepted
type: decision-record
target: v0.1
owners:
  - gestalt-core
  - gestalt-runtime
  - gestalt-app
---

# Stable v0.1 API and SPI Inventory

## Classification Rule

The stable surface is allowlisted below. Every other technically public module,
re-export, constructor, type, report, and feature-gated item in `gestalt-core`,
`gestalt-runtime`, and `gestalt-app` is **experimental v0.1** unless a row
explicitly marks it internal. Rust visibility alone never creates a stability
promise.

This deny-by-default rule classifies the full public tree without duplicating
more than one thousand implementation declarations into a manual spreadsheet.
The stable rows are the compatibility inventory that H5 freezes.

## Stable Rust Embedding API

| Path / symbols | Audience | Class | Gate | Owner | Failure and panic contract | Disposition | Evidence |
|---|---|---|---|---|---|---|---|
| `gestalt_runtime::control::contract::{RuntimeControlV1, SessionControlV1, RunQueryV1, ApprovalControlV1, EventSourceV1, ArtifactAccessV1, RuntimeInspectionV1}` | embedding hosts | Rust embedding API | base | runtime | `ControlErrorV1`; documented validation/policy/provider/tool/trace/context/cancellation/concurrency/retry classes; no expected panic | stable | `control_conformance` |
| `gestalt_runtime::control::contract` request, response, ID, cursor, event, artifact, policy, approval, inspection, and error DTOs used by those traits | embedding clients | client DTO | base | runtime | serializable v1 DTOs; no raw model, registry, receiver, absolute path, secret, or internal chain | stable | `control_conformance::test_dto_serialization_round_trip` |
| `gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig, AgentRuntime}` | embedding hosts | construction API | base | runtime | builder returns `RuntimeError`; expected configuration/startup failure does not panic | stable | `runtime_builder_tests`, `runtime_run_tests` |
| `gestalt_app::workspace::{init_workspace, info_workspace}` and their request/report types | embedding hosts | app service | base | app | expected I/O/config/validation failures return `HarnessError` | stable | `workspace_contract_tests`, `report_contract_tests` |

`RuntimeHost` and the full session-owning host implementation remain proposed
until H1B's local and mock implementations pass the same conformance suite.

## Stable Authoring SPIs

| Trait | Exact methods | Class | Gate | Owner | Failure and panic contract | Evidence |
|---|---|---|---|---|---|---|
| `gestalt_core::Provider` | `id`, `display_name`, `default_model`, `capabilities`, `model_info`, `count_tokens`, `count_request_tokens`, `stream`, `adapt_tools` | external provider SPI | base | core | normalized `HarnessError`; cancellation through the stream consumer; no expected panic | provider contract tests |
| `gestalt_core::Tool` | `name`, `description`, `schema`, `risk`, `can_run_in_parallel`, `execute`, `descriptor` | external tool SPI | base | core | `ToolError`; host-enforced timeout/cancellation/policy; no expected panic | tool and tool-validation tests |
| `gestalt_core::ToolCatalog` | `schemas`, `get`, `get_by_id`, `descriptors` | external tool SPI | base | core | missing tools return `None`; no expected panic | tool catalog tests |
| `gestalt_core::PolicyEngine` | `evaluate` | external policy SPI | base | core | `PolicyError`; failures are fail-closed | policy tests |
| `gestalt_core::ApprovalProvider` | `request_approval` | external approval SPI | base | core | `ApprovalError`; cancellation/expiry are explicit | approval tests |
| `gestalt_core::ContextAssembler` | `assemble` | external context SPI | base | core | `ContextError`; caller supplies bounded input | context tests |

`ContextPipeline`, composition hooks, trace sinks, artifact stores, registries,
extension managers, raw runtime events, and raw sessions are experimental or
internal implementation interfaces, not stable client contracts.

## Versioned Non-Rust Contracts

| Surface | Class | Stable shape | Owner | Evidence |
|---|---|---|---|---|
| `gestalt.json` | JSON schema | strict integer version 1 | app | `config_schema_tests`, `config_tests` |
| CLI JSON envelope | CLI contract | `{schema_version,status,kind,data,error,warnings}` | CLI | `main_cli_contract_tests` |
| selected CLI commands | CLI contract | command matrix in the accepted H0B decision | CLI | H3B per-command snapshots before publication |
| trace event envelope | persisted contract | independently versioned metadata-bearing JSONL record | runtime | H2A fixtures before publication |
| context build report | persisted/client diagnostic | `ContextBuildReportV1` | runtime | `context_report_contract_tests`; publication waits for H2B |

## Forbidden Stable Boundary Types

Stable client DTOs and persisted public records must not contain raw `Session`,
`AgentEvent`, `RuntimeEvent`, `RuntimeConfig`, provider-native values,
registries, broadcast receivers, artifact stores, absolute host paths, secrets,
or serialized Rust error chains.

## Compatibility Impact

Items outside the allowlist may change during v0.1 hardening. Moving an item
from experimental to stable requires updating this inventory, its version
contract, and a named conformance test. Removed pre-hardening APIs are tracked
in the [removal ledger](./pre-hardening-removal-ledger.md); no deprecated shim is
provided.
