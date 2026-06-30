# Gestalt Runtime Crate (`gestalt-runtime`)

`gestalt-runtime` is the runtime composition layer of Gestalt. It wires
providers, tools, policy, trust, extension discovery, and lifecycle execution on
top of `gestalt-core`.

Stable v0.1 is V2-only for extension packages and lifecycle protocol. V1
compatibility is removed from the active contract.

For the full API surface, see [`REFERENCE.md`](./REFERENCE.md).

## Primary responsibilities

- run sessions through `AgentRuntime`
- build runtime state through `AgentRuntimeBuilder`
- discover and resolve V2 extension packages
- manage lifecycle-component processes and runtime modules
- publish runtime events and inspection snapshots

## Quick start

```rust
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};

let runtime = AgentRuntimeBuilder::new()
    .provider(provider)
    .tools(tools)
    .assembler(assembler)
    .policy(policy)
    .approval(approval)
    .config(RuntimeConfig::default())
    .build()?;
```

## Runtime surfaces

### Agent runtime

`AgentRuntime` is the execution boundary around `gestalt-core::AgentLoop`.
It owns the provider, tools, policy engine, approval provider, registry,
hooks, and event bus.

### Builder

`AgentRuntimeBuilder` assembles the runtime and registers native modules through
`RuntimeModule`.

### Registry

`RuntimeRegistryBuilder` collects tools, providers, context contributors,
verifiers, hooks, and extension IDs before snapshotting them into a
`RuntimeRegistrySnapshot`.

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

## Related docs

- [Extension Package Manifest Schema](../../docs/extension-manifest-schema.md)
- [Gestalt Lifecycle Protocol V2](../../docs/jsonrpc-extension-protocol.md)
- [Extension Development Guide](../../docs/extension-development-guide.md)
