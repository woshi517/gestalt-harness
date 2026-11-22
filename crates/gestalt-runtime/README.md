# `gestalt-runtime`

Reusable Tokio I/O host, OS/VM sandbox drivers, and capability primitives for Gestalt.

`gestalt-runtime` executes the `DriverDirective`s yielded by `gestalt-kernel` against the real operating system and network—while remaining **completely agnostic** to specific agent loops, prompts, tools, model vendors, or UI surfaces.

## Hard Constraints

- `#![forbid(unsafe_code)]`
- `#![deny(missing_docs)]`
- Zero hardcoded loops, tools, model providers, or UI code; all concrete capabilities register via `RuntimeRegistry`, `ProviderRegistry`, and `TriggerManager`.

## Scope & Responsibilities

| Module | Responsibility | Key Types & Functions |
| :--- | :--- | :--- |
| `driver/` | Effect dispatch loop, JSONL session I/O & advisory locking, offline replay runner, and git worktrees | `RuntimeHost`, `RuntimeRegistry`, `ToolHandler`, `CommandHandler`, `PauseHandler`, `EventSink`, `TurnOutcome`, `replay_session_tree`, `create_worktree` |
| `sandbox/` | OS, container, and micro-VM isolation drivers plus in-process path containment | `SandboxDriver`, `SandboxKind`, `LinuxBwrapDriver`, `MacOsSeatbeltDriver`, `DockerSandboxDriver`, `MicroVmSandboxDriver`, `WorkspacePathGuard`, `select_sandbox_driver` |
| `provider.rs` | Vendor-agnostic HTTP/SSE model invocation, token/cost accounting, and alias routing | `ModelProvider`, `ProviderRegistry`, `ProviderContext`, `SharedHttpClient`, `SseLineParser`, `ModelRequest`, `ModelResponse` |
| `credentials/` | Host-side secret storage (`0600` permission check) and output scrubbing | `CredentialStore`, `CredentialResolver`, `CredentialRecord`, `StreamRedactor` |
| `artifacts.rs` | Staged self-modification (`Effect::Save`), SHA-256 manifests, and atomic rollback ledger | `ArtifactManager`, `StageManifest`, `LedgerEntry` |
| `blobs.rs` | Content-addressed `.gestalt/blobs/<sha256>` store and large tool-output spill-to-blob | `BlobStore`, `DEFAULT_INLINE_SPILL_BYTES` |
| `config.rs` | Layered `gestalt.toml` loading (Global < Workspace) and explicit workspace trust gate | `GestaltConfig`, `ResolvedRuntimeConfig`, `WorkspaceTrustStore`, `load_and_merge_config` |
| `ext_ipc.rs` | Out-of-process stdio JSON-RPC worker host for polyglot / hot-reloadable extensions | `ExtIpcHost`, `ExtIpcWorker` |
| `mcp.rs` | Stdio MCP (`2025-11-25`) client bridge exposing external MCP tools through `PolicyGate` | `McpClient`, `McpServerInfo`, `MCP_PROTOCOL_VERSION` |
| `trigger.rs` | Background event/schedule trigger lifecycle and idle-activity tracking | `TriggerManager`, `TriggerHandler`, `TriggerEvent`, `TriggerStatus`, `ActivityTracker` |
| `lib.rs` | Shared runtime errors, provider error classification, and cooperative cancellation | `RuntimeError`, `ProviderErrorKind`, `CancellationToken` |

## What Does *Not* Belong Here

- Kernel policy/tree invariants (owned by `gestalt-kernel`).
- Concrete loops (`ChatLoop`, `DecisionRouter`), built-in tools (`read`, `edit`, `bash`), vendor adapters (`anthropic`, `openai`, `jev`), or user-facing surfaces (`tui`, `rpc`, `acp`).

## Verification

```bash
cargo test -p gestalt-runtime
```
