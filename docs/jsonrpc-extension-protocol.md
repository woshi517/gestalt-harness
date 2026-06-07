# Gestalt JSON-RPC Extension Protocol Specification

**Version:** 1.0  
**Status:** Implemented  
**Runtime:** `stdio` (MVP)

---

## 1. Protocol Overview

The Gestalt extension protocol uses **JSON-RPC 2.0** over **newline-delimited JSON (NDJSON)** on the child process's **stdin/stdout** pipes. Each RPC message is a single line of JSON terminated by `\n`. The broker (host) sends requests; the extension reads from stdin and writes responses to stdout.

**Key architectural rules:**

- **One request per line, one response per line.** No whitespace or framing between JSON objects.
- **Requests always include an `id`** (UUID v4 string). The broker tracks pending request IDs and matches responses by `id`.
- **Stderr is reserved for extension-side logging.** The broker drains stderr asynchronously and publishes lines as `ExtensionError` events, but **never** uses stderr for control flow.
- **The host initiates all communication.** Extensions never send unsolicited messages.
- **Only one outstanding request per extension** at the transport level (the broker serializes via an mpsc channel), but the protocol does not mandate this — the request/response matching by `id` would support concurrency if the broker were changed.

---

## 2. Transport & Framing

### 2.1 Newline-Delimited JSON (NDJSON)

```
<JSON-RPC-request-object>\n
<JSON-RPC-response-object>\n
```

- Each JSON object is serialized on a single line without pretty-printing.
- The `\n` delimiter is the **only** framing mechanism.
- The broker uses `tokio::io::BufReader::read_line` to read from stdout and `write_all(b'\n')` after each serialized request.
- No content-length headers or chunking are used.

### 2.2 Message Size Limits

- **No explicit maximum message size is enforced at the protocol level.** The underlying Tokio buffered I/O uses an internal 8 KB buffer and reallocates as needed for lines exceeding the buffer size.
- In practice, individual JSON-RPC messages are bounded by application-level constraints (e.g., tool input size, tool output size).
- **Note:** Extremely large messages (hundreds of MB) may cause memory pressure; extensions should avoid sending oversized responses.

### 2.3 Partial Reads & Malformed JSON

- If `read_line` returns `Ok(0)` (EOF) or `Err(_)`, the broker treats this as **process exit**, drains all pending requests with `Err("Process exited")`, and kills the child process.
- If a line cannot be parsed as `JsonRpcResponse` (i.e., `serde_json::from_str` fails), the line is **silently discarded**. The broker does not send a JSON-RPC parse error back because the response cannot be matched to a request without a valid `id`.
- Malformed lines and stderr output are logged via the event bus as `ExtensionError` events, but do not affect the broker's operation beyond the immediate line.

---

## 3. JSON-RPC 2.0 Basics

### 3.1 Request

```rust
pub struct JsonRpcRequest {
    pub jsonrpc: String,    // "2.0"
    pub method: String,
    pub params: Option<serde_json::Value>,
    pub id: Option<serde_json::Value>,
}
```

- `jsonrpc` — MUST be `"2.0"`.
- `method` — A string identifying the RPC method.
- `params` — Optional. A JSON object or array providing the method's parameters.
- `id` — Optional. For extension protocol, the broker **always** includes a string `id` (UUID v4). A notification (no `id`) is not used in the current protocol.

### 3.2 Response

```rust
pub struct JsonRpcResponse {
    pub jsonrpc: String,    // "2.0"
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
    pub id: Option<serde_json::Value>,
}
```

- Exactly one of `result` or `error` MUST be present (not both, not neither).
- `id` — MUST match the `id` from the corresponding request.

### 3.3 Error Object

```rust
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}
```

- `code` — Integer error code (see §6 Error Codes).
- `message` — Human-readable error description.
- `data` — Optional additional error metadata.

### 3.4 Example Round-Trip

**Broker sends:**
```json
{"id":"a1b2c3d4-...","jsonrpc":"2.0","method":"initialize","params":{"capabilities":{"tools":true,"hooks":false,"context":false},"version":"1.0.0"}}
```

