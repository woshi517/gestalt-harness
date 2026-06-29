# ADR-030 Lifecycle Protocol V2

Status: Accepted

> **Superseded in part by
> [ADR-031](./ADR-031-v0-1-greenfield-compatibility-cutoff.md):** Protocol V1
> compatibility through adapters is removed before stable v0.1. Lifecycle
> Protocol V2 is the only supported Gestalt lifecycle protocol.

Lifecycle protocol v2 replaces generic external hooks with typed capabilities: context providers, policy guards, turn routers, external verifiers, and event observers.

The stable method set is `initialize`, `capabilities/describe`, `lifecycle/invoke`, `shutdown`, and `$/cancelRequest`. DTOs are versioned and must not serialize internal `Session`, raw `ContextPacket`, or raw `AgentEvent` values as the external contract.

Protocol v1 remains compatible through adapters. `HookOutcome::Aggregated` is not part of the v2 external contract.

`initialize` declares the negotiated protocol version. `InitializeResponseV2` includes an explicit `supports_cancellation` flag; when set, cancellation is treated as best-effort.

*Note on Implementation:* The process-backed client `ProcessLifecycleClient` negotiates the version via `initialize`, retrieves descriptors via `capabilities/describe`, executes hooks/actions via `lifecycle/invoke` over the child process stdin/stdout, and issues `shutdown` signals. Runtime execution pins one `RuntimeSnapshotLease` per run so capability dispatch remains isolated from later reloads.
