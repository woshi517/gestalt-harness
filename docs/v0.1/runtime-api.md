---
title: Gestalt Runtime Rust API
status: active
type: version-contract
target: v0.1
---

# Gestalt Runtime Rust API

The deliberate v0.1 Rust namespace is:

```rust
gestalt_runtime::api::v1
```

It exposes runtime construction, the real runtime-backed control host,
versioned control DTOs and traits, and artifact-store inputs required by that
host. Runtime types are not re-exported from the crate root.

```rust
use gestalt_runtime::api::v1::{
    AgentRuntimeBuilder, InMemoryArtifactStore, RuntimeBackedControlHost,
    RuntimeConfig,
};

let builder = AgentRuntimeBuilder::new().config(RuntimeConfig::default());
let host = RuntimeBackedControlHost::new(
    builder,
    std::sync::Arc::new(InMemoryArtifactStore::new()),
)?;
# Ok::<(), gestalt_runtime::api::v1::RuntimeError>(())
```

`AgentRuntimeBuilder` has opaque state. Supported inputs are supplied through
builder methods; direct mutation of its registry, event bus, provider, tools,
or configuration is rejected by the compiler.

## Unstable surface

`gestalt_runtime::unstable` contains first-party implementation APIs and test
support. Its activation pipeline, registries, raw events, lifecycle clients,
orchestration types, queues, planners, providers, built-in tools, trace
utilities, skills, MCP, and verification types can change without a v0.1
compatibility guarantee.

Rust visibility is not protocol publication. The
[contract inventory](./contract-inventory.md) remains authoritative for whether
each versioned protocol is published or still gated.