**Extension responds:**
```json
{"id":"a1b2c3d4-...","jsonrpc":"2.0","result":{"status":"ok"}}
```

---

## 4. Method Catalog

### 4.1 `initialize`

**Direction:** Broker → Extension  
**Timing:** Sent immediately after the child process is spawned and stdin/stdout pipes are established. This is the **first** RPC call the extension receives.  
**Purpose:** Handshake that communicates the extension's own manifest capabilities and version back to it, allowing the extension to validate compatibility and perform any one-time setup.

#### Request Parameters

```typescript
interface InitializeParams {
    capabilities: {
        tools: boolean;
        hooks: boolean;
        context: boolean;
    };
    version: string;  // Extension version from manifest
}
```

The broker serializes the extension's own `manifest.capabilities` and `manifest.version` directly.

#### Request Example

```json
{
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "jsonrpc": "2.0",
    "method": "initialize",
    "params": {
        "capabilities": {
            "tools": true,
            "hooks": false,
            "context": true
        },
        "version": "0.1.0"
    }
}
```

#### Expected Response (Success)

```json
{
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "jsonrpc": "2.0",
    "result": {
        "status": "ok"
    }
}
```

The `result` object should contain `"status": "ok"`. The broker does not currently inspect the response result beyond confirming it is an `Ok` (no JSON-RPC error).

#### Error Response

```json
{
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "jsonrpc": "2.0",
    "error": {
        "code": -32000,
        "message": "Incompatible extension version",
        "data": null
    }
}
```

#### Broker Behavior

- **On success:** The broker publishes `RuntimeEvent::ExtensionLoaded`.
- **On failure (error response):** The broker calls `shutdown()` (kills the child process), publishes `RuntimeEvent::ExtensionRejected`, and returns an error that prevents the extension from being registered.
- **On timeout (30s):** Same as failure — the extension is rejected.

### 4.2 `tools/call`

**Direction:** Broker → Extension  
**Purpose:** Execute a tool declared in the extension's manifest. The broker performs permission checks **before** sending the RPC.

#### Request Parameters

```typescript
interface ToolsCallParams {
    name: string;     // Tool name (matches a ToolDeclaration.name from manifest)
    input: object;    // Arbitrary JSON object matching the tool's input_schema
}
```

#### Request Example

```json
{
    "id": "b1c2d3e4-...",
    "jsonrpc": "2.0",
    "method": "tools/call",
    "params": {
        "name": "my-tool",
        "input": {
            "path": "/workspace/file.txt",
            "content": "Hello, world!"
        }
    }
}
```

#### Expected Response (Success)

```json
{
    "id": "b1c2d3e4-...",
    "jsonrpc": "2.0",
    "result": {
        "content": "Execution result text..."
    }
}
```

The broker extracts `result.content` as a string. If the `result` object has no `"content"` field, the broker falls back to `result.to_string()` (the entire JSON object serialized).

#### Error Handling

- **JSON-RPC error response** → mapped to `ToolError::ExecutionFailed` with message `"JSON-RPC Error {code}: {message}"`.
- **Timeout (30s)** → `Err("Request timed out")` → `ToolError::ExecutionFailed`.
- **Process exit** → All pending `tools/call` requests receive `Err("Process exited")`.
- **Channel failure** → `Err("Broker channel closed")` or `Err("Failed to write to stdin")`.

#### Broker-Side Pre-Checks (Before RPC)

1. **Capability gate:** If `manifest.capabilities.tools` is `false`, the tool is immediately denied with `ToolError::Denied`.
2. **Input permission scanning:** `check_input_permissions` recursively walks the `input` JSON for keys containing path-related substrings (`path`, `file`, `dir`, `dest`, `src`, `target`, `output`). Values for these keys are checked against `manifest.permissions` (workspace read/write or `allowed_paths`). Keys containing URL-related substrings (`url`, `host`, `uri`, `address`) trigger network permission checks against `manifest.permissions.allow_network`.

### 4.3 `context/inject`

