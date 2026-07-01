---
title: Gestalt Harness Product Requirements
status: active
type: prd
target: general
owners:
  - product
---

# Gestalt Harness Product Requirements

## Vision

Gestalt is a local-first harness for safe, inspectable single-agent work. It
provides a small execution kernel that products can embed without inheriting a
particular terminal, desktop, or remote-service experience.

The harness owns execution guarantees. Products own user intent, presentation,
and workflow.

## Users and Problems

### Primary users

- developers embedding an agent loop in a local product;
- operators who need deterministic traces and explicit policy decisions;
- tool and provider authors who need narrow, product-neutral interfaces;
- users performing coding and knowledge work in a controlled workspace.

### Problems

- agent behavior is difficult to inspect when execution, presentation, and
  provider details are coupled;
- tool execution is unsafe when validation, policy, approval, and authority are
  implicit;
- replay and debugging are unreliable without ordered events and captured
  context;
- extension systems become fragile when compatibility promises are accidental;
- products cannot reuse the harness when core APIs expose CLI or UI concerns.

## Product Principles

1. **Small loop:** keep orchestration focused on context, model turns, tools,
   policy, approval, cancellation, and events.
2. **Events are ground truth:** clients render state from ordered events instead
   of hidden mutable state.
3. **Host-owned authority:** providers, tools, and extensions describe
   capabilities; the host grants effective authority.
4. **Product-neutral contracts:** stable APIs and DTOs contain no CLI, TUI,
   desktop, or provider-native types.
5. **Deterministic where it matters:** identities, context inputs, policy
   decisions, and replay metadata are explicit.
6. **Bounded operations:** queues, artifacts, tool execution, and context inputs
   have defined limits and structured failures.
7. **Greenfield v0.1:** obsolete pre-release formats and deprecated APIs are
   removed rather than preserved through compatibility adapters.

## Scope

### v0.1 priorities

- a product-neutral runtime-control boundary;
- stable tool-authoring and selected provider-authoring interfaces;
- strict `gestalt.json` configuration;
- traceable policy, approval, tool, and context behavior;
- extension manifest and lifecycle protocol V2 only;
- stable, explicitly selected CLI automation commands;
- versioned client, trace, and diagnostic contracts;
- clear documentation authority and crate ownership.

### Supported execution

- local single-agent sessions;
- interactive and non-interactive clients;
- built-in, extension, command, and MCP-backed tools under the same validation
  and policy pipeline;
- local traces, run manifests, artifacts, and deterministic replay inputs.

## Boundaries

Gestalt v0.1 does not promise:

- multi-agent orchestration;
- remote task execution or a wire-level task bundle;
- process isolation equivalent to a security sandbox;
- compatibility with pre-hardening configuration, extension V1, or persisted
  development formats;
- stability for every public Rust item or every CLI command;
- product-specific UI or workflow semantics.

Detailed contracts live in:

- [v0.1 contract map](./v0.1/README.md);
- [v0.1 hardening specification](./feature-spec/v0.1-hardening.md);
- [architecture overview](./gestalt-harness-architecture.md);
- [accepted ADRs](./adrs/README.md).

## Success Measures

The release is successful when:

- supported host operations pass one shared conformance suite;
- stable contracts expose no raw internal, provider-native, secret, or absolute
  host-path data;
- all tool origins pass equivalent validation, policy, approval, execution, and
  trace checks;
- cancellation, concurrency, queue, cursor, and artifact bounds are tested;
- configuration and extension V1 inputs fail through stable rejection paths;
- stable CLI commands have normalized JSON snapshots and documented exits;
- no deprecated pre-hardening Rust API or unclassified compatibility path
  remains;
- versioned documentation names the implementation and enforcing tests for
  every published contract.
