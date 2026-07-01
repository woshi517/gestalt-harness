# Gestalt Runtime Crate (`gestalt-runtime`)

`gestalt-runtime` is the runtime composition layer of Gestalt. It wires
providers, tools, policy, trust, extension discovery, and lifecycle execution on
top of `gestalt-core`.

Stable v0.1 is V2-only for extension packages and lifecycle protocol. V1
compatibility is removed from the active contract.

For the supported API surface, see [`REFERENCE.md`](./REFERENCE.md).

## Ownership and stability

This crate owns concrete runtime composition, context assembly, providers,
tools, traces, policy enforcement, and extension activation. It does not own
product session workflows, CLI/TUI rendering, or remote transport.

The only v0.1 compatibility namespace is `gestalt_runtime::api::v1`.
`gestalt_runtime::unstable` exists for first-party integration and test support;
it carries no v0.1 compatibility guarantee. There are no runtime types
re-exported from the crate root.

## Primary responsibilities

- run sessions through `AgentRuntime`
- build runtime state through `AgentRuntimeBuilder`
- discover and resolve V2 extension packages
- manage lifecycle-component processes and runtime modules
- publish runtime events and inspection snapshots

## Quick start

```rust
use gestalt_runtime::api::v1::{
    RuntimeBackedControlHost, SessionControlV1, StartSessionRequestV1,
};

let host = RuntimeBackedControlHost::new(builder, artifact_store)?;
let session = host.start_session(StartSessionRequestV1 {
    session_id: None,
    idempotency_key: None,
    config_override: None,
}).await?;
```

`RuntimeBackedControlHost`, the six V1 capability traits, and their versioned
DTOs are the embedding boundary. `InMemoryControlHost` and `MockControlHost`
exist only for conformance tests. See
[`examples/embed_runtime.rs`](./examples/embed_runtime.rs) for a complete
runtime-backed setup.

## Runtime surfaces

### Agent runtime

`AgentRuntime` is the execution boundary around `gestalt-core::AgentLoop`.
It owns the provider, tools, policy engine, approval provider, registry,
hooks, and event bus.

### Builder

`AgentRuntimeBuilder` assembles the runtime and registers native modules through
method calls. Its state is private; consumers cannot mutate the registry,
event bus, provider, tools, or configuration fields directly.

### Registry

The registry is an unstable implementation subsystem. First-party crates that
need it must opt into `gestalt_runtime::unstable`.

## Extension packages

V2 extension packages are parsed from `gestalt.extension.toml` via
`ExtensionManifestV2`.

Supported component kinds:

- `gestalt-lifecycle`
- `command-tool`
- `mcp-server`
- `skill`
- `client-product`

Lifecycle components are launched through `ProcessExtensionBroker` and speak the
typed V2 JSON-RPC protocol. Command tools and MCP servers are registered as
package components and executed through the runtime's tool and MCP surfaces.

## Trust and permissions

Package trust is applied during activation. Component permissions are enforced
host-side before process launch and before tool execution. Permission decisions
are published as `RuntimeEvent::PermissionDecision`.

## Composition hooks

`CompositionHooks` intercept runtime execution at the user-hook layer. The H4A
cleanup removed the old extension hook bridge; lifecycle components now use the
typed protocol instead of injecting hooks through a legacy adapter.

## Failure, cancellation, and feature gates

Builder, activation, provider, trace, and tool failures return structured
errors/reports; required security components fail closed. Cancellation
propagates through provider streaming, approvals, tools, and lifecycle
processes. Features `providers`, `tools`, `trace`, `mcp`, `skills`, and `verify`
only expose their corresponding experimental implementation modules; enabling a
feature does not make its entire surface stable.

## Related docs

- [Extension Package Manifest Schema](../../docs/extension-manifest-schema.md)
- [Gestalt Lifecycle Protocol V2](../../docs/jsonrpc-extension-protocol.md)
- [Extension Development Guide](../../docs/extension-development-guide.md)