**Direction:** Broker → Extension  
**Purpose:** Inject a system message into the agent's context for the current session. Called during the context-building phase, between `before_context_build` and `after_context_build` hooks.

#### Request Parameters

```typescript
interface ContextInjectParams {
    name: string;     // Context injector name (matches ContextInjectorDeclaration.name)
}
```

**Note:** Unlike `tools/call`, the `workspace_root` is not sent to the extension. It is used only on the broker side for permission checking.

#### Request Example

```json
{
    "id": "c2d3e4f5-...",
    "jsonrpc": "2.0",
    "method": "context/inject",
    "params": {
        "name": "project-scanner"
    }
}
```

#### Expected Response (Success)

```json
{
    "id": "c2d3e4f5-...",
    "jsonrpc": "2.0",
    "result": {
        "content": "Discovered 15 Rust crates, 3 workspaces..."
    }
}
```

The broker extracts `result.content` (defaulting to `""` if missing) and wraps it in a `Message::System { content }`.

#### Error Handling

- **JSON-RPC error** → `RuntimeError::Extension(err_msg)` — prevents the context contributor from contributing; the agent continues with other contributors.
- **Timeout (30s)** → Same as error — the contributor is skipped.
- The broker does **not** crash on context injector failure; it is treated as recoverable.

#### Broker-Side Pre-Checks (Before RPC)

1. **Capability gate:** If `manifest.capabilities.context` is `false`, returns `RuntimeError::Extension`.
2. **Workspace path permission:** The `workspace_root` is checked against the manifest's path permissions (read access). This prevents an extension from discovering workspace paths it does not have permission to read.

### 4.4 `hooks/call`

**Direction:** Broker → Extension  
**Purpose:** Invoke a lifecycle hook registered by the extension at a specific lifecycle point in the agent loop.

#### Request Parameters

```typescript
interface HooksCallParams {
    name: string;               // Hook name (matches HookDeclaration.name)
    lifecycle_point: LifecyclePoint;
    context: HookContext;
}

type LifecyclePoint =
    | "before_context_build"
    | "after_context_build"
    | "before_tool_policy"
    | "after_tool_result"
    | "on_event";

interface HookContext {
    // before_context_build
    session_id?: string;
    history?: Message[];

    // after_context_build
    session_id?: string;
    history?: Message[];
    packet?: ContextPacket;

    // before_tool_policy
    session_id?: string;
    tool_name?: string;
    tool_input?: object;

    // after_tool_result
    session_id?: string;
    tool_name?: string;
    result?: ToolExecutionResult;

    // on_event
    session_id?: string;
    event?: AgentEvent;
}
```

#### Context Shape by Lifecycle Point

| Lifecycle Point        | Context Fields                                              |
|------------------------|-------------------------------------------------------------|
| `before_context_build` | `session_id`, `history` (full message history)              |
| `after_context_build`  | `session_id`, `history`, `packet` (the built ContextPacket) |
| `before_tool_policy`   | `session_id`, `tool_name`, `tool_input`                     |
| `after_tool_result`    | `session_id`, `tool_name`, `result` (ToolExecutionResult)   |
| `on_event`             | `session_id`, `event` (AgentEvent)                          |

#### Request Example

```json
{
    "id": "d3e4f5a6-...",
    "jsonrpc": "2.0",
    "method": "hooks/call",
    "params": {
        "name": "audit-logger",
        "lifecycle_point": "before_tool_policy",
        "context": {
            "session_id": "sess-001",
            "tool_name": "bash",
            "tool_input": {
                "command": "rm -rf /"
            }
        }
    }
}
```

#### Response Structure

```typescript
type HookResponse =
    | { outcome: "continue" }
    | { outcome: "block"; reason: string }
    | { outcome: "add_context"; message: Message }
    | { outcome: "annotate"; metadata: object };
```

The `on_event` lifecycle point ignores the response (the broker does not process it).

#### Response Parsing Logic (`parse_hook_outcome`)

The broker parses the response result with the following precedence:

