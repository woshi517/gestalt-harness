# ADR-024: Process-Backed Extensions over Stdio JSON-RPC

**Status:** Accepted

## Context
Users wanted to extend the capabilities of the Gestalt harness (registering new tools, hooks, and context contributors) dynamically without recompiling the main binary. In-process Rust extensions require compile-time registration, which limits flexibility for developer environments.

## Decision
Implement process-backed extensions executed in separate child processes over stdio using a newline-delimited JSON-RPC 2.0 protocol.
- Extensions declare their capabilities (tools, hooks, context contributors) and permissions (paths, network, shell) via a `gestalt.extension.toml` manifest.
- Implement host-side capability and permission verification in the `ProcessExtensionBroker` before executing RPC methods on the child.
- Enforce strict lifecycle bounds, including initialize/request timeouts, output limits, and process reaping on shutdown or failure.

## Consequences
- Extensions can be written in any language and loaded dynamically at startup.
- Safe isolation of extension execution; errors in process extensions are caught and isolated, failing open or closed based on configuration.
- Honest security posture: manifest permissions are explicitly presented as host-side verification/gating boundaries rather than operating-system level sandboxing.
