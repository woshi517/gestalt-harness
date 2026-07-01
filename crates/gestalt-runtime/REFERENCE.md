# Gestalt Runtime API Reference

The supported v0.1 Rust boundary is:

```rust
gestalt_runtime::api::v1
```

It contains:

- `AgentRuntimeBuilder`, `AgentRuntime`, `RuntimeConfig`, and runtime errors;
- provider/tool/context construction through opaque builder methods;
- `RuntimeBackedControlHost`;
- the six `RuntimeControlV1` capability traits and their versioned DTOs;
- artifact stores used to construct a runtime-backed host;
- in-memory and mock control hosts for conformance testing.

`AgentRuntimeBuilder` fields are private. Configure it through `provider`,
`tools`, `assembler`, `policy`, `approval`, `trace_sink`, `config`, `hooks`,
`extension_package`, `workspace_context_snapshot`, and `build`.

Everything under `gestalt_runtime::unstable` is explicitly outside the v0.1
compatibility promise. That namespace includes activation, registries, raw
runtime events, extension managers, lifecycle clients, orchestration internals,
queues, planners, providers, built-in tools, trace utilities, skills, MCP, and
verification implementations.

The crate root intentionally exports only `api` and `unstable`. Compile-fail
rustdoc checks enforce both the missing crate-root aliases and builder field
privacy. `tests/public_api_contract.rs` compile-checks construction through the
stable namespace.

Publication status for each protocol remains governed by the
[v0.1 contract inventory](../../docs/v0.1/contract-inventory.md).