1. If the result is a **string**: `"continue"` maps to `HookOutcome::Continue`.
2. If the result is an **object** with a `"type"` field:
   - `"block"` → `HookOutcome::Block { reason }` (default reason: `"Blocked by hook"`)
   - `"add_context"` → `HookOutcome::AddContext { message }` (deserialized from the `"message"` field)
   - `"annotate"` → `HookOutcome::Annotate { metadata }`
   - Unknown type → falls through to `HookOutcome::Continue`
3. **Default:** `HookOutcome::Continue`

#### Response Examples

Continue:
```json
{
    "id": "d3e4f5a6-...",
    "jsonrpc": "2.0",
    "result": "continue"
}
```

Block:
```json
{
    "id": "d3e4f5a6-...",
    "jsonrpc": "2.0",
    "result": {
        "type": "block",
        "reason": "Command violates security policy"
    }
}
```

Add context:
```json
{
    "id": "d3e4f5a6-...",
    "jsonrpc": "2.0",
    "result": {
        "type": "add_context",
        "message": {
            "role": "system",
            "content": "Warning: workspace contains sensitive files"
        }
    }
}
```

Annotate:
```json
{
    "id": "d3e4f5a6-...",
    "jsonrpc": "2.0",
    "result": {
        "type": "annotate",
        "metadata": {
            "risk_score": 0.85,
            "policy": "deny-write"
        }
    }
}
```

#### How Each Outcome Variant Maps

| Outcome       | Behavior                                                                 |
|---------------|--------------------------------------------------------------------------|
| `continue`    | No action; processing proceeds normally.                                 |
| `block`       | Processing is interrupted. Reason is captured and surfaced as an error.  |
| `add_context` | The `message` is appended to the `patch_store` and injected into context.|
| `annotate`    | Metadata is stored (reserved for future use in current implementation).  |

#### Execution Order (ComposedCompositionHooks)

For each lifecycle point, hooks are executed in this order:

1. **User-defined hooks** (if present / registered programmatically). If the user hook returns a non-`Continue` outcome, extension hooks are **skipped** and the user hook's outcome is returned immediately.
2. **Extension hooks** — iterated in order. Each extension whose manifest contains a `HookDeclaration` with a matching `lifecycle_point` is called. The final outcome is:
   - If **any** extension returns `Block` → immediately return `Block` (short-circuit).
   - If extensions return `AddContext` or `Annotate`, the **last** non-`Continue` outcome wins (later extensions override earlier ones).
   - Continue outcomes are aggregated silently.

For `on_event`, the response is ignored entirely (`let _ = ...`).

#### Error Handling

- RPC failures (timeout, transport error, JSON-RPC error) are **silently ignored** for hooks — the broker does not propagate hook call failures to the agent. The hook is simply skipped for that lifecycle point.
- Hook errors are published as `RuntimeEvent::HookFailed` events.

---

## 5. Error Codes

### 5.1 Standard JSON-RPC Error Codes

| Code    | Message              | Description                          |
|---------|----------------------|--------------------------------------|
| -32700  | Parse error          | Invalid JSON in request/response     |
| -32600  | Invalid Request      | Malformed JSON-RPC object            |
| -32601  | Method not found     | Unknown method                       |
| -32602  | Invalid params       | Invalid method parameter(s)          |
| -32603  | Internal error       | Internal JSON-RPC error              |

### 5.2 Application-Level Error Codes

| Code    | Message                              | Description                                      |
|---------|--------------------------------------|--------------------------------------------------|
| -32000  | Extension initialization failed       | Returned by `initialize` when incompatible       |
| -32001  | Tool execution error                  | Generic tool failure                             |
| -32002  | Context injection error               | Context contributor failure                      |
| -32099  | Reserved for future standard errors   | —                                                |

Extensions may use custom error codes in the server error range (`-32000` to `-32099`) or positive codes for application-specific errors.

### 5.3 Error Propagation

All errors propagate as follows:

```
Extension returns JSON-RPC error
    → Broker formats: "JSON-RPC Error {code}: {message}"
    → Returned to the caller (tool executor, context builder, etc.)
    → Published as RpcResponse { success: false } on the event bus
```

