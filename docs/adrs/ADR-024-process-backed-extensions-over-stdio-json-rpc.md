# ADR-024: Process-Backed Extensions over Stdio JSON-RPC

**Status:** Accepted

## Context
Users wanted to extend harness capabilities (tools, hooks, context contributors) dynamically without recompiling.

## Decision
Support running dynamic extensions in child processes over stdio using newline-delimited JSON-RPC 2.0. Enforce capabilities and permissions (paths, network, shell) via manifest check on the host side, with timeouts, output limits, and auto-reaping on exit.

## Consequences
Multi-language extensibility with runtime isolation and transparent security/permission checks.
