# ADR-003 Lifecycle Protocol V2

Status: Accepted

Lifecycle protocol v2 replaces generic external hooks with typed capabilities: context providers, policy guards, turn routers, external verifiers, and event observers.

The stable method set is `initialize`, `capabilities/describe`, `lifecycle/invoke`, `shutdown`, and `$/cancelRequest`. DTOs are versioned and must not serialize internal `Session`, raw `ContextPacket`, or raw `AgentEvent` values as the external contract.

Protocol v1 remains compatible through adapters. `HookOutcome::Aggregated` is not part of the v2 external contract.

*Note on Implementation:* The process-backed client `ProcessLifecycleClient` is fully implemented and tested. It negotiates the version via `initialize`, retrieves descriptors via `capabilities/describe`, executes hooks/actions via `lifecycle/invoke` over the child process stdin/stdout, and issues `shutdown` signals.