Transport-level failures (timeout, process exit, broken pipe) produce plain string errors:

| Condition              | Error String                  |
|------------------------|-------------------------------|
| 30s timeout            | `"Request timed out"`         |
| Process exited         | `"Process exited"`            |
| Channel closed         | `"Broker channel closed"`     |
| Write failure          | `"Failed to write to stdin"`  |

---

## 6. Timeouts & Cancellation

### 6.1 Default RPC Timeout

All RPC calls have a **30-second default timeout** (`Duration::from_secs(30)`), implemented via `tokio::time::timeout` on the oneshot response channel.

### 6.2 Timeout Behavior

- When a timeout occurs, the broker returns `Err("Request timed out")` to the caller.
- The extension process is **not killed** on a single timeout. The same extension may continue to serve subsequent requests.
- If timeouts are persistent, the caller (e.g., the tool policy layer) may decide to reject the extension entirely.

### 6.3 No Cancellation Notifications

- The protocol does **not** support cancellation notifications (e.g., a `$/cancelRequest` equivalent).
- Timeout is the only mechanism for dealing with unresponsive extensions.
- The extension is expected to handle its own internal cancellation if it detects that the caller has stopped waiting.

### 6.4 Process Exit During Request

If the extension process exits (stdout returns EOF) while requests are pending:

1. The broker's I/O loop detects `read_line` returning `Ok(0)`.
2. The loop breaks and **all** pending requests receive `Err("Process exited")`.
3. The broker kills the child process (if not already exited) via `child.kill().await`.
4. The exit code is published as `RuntimeEvent::ProcessExited`.

---

## 7. Lifecycle

### 7.1 Process Spawn

```
Broker                                    Extension
  │                                          │
  ├── Shell permission check ──────────────► │
  ├── env_clear()                            │
  ├── Set safe env allowlist                 │
  ├── pipe(stdin, stdout, stderr)            │
  ├── kill_on_drop(true)                     │
  ├── child.spawn() ─────────────────────►   │
  │                                          │ (process starts)
  ├── Take stdin/stdout/stderr handles       │
  ├── Spawn stderr drainer task              │
  ├── Spawn stdin/stdout I/O loop task       │
  ├── Send initialize RPC ───────────────►   │
  │                                          ├── Parse request
  │                                          ├── Perform setup
  │◄── Receive response ──────────────────── │
  │                                          │
  ├── [if error] shutdown + reject           │
  ├── [if success] ExtensionLoaded event     │
  │                                          │
  ▼                                          ▼
     READY                               READY
```

### 7.2 Environment Setup

The broker calls `env_clear()` on the child `Command`, then selectively inherits only these safe environment variables:

```
PATH, HOME, USER, LOGNAME, SHELL, TERM, LANG,
LC_ALL, LC_CTYPE, TMPDIR, TEMP, TMP
```

All other environment variables from the parent process are **excluded**. This prevents accidental leakage of sensitive environment variables (API keys, tokens, etc.) to extension child processes.

### 7.3 Shell Permission Check

Before spawning, the broker checks the manifest's `permissions.allow_shell` flag:

- If `allow_shell` is `false`, the entrypoint command must not contain shell metacharacters (space, `|`, `&`, `;`, `>`, `<`) and must not be a known shell executable (`sh`, `bash`, `zsh`, `ksh`, `csh`, `tcsh`, `cmd`, `powershell`, `pwsh`, `fish`).
- If the check fails, the extension is rejected with `ExtensionRejected` before any process is spawned.

### 7.4 I/O Loop (Stdin/Stdout)

The broker spawns an asynchronous task (`tokio::spawn`) that runs a `tokio::select!` loop with two branches:

1. **Request channel** (`rx.recv`): Receives outgoing requests from the mpsc channel. Each request is serialized, appended with `\n`, and written to stdin. Pending responses are tracked in a `HashMap<String, oneshot::Sender>` keyed by request ID.
2. **Stdout reader** (`read_line`): Reads one line from stdout, deserializes it as `JsonRpcResponse`, matches by response `id`, and sends the result to the corresponding oneshot sender.

