# Gestalt Lifecycle Protocol V2

This document describes the JSON-RPC protocol used by process-backed
`gestalt-lifecycle` components.

Stable v0.1 supports only Protocol V2. The broker rejects unsupported
versions, and there is no V1 compatibility fallback.

## Transport

- JSON-RPC 2.0
- newline-delimited JSON over `stdin` / `stdout`
- `stderr` is reserved for logs
- the broker sends all requests; the component never initiates requests

## Message Shape

```rust
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
    pub id: Option<serde_json::Value>,
}

pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
    pub id: Option<serde_json::Value>,
}

pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}
```

The broker always sends an `id` and expects exactly one of `result` or
`error` in the response.

## Methods

### `initialize`

Handshake sent immediately after spawn.

Request:

```json
{"jsonrpc":"2.0","method":"initialize","params":{"supported_versions":["2.0"]},"id":"init-1"}
```

Response:

```json
{"jsonrpc":"2.0","result":{"negotiated_version":"2.0","supports_cancellation":true},"id":"init-1"}
```

The broker rejects any response that does not negotiate `2.0`.

### `capabilities/describe`

Returns the lifecycle capabilities exported by the component.

Request:

```json
{"jsonrpc":"2.0","method":"capabilities/describe","id":"cap-1"}
```

Response payload:

```json
[
  {
    "component_id":"component:com.example.lifecycle:primary:lifecycle",
    "capability":"context_provider",
    "priority":0,
    "timeout_ms":15000,
    "failure_mode":"fail_closed",
    "data_scope":"turn"
  }
]
```

### `lifecycle/invoke`

Invokes one typed lifecycle capability.

Request:

```json
{"jsonrpc":"2.0","method":"lifecycle/invoke","params":{"component_id":"component:com.example.lifecycle:primary:lifecycle","capability":"context_provider","payload":{"session_id":"...","history":[]}},"id":"invoke-1"}
```

Response:

```json
{"jsonrpc":"2.0","result":{"payload":{"content":"extra context"}},"id":"invoke-1"}
```

### `shutdown`

Best-effort shutdown signal. The broker may call it during teardown, but it
also kills the child process on drop.

### `$/cancelRequest`

Standard JSON-RPC cancellation notification. The broker sends it only when the
component reported `supports_cancellation = true` during initialization.

## Failure Rules

- malformed JSON is treated as a protocol error;
- responses without `id` are rejected;
- responses with both `result` and `error` are rejected;
- unknown methods are surfaced as JSON-RPC errors by the component;
- the broker closes and rejects the component if initialization fails.
