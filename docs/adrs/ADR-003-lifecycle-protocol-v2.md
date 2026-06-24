# ADR-003 Lifecycle Protocol V2

Status: Accepted

Lifecycle protocol v2 replaces generic external hooks with typed capabilities: context providers, policy guards, turn routers, external verifiers, and event observers.

The stable method set is `initialize`, `capabilities/describe`, `lifecycle/invoke`, `shutdown`, and `$/cancelRequest`. DTOs are versioned and must not serialize internal `Session`, raw `ContextPacket`, or raw `AgentEvent` values as the external contract.

Protocol v1 remains compatible through adapters. `HookOutcome::Aggregated` is not part of the v2 external contract.