When the channel closes or stdout returns EOF, the loop:
- Drains all pending requests with `Err("Process exited")`.
- Kills the child process.
- Publishes `RuntimeEvent::ProcessExited` with the exit code.

### 7.5 Stderr Draining

A separate task reads stderr line by line via `BufReader`. Each non-empty line is published as `RuntimeEvent::ExtensionError { message: "stderr: <line>" }`. This is purely observational; stderr content does not affect broker control flow.

### 7.6 Shutdown

`ProcessExtensionBroker::shutdown()`:

1. Takes ownership of the `Child` handle from the `Arc<Mutex<Option<Child>>>`.
2. Calls `child.kill().await` (sends `SIGTERM`/`SIGKILL`).
3. Calls `child.wait().await` to reap the process.
4. Publishes `RuntimeEvent::ProcessExited` with the exit code.

If the child has already exited, `kill()` is a no-op.

The `kill_on_drop(true)` setting on the `Command` ensures that if the `Child` handle is dropped without explicit shutdown (e.g., during panic or drop), the OS will terminate the child process.

---

## 8. Security Considerations

### 8.1 Host-Side Permission Enforcement

All permission checks happen **before** the RPC call is sent to the extension. The extension never decides whether an operation is permitted; the host's manifest permissions are the sole authority.

- **Tools:** `check_input_permissions` recursively scans all input parameter values for path-like and network-like keys. Each detected path is validated against workspace read/write permissions and `allowed_paths`. Each detected URL/host is validated against `allow_network`.
- **Context:** Workspace root is checked against path permissions (read access) before `context/inject` is called.
- **Hooks:** No input permission scanning is performed for hooks (hook context is read-only by design).

### 8.2 Input Argument Scanning

The broker heuristically identifies path and network parameters by keyword matching on JSON object keys (case-insensitive):

- **Path keys:** `path`, `file`, `dir`, `dest`, `src`, `target`, `output`
- **Write detection:** Keys containing `write`, `dest`, `output`, or `target` imply write intent.
- **Network keys:** `url`, `host`, `uri`, `address`

This scanning is recursive — nested objects and arrays are traversed.

### 8.3 Environment Isolation

- `env_clear()` on the child `Command` removes all inherited environment variables.
- Only a curated allowlist of safe, non-sensitive variables is reinstated (see §7.2).
- This prevents credential leakage (e.g., `AWS_ACCESS_KEY_ID`, `GITHUB_TOKEN`, `DATABASE_URL` from the parent process to the extension).

### 8.4 Stderr Isolation

- Stderr is read asynchronously and used **only** for logging and observability.
- The broker never reads control signals or responses from stderr.
- An extension sending a valid JSON-RPC response to stderr will be ignored (it will appear as an `ExtensionError` log line).

### 8.5 Extensions Cannot Bypass Host Checks

- Every `tools/call` RPC is preceded by the host's input permission scan (§8.2).
- If the extension's manifest lacks the `tools` or `context` capability flag, the host never sends the corresponding RPC.
- The extension process runs with the same OS-level privileges as the host, but all host-gated checks (filesystem, network, shell) are applied before delegation.

### 8.6 Shell Metacharacter Restriction

When `allow_shell` is `false`:
- The command must be a direct executable path with no shell interpretation.
- The command and its arguments are passed via `Command::arg()` not `Command::arg("sh -c ...")`, so no shell expansion occurs.
- This prevents shell injection through argument values.

### 8.7 Tool Trust Annotations and Harness Allow-List

Each `ToolDeclaration` may carry two optional annotations:

```toml
[[tools]]
name = "read_only_safe_op"
read_only = true
idempotent = true
```

These annotations affect the tool descriptor's `AnnotationSource`:

