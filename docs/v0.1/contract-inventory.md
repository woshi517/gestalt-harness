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

## Unpublished

| Contract target | Missing gate | Current evidence |
|---|---|---|
| Runtime control and artifact access | Real runtime-backed execution, event, approval, cancellation, trace, and artifact integration | In-memory `LocalControlHost` and `MockControlHost` pass `control_conformance` |
| Policy and approval | Real policy-confirmed tool execution through runtime control | DTO and in-memory conformance tests |
| App service reports | Runtime-factory diagnostics and complete value/error contract tests | `report_contract_tests` |
| CLI automation | Upstream diagnostics must reach the JSON `warnings` field | JSON snapshots and envelope tests |
| Trace and client events | Complete lossless trace/client projection split and forward-compatible projection tests | `trace_contract_tests` |
| Context diagnostics | Safe capture policy, redaction, bounds, replay integrity, and durability tests | `context_report_contract_tests` |
| Extension packages/components | Complete trust, activation, permission, and generation matrix | extension V2 integration tests |

## Experimental and Internal Rust Surface

All other public items in `gestalt-core`, `gestalt-runtime`, `gestalt-app`, and
`gestalt-cli` are experimental until added to the published table.
`AgentRuntimeBuilder`, its public fields, crate-root runtime re-exports, raw
events, registries, queues, planners, snapshots, and activation internals are
therefore not stable v0.1 APIs. Private and `pub(crate)` items are internal.

## Deprecated and Removed

Legacy TOML configuration, removed configuration aliases, `secret:` auth
references, and extension manifest/protocol V1 are unsupported. Rejection
tests are compatibility evidence; they do not make those formats active
contracts.
