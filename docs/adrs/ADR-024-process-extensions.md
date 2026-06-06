# ADR-024: Process-Backed Extensions over Stdio JSON-RPC

**Status:** Accepted

## Context
Users wanted to extend the capabilities of the Gestalt harness (registering new tools, hooks, and context contributors) dynamically without recompiling the main binary. In-process Rust extensions require compile-time registration, which limits flexibility for developer environments.

## Decision
Implement process-backed extensions executed in separate child processes over stdio using a newline-delimited JSON-RPC 2.0 protocol.
- Extensions declare their capabilities (tools, hooks, context contributors) and permissions (paths, network, shell) via a `gestalt.extension.toml` manifest.
- Implement host-side capability and permission verification in the `ProcessExtensionBroker` before executing RPC methods on the child.
- Enforce strict lifecycle bounds, including initialize/request timeouts, output limits, and process reaping on shutdown or failure.
- Introduce `GestaltExtension` trait as the abstraction bridging both process-backed and in-process (Rust) extensions into the `AgentRuntimeBuilder`.
- Implement `ProcessBackedTool` (wrapping `ProcessExtensionBroker` to implement the core `Tool` trait) and `ProcessBackedContextContributor` (implementing `ContextContributor`).
- Introduce `ExtensionDiscovery` with three-tier lookup: explicit CLI paths → project-local (`.gestalt/extensions/`) → global (`~/.config/gestalt/extensions/`). Deduplicated by extension `id`.
- Introduce `RuntimeEvent` variants (`ExtensionDiscovered`, `ExtensionLoaded`, `ExtensionRejected`, `ProcessSpawned`, `ProcessExited`, `RpcRequest`, `RpcResponse`, `PermissionDecision`) for auditability of the extension lifecycle.
- Implement recursive input argument scanning: before forwarding tool calls, the broker recursively walks the JSON input for path-like and network-like keys and enforces declared permissions.
- Implement trust model: explicitly loaded and global extensions are always trusted; project-local extensions require `trusted` list entry or `allow_untrusted = true`.
- Restrict subprocess environments by calling `env_clear()` and only forwarding a safe allowlist of environment variables (PATH, HOME, USER, LOGNAME, SHELL, TERM, LANG, LC_ALL, LC_CTYPE, TMPDIR, TEMP, TMP).
- Reject known shell executables (sh, bash, zsh, cmd, powershell, etc.) as entrypoints when `allow_shell` is false.

## Consequences
- Extensions can be written in any language and loaded dynamically at startup.
- Safe isolation of extension execution; errors in process extensions are caught and isolated, failing open or closed based on configuration.
- Honest security posture: manifest permissions are explicitly presented as host-side verification/gating boundaries rather than operating-system level sandboxing.