- **No annotations** — descriptor is tagged `AnnotationSource::ExtensionDeclared`. The trust tier is whatever the extension's manifest already says (default: `Untrusted`).
- **Annotations present** — descriptor is still `ExtensionDeclared`. The annotations are recorded on the descriptor so the executor can reason about them, but they are not treated as a self-attestation of trust.
- **Annotations present AND extension ID is in the harness allow-list** — descriptor is promoted to `AnnotationSource::BuiltInTrusted` and a `RetryPolicy` (`max_retries: 1, backoff_ms: 200`) is attached. This is the **only** path through which an extension tool receives a retry policy.

The harness-side allow-list is configured at runtime:

```rust
use gestalt_runtime::extension_trust;
extension_trust::set_trusted_extension_ids(vec!["acme.fs".into(), "noentic.cache".into()]);
```

Extensions not on the list (including all `*-local` and explicitly-loaded ones unless they appear here) get the conservative path: no retry policy, no `BuiltInTrusted` promotion, and any retry gating falls back to the transient-failure rule.

This avoids the "manifest says it's safe, harness believes the manifest" footgun: trust is a host-side decision that mirrors the way a real package manager's GPG signature would.

---

## 9. Compatibility & Versioning

### 9.1 Version Field

- The extension's `version` field (from the manifest TOML) is sent in the `initialize` request parameters.
- Future versions of the broker may use this to enable or disable protocol features based on version negotiation.

### 9.2 Version Mismatch Handling

- Currently, the broker does **not** enforce or check the extension version — it simply passes the manifest's `version` string to the extension in `initialize`.
- The extension SHOULD return an error in its `initialize` response if it detects an incompatible broker version or protocol version.
- The extension MAY inspect the `version` and `capabilities` fields in the `initialize` params to determine if it can operate.

### 9.3 Method-Level Compatibility

- Unknown methods: The extension should respond with a JSON-RPC `Method not found` error (`-32601`) for any method it does not implement.
- Unknown lifecycle points in `hooks/call`: The extension should treat unknown lifecycle points as a no-op (return `"continue"`) or return an error.
- Extra fields in params: Extensions MUST ignore unknown fields in params for forward compatibility.
- Missing optional fields: Extensions SHOULD have sensible defaults for missing fields.

### 9.4 Backward Compatibility Guarantees

- **New methods** may be added in future protocol versions. Extensions that return `Method not found` for unrecognized methods will continue to work.
- **New lifecycle points** may be added. Extensions must either return `"continue"` or an error for unrecognized lifecycle points.
- **New fields** in existing params or response objects will use `#[serde(default)]` semantics — extensions MUST ignore unknown fields.
- **Existing methods and lifecycle points** will not be removed or renamed without a major version bump.
- **Response shape changes** (e.g., `tools/call` returning additional fields in the `result` object) will use optional fields.

---

## Appendix A: Extension Manifest (TOML Schema)

The manifest is parsed from a TOML file and informs the broker what capabilities to advertise and what methods to call.

```rust
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: String,          // "stdio"
    pub entrypoint: Entrypoint,
    pub capabilities: Capabilities,
    pub permissions: Permissions,
    pub tools: Vec<ToolDeclaration>,
    pub hooks: Vec<HookDeclaration>,
    pub context_injectors: Vec<ContextInjectorDeclaration>,
}
```

Fields relevant to the protocol:

- `runtime` — Must be `"stdio"` (the only supported transport in MVP).
- `capabilities` — Booleans for `tools`, `hooks`, `context`. These are sent in `initialize`. No RPC calls are made for disabled capabilities.
- `hooks` — A list of `{ name, lifecycle_point }` pairs. Each `lifecycle_point` must match one of the five supported values.

## Appendix B: Example Extension (Shell)

The canonical mock extension (bash) demonstrates the minimal protocol implementation:

```bash
#!/bin/bash
while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then
    req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2)
  fi

  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)

  if [ "$method" = "initialize" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"status\":\"ok\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "tools/call" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"content\":\"tool output\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "context/inject" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"content\":\"injected context\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "hooks/call" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"type\":\"block\",\"reason\":\"blocked\"},\"id\":\"$req_id\"}"
  fi
done
```
