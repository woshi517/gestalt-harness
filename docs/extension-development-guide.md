## Extension Development Guide

**Status:** Living Document  
**Applies to:** gestalt-harness v0.2+  
**Primary Model:** V2 Package & Components  
**Runtime protocol:** Lifecycle Protocol V2 / MCP / Stdio  

---

## 1. Introduction

Gestalt extensions are structured packages containing one or more typed runtime components. The runtime activates these components as part of an immutable snapshot generation, coordinating lifecycles, configuration, and security permissions.

Extensions can contribute the following kinds of functionality:

- **Command Tools** — executable binaries or scripts wrapped as tools.
- **MCP Server Components** — Model Context Protocol servers run over stdio transport.
- **Lifecycle Components** — custom code implementing the Lifecycle Protocol V2 to hook into agent phases, turn routing, context assembly, policy guards, or verifiers.
- **Client/Product Descriptors** — metadata/inventory for visual, SDK, or GUI hosts (not executed directly by the runtime core).
- **Skills** — agent instructions and patterns.

These components are configured as package instances and verified using a cryptographic trust/grants model.

Stable v0.1 and later require extension manifest version 2 and Lifecycle
Protocol V2. Manifest/protocol V1 is unsupported and is not adapted or migrated
by the runtime.

---

## 2. Getting Started with V2 Packages

The Gestalt V2 architecture is component-centric:

```
┌────────────────────────────────────────────────────────┐
│                      Runtime Host                      │
│  ┌─────────────────────────┐   ┌────────────────────┐  │
│  │   AgentRuntime (Run)    │   │  ExtensionManager  │  │
│  └────────────┬────────────┘   └─────────┬──────────┘  │
│               │                          │             │
│  adopts lease │                          │ spawns &    │
│               ▼                          ▼ manages     │
│  ┌──────────────────────────────────────────────────┐  │
│  │             RuntimeExtensionSnapshot             │  │
│  │  ┌────────────────────────────────────────────┐  │  │
│  │  │            Configured Instances            │  │  │
│  │  │  ┌──────────────┐ ┌────────────┐ ┌──────┐  │  │  │
│  │  │  │ Command Tool │ │ MCP Server │ │ ...  │  │  │  │
│  │  │  └──────────────┘ └────────────┘ └──────┘  │  │  │
│  │  └────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────┘
```

1. You declare a package manifest `gestalt.extension.toml` with `manifest_version = 2`.
2. You declare one or more typed `[[components]]` (e.g., `command-tool`, `mcp-server`, `gestalt-lifecycle`).
3. You configure instances of these components under host configuration.
4. The runtime manages generation leases, sandboxing policies, and host-level network grants for each active component instance.

---

## 3. The V2 Extension Manifest

Every V2 extension package must contain a `gestalt.extension.toml` file at its root.

### 3.1 Package Metadata

```toml
manifest_version = 2              # Required. Must be 2 for the package model.

[package]
id = "com.example.review"         # Required. Unique identifier.
name = "Review Extensions"         # Required. Human-readable name.
version = "1.0.0"                 # Required. Semver string.
description = "V2 review tools"   # Optional. Description.
```

### 3.2 Components

A package contains a list of component declarations. Each component must declare an `id`, `kind`, and its `entrypoint`:

```toml
[[components]]
id = "reviewer"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "review_ext"]
```

#### Component Kinds:
- `gestalt-lifecycle` — Implements the Lifecycle Protocol V2.
- `command-tool` — A script/binary executed directly to fulfill a single tool.
- `mcp-server` — An external MCP server communicating via stdio JSON-RPC.

### 3.3 Sandboxing & Permissions

Component execution is governed by a permissions schema:

```toml
[permissions]
allow_network = ["api.github.com"] # List of allowed hostnames. "*" means any host.
allow_workspace_read = true        # Allow reading files inside the workspace root.
allow_workspace_write = false      # Allow writing files inside the workspace root.
allow_shell = false                # Allow shell-interpreted commands.
allow_all_paths = false            # Bypass path verification checks (dangerous).
allowed_paths = []                 # Additional paths outside the workspace that are allowed.
```

Host-level MCP network policies are applied to outgoing connections.

### 3.4 Unsupported Manifest Versions

Manifests without `manifest_version = 2`, including pre-hardening V1 manifests,
are rejected before activation. Gestalt does not provide a V1 compatibility
loader.

----empty, `capabilities.context` must be `true`
- If `allow_shell` is `false`, the entrypoint command must not contain shell metacharacters or be a shell executable

### 3.9 Complete Worked Example

```toml
id = "doc-search"
name = "Documentation Search"
version = "1.2.0"
runtime = "stdio"

[entrypoint]
command = "python3"
args = ["-m", "doc_search_extension"]

[capabilities]
tools = true
hooks = false
context = false

[permissions]
allow_network = ["api.openai.com"]
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = ["/usr/share/doc"]

[[tools]]
name = "search"
description = "Search documentation index"
input_schema = { type = "object", properties = { query = { type = "string" } }, required = ["query"] }
risk = "low"

[[tools]]
name = "index_status"
description = "Check if the documentation index is up to date"
input_schema = { type = "object" }
risk = "low"
```

---

## 4. The JSON-RPC Protocol

Extensions communicate via **newline-delimited JSON-RPC 2.0**. Each request or response is a single JSON object on one line, terminated by `\n`. The host writes requests to the child's stdin and reads responses from its stdout.

### 4.1 Wire Format Types

From `crates/gestalt-runtime/src/jsonrpc.rs`:

```rust
pub struct JsonRpcRequest {
    pub jsonrpc: String,          // always "2.0"
    pub method: String,
    pub params: Option<Value>,    // optional params object
    pub id: Option<Value>,        // string or integer ID; omitted for notifications
}

pub struct JsonRpcResponse {
    pub jsonrpc: String,          // always "2.0"
    pub result: Option<Value>,    // present on success
    pub error: Option<JsonRpcError>,  // present on error
    pub id: Option<Value>,        // mirrors the request ID
}

pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,      // optional additional error data
}
```

### 4.2 Initialize Handshake

Immediately after spawning the child process, the runtime sends an `initialize` request:

**Request:**
```json
{"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{"tools":true,"hooks":false,"context":true},"version":"0.1.0"},"id":"init-1"}
```

**Expected response:**
```json
{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"init-1"}
```

The extension **must** respond to `initialize` before it can receive any other requests. If the extension fails to respond within 30 seconds or returns an error, the runtime kills the process and marks the extension as rejected.

The `params.capabilities` mirror the manifest's `[capabilities]` section so the extension knows which features are enabled. The `result.capabilities` is reserved for future capability negotiation.

### 4.3 `tools/call` Method

Dispatched when the agent invokes a tool declared in the manifest.

**Request:**
```json
{"jsonrpc":"2.0","method":"tools/call","params":{"name":"search","input":{"query":"rust async"}},"id":"uuid-abc-123"}
```

**Successful response:**
```json
{"jsonrpc":"2.0","result":{"content":"Found 3 results..."},"id":"uuid-abc-123"}
```

**Error response:**
```json
{"jsonrpc":"2.0","error":{"code":-32000,"message":"Index not found","data":null},"id":"uuid-abc-123"}
```

The `result` object must contain a `content` string field, which becomes the tool's text output. If no `content` field is present, the entire result object is stringified.

### 4.4 `context/inject` Method

Dispatched before each agent turn to gather context contributions.

**Request:**
```json
{"jsonrpc":"2.0","method":"context/inject","params":{"name":"current_weather"},"id":"uuid-456"}
```

**Successful response:**
```json
{"jsonrpc":"2.0","result":{"content":"Current weather: Sunny, 72°F"},"id":"uuid-456"}
```

The `content` field is used as a system message injected into the context window.

### 4.5 `lifecycle/invoke` Method

Dispatched when a typed lifecycle capability fires. The params and result payloads are capability-specific and are wrapped in versioned DTOs.

**Request:**
```json
{"jsonrpc":"2.0","method":"lifecycle/invoke","params":{"component_id":"component:com.example.lifecycle:primary:lifecycle","capability":"context_provider","payload":{"session_id":"...","history":[]}},"id":"uuid-789"}
```

**Response:**
```json
{"jsonrpc":"2.0","result":{"payload":{"content":"Extra context"}},"id":"uuid-789"}
```

### 4.6 Error Codes

Custom error codes use the range `-32000` to `-32099` (reserved by JSON-RPC for server errors). There are no predefined error codes in MVP; extensions may define their own. Standard JSON-RPC error codes:

| Code | Meaning |
|---|---|
| `-32700` | Parse error (invalid JSON) |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |
| `-32000` to `-32099` | Server/extension errors |

### 4.7 Timeout Semantics

Every RPC call has a **30-second timeout** (`Duration::from_secs(30)` in `ProcessExtensionBroker::call`). If the extension does not write a response within 30 seconds, the runtime returns a "Request timed out" error to the caller. The extension process remains running and can handle subsequent requests.

### 4.8 Message Framing

- Each JSON message is a single line terminated by `\n` (newline).
- The runtime writes the JSON bytes followed by `\n` to stdin.
- The runtime reads lines from stdout using `BufReader::read_line`.
- JSON values with embedded newlines are **not supported** — the `input_schema` and all param/result values should avoid multiline strings, or the extension must encode them (e.g., `\n` escapes).

---

## 5. Writing an Extension

### 5.1 In Python

```python
#!/usr/bin/env python3
"""Minimal Gestalt stdio extension in Python."""
import sys
import json

def respond(request_id, result=None, error=None):
    response = {"jsonrpc": "2.0", "id": request_id}
    if error:
        response["error"] = error
    else:
        response["result"] = result
    sys.stdout.write(json.dumps(response) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except json.JSONDecodeError:
        continue

    method = req.get("method", "")
    req_id = req.get("id")
    params = req.get("params", {}) or {}

    if method == "initialize":
        respond(req_id, {"capabilities": {}})

    elif method == "tools/call":
        tool_name = params.get("name", "")
        tool_input = params.get("input", {})

        if tool_name == "search":
            query = tool_input.get("query", "")
            result = {"content": f"Searched for: {query}"}
            respond(req_id, result)
        else:
            respond(req_id, None,
                    error={"code": -32601, "message": f"Unknown tool: {tool_name}"})

    elif method == "context/inject":
        injector_name = params.get("name", "")
        respond(req_id, {"content": f"Injected from {injector_name}"})

    elif method == "lifecycle/invoke":
        respond(req_id, {"payload": {"content": f"Lifecycle payload for {params.get('component_id', '')}"}})

    else:
        respond(req_id, None,
                error={"code": -32601, "message": f"Unknown method: {method}"})
```

### 5.2 In Node.js / TypeScript

```javascript
#!/usr/bin/env node
const readline = require("readline");

const rl = readline.createInterface({ input: process.stdin });

function respond(id, result, error) {
  const msg = { jsonrpc: "2.0", id };
  if (error) msg.error = error;
  else msg.result = result;
  process.stdout.write(JSON.stringify(msg) + "\n");
}

rl.on("line", (line) => {
  let req;
  try { req = JSON.parse(line); }
  catch { return; }

  const { method, params = {}, id } = req;

  switch (method) {
    case "initialize":
      respond(id, { capabilities: {} });
      break;

    case "tools/call":
      const { name, input } = params;
      if (name === "search") {
        respond(id, { content: `Node.js search: ${input.query}` });
      } else {
        respond(id, null, { code: -32601, message: `Unknown tool: ${name}` });
      }
      break;

    case "context/inject":
      respond(id, { content: `Node.js context from ${params.name}` });
      break;

    case "lifecycle/invoke":
      respond(id, { payload: { content: `Lifecycle payload for ${params.component_id}` } });
      break;

    default:
      respond(id, null, { code: -32601, message: `Unknown: ${method}` });
  }
});
```

### 5.3 In Rust

```rust
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn respond(id: &Option<Value>, result: Option<Value>, error: Option<Value>) {
    let mut resp = json!({"jsonrpc": "2.0", "id": id});
    if let Some(e) = error {
        resp["error"] = e;
    } else if let Some(r) = result {
        resp["result"] = r;
    }
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    writeln!(handle, "{}", resp).ok();
    handle.flush().ok();
}

fn main() {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = req["method"].as_str().unwrap_or("");
        let id = req.get("id").cloned();
        let params = req.get("params").and_then(|p| p.as_object()).cloned().unwrap_or_default();

        match method {
            "initialize" => {
                respond(&id, Some(json!({"capabilities": {}})), None);
            }
            "tools/call" => {
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let tool_input = params.get("input").cloned().unwrap_or(json!({}));
                if tool_name == "search" {
                    let query = tool_input.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    respond(&id, Some(json!({"content": format!("Rust search: {}", query)})), None);
                } else {
                    respond(&id, None, Some(json!({"code": -32601, "message": format!("Unknown tool: {}", tool_name)})));
                }
            }
            "context/inject" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                respond(&id, Some(json!({"content": format!("Rust context: {}", name)})), None);
            }
            "lifecycle/invoke" => {
                respond(&id, Some(json!({"payload": {"content": format!("Lifecycle payload for {}", params.get("component_id").and_then(|v| v.as_str()).unwrap_or(""))}})), None);
            }
            _ => {
                respond(&id, None, Some(json!({"code": -32601, "message": format!("Unknown method: {}", method)})));
            }
        }
    }
}
```

Add to `Cargo.toml`:
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### 5.4 The Contract (Any Language)

There are only three rules:

1. **Read one JSON object per line** from stdin (UTF-8).
2. **Write one JSON object per line** to stdout (UTF-8), ending with `\n`. Flush after every write.
3. **Respond to every request** (matching id) — if you don't, the runtime times out after 30s.

Everything else (argument parsing, tool routing, error handling) is up to you. There are no SDK requirements, no HTTP servers, no dependency constraints.

---

## 6. Permissions & Sandboxing

The runtime enforces permissions on the **host side** — before arguments reach your extension's process. This means an extension cannot bypass its permission declaration even if its code misbehaves.

### 6.1 Filesystem Access

Path permission checking is implemented in `crates/gestalt-runtime/src/permissions.rs` (`check_path_permission_impl`):

1. If `allow_all_paths` is `true`, all paths pass.
2. If the path is inside the **workspace root** and it's a **read**: pass if `allow_workspace_read` is `true`.
3. If the path is inside the workspace root and it's a **write**: pass if `allow_workspace_write` is `true`.
4. If the path is outside the workspace root, check `allowed_paths` — the path must start with one of the allowed prefixes.
5. All paths are canonicalized (`fs::canonicalize`) to prevent `../` traversal attacks.

Paths inside the workspace root that exceed workspace permissions also fall through to the `allowed_paths` check.

### 6.2 Input Argument Scanning

Before a tool call is dispatched, the runtime scans the tool call's `input` JSON for fields whose keys match sensitive patterns (`path`, `file`, `dir`, `dest`, `src`, `target`, `output`, `url`, `host`, `uri`, `address`). These are checked for:

- **Path-like fields** → `check_path_permission` (written to if the key also contains `write`, `dest`, `output`, or `target`)
- **URL/host fields** → `check_network_permission`

This scanning is recursive — it descends into nested objects and arrays. The logic lives in `check_input_permissions` in `process_extension.rs:308`.

### 6.3 Network Access

```rust
pub fn check_network_permission(
    manifest: &ExtensionManifest,
    host: &str,
    event_bus: &RuntimeEventBus,
) -> Result<(), String>
```

Matches the host against `manifest.permissions.allow_network`. Each entry is compared verbatim, or `"*"` matches any host. URL fields are parsed and the hostname is extracted for comparison.

### 6.4 Shell Command Restrictions

```rust
pub fn check_shell_permission(
    manifest: &ExtensionManifest,
    event_bus: &RuntimeEventBus,
) -> Result<(), String>
```

If `allow_shell` is `false`, the entrypoint command is validated at manifest load time: it must not contain shell metacharacters and must not be a known shell. Additionally, `ProcessExtensionBroker::spawn` re-checks before spawning.

### 6.5 Environment Isolation

The runtime clears the environment before spawning the child process (`cmd.env_clear()`), then selectively inherits only safe variables:

```rust
let safe_envs = [
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "LANG",
    "LC_ALL", "LC_CTYPE", "TMPDIR", "TEMP", "TMP"
];
```

All other environment variables are stripped. This prevents credential leakage and provides a predictable execution environment.

### 6.6 Permission Events

Every permission check publishes a `RuntimeEvent::PermissionDecision` to the event bus, which includes:
- `extension_id`, `capability` (filesystem/network/shell), `permission` (read/write/connect/execute)
- `resource` (the path or host), `granted` (bool), `reason` (error message if denied)

This enables observability and audit logging.

---

## 7. Extension Lifecycle

### 7.1 Discovery

The runtime discovers extensions in three locations, in priority order (`crates/gestalt-runtime/src/discovery.rs`):

1. **Explicit paths** — paths provided via the `[extensions]` config section or `--load-extension` CLI flag. These are always enabled.
2. **Project-local** — `.gestalt/extensions/<name>/gestalt.extension.toml` in the project root.
3. **Global** — `<config-dir>/extensions/<name>/gestalt.extension.toml` in the user's global config directory.

Within each directory, each extension lives in its own subdirectory containing a `gestalt.extension.toml` manifest file. The directory can also contain any other files the extension needs (scripts, data, binaries).

Extensions with duplicate IDs are deduplicated: the first instance wins.

### 7.2 Loading

1. Manifest is parsed and validated (`ExtensionManifest::parse` + `validate`).
2. `ProcessExtensionBroker::spawn` is called:
   - Shell permissions are checked.
   - The child process is spawned with piped stdin/stdout/stderr, environment cleared, safe vars injected.
   - A stderr drainer task is spawned (reads stderr, publishes each line as `RuntimeEvent::ExtensionError`).
   - An RPC loop task is spawned (reads requests from channel, writes to stdin, reads responses from stdout).
   - The `initialize` handshake is sent. If it fails, the process is killed and the extension is rejected.
3. If the handshake succeeds, the extension's tools, context injectors, and hooks are registered with the `RuntimeRegistry`.

### 7.3 Operation

During normal operation, the broker maintains an internal pending-requests map keyed by request ID. When a response line arrives from stdout, the matching request's oneshot channel is resolved. This allows concurrent requests (though MVP extensions typically process sequentially).

### 7.4 Shutdown

`ProcessExtensionBroker::shutdown`:
1. Acquires the child handle from the mutex.
2. Kills the child process (`child.kill().await`).
3. Waits for the exit status.
4. Publishes `RuntimeEvent::ProcessExited` with the extension ID and exit code.

The RPC loop task also auto-cleans up when the stdin channel closes or stdout reaches EOF — it drains all pending requests with `"Process exited"` errors, then kills the child.

### 7.5 Error Recovery

| Failure Mode | Behavior |
|---|---|
| Process crashes | stdout EOF is detected, pending requests fail with "Process exited", the process is killed if still alive |
| Request timeout (30s) | The specific request fails with "Request timed out", the process remains running for future requests |
| Manifest parse error | The extension is rejected with a `RuntimeEvent::ExtensionRejected` event |
| Initialize handshake fails | The process is killed, extension rejected, event published |

The runtime does **not** auto-restart crashed extensions in MVP.

---

## 8. Configuration & Trust

### 8.1 `gestalt.json` `extensions` Section

From `crates/gestalt-cli/src/config.rs`:

```json
"extensions": {
  "explicit_loads": ["path/to/extension"],
  "disabled": ["some-ext"],
  "trusted": ["trusted-ext"],
  "allow_untrusted": true
}
```

- `explicit_loads` — load extensions from specific paths (can be a directory containing `gestalt.extension.toml`, or directly the manifest file)
- `disabled` — explicitly disable discovered extensions by ID
- `trusted` — mark specific extensions as trusted (bypasses approval prompt)
- `allow_untrusted` — global switch; if `false`, only explicitly trusted extensions are loaded

### 8.2 CLI Commands

| Command | Description |
|---|---|
| `gestalt extension list` | List all discovered extensions with their status (enabled/disabled) |
| `gestalt extension enable <id>` | Enable a disabled extension by ID |
| `gestalt extension disable <id>` | Disable an enabled extension by ID |
| `gestalt extension inspect <id>` | Show the full manifest of an extension by ID |
| `gestalt extension reload` | Re-discover and reload all extensions |
| `gestalt extension validate <path>` | Parse and validate a manifest file without loading it |

### 8.3 Trust Model

- Discovered extensions require explicit trust verification. Trust is determined by matching the extension ID or integrity hash against the `extensions.trusted` list.
- If an extension is not trusted, it is rejected unless `extensions.allow_untrusted` is set to `true`.
- Untrusted extensions loaded when `allow_untrusted` is `true` run as untrusted and do not receive elevated capabilities.
- The `disabled` list allows users to suppress extensions they do not want without removing the files.

---

## 9. Debugging & Testing

### 9.1 stderr Output

Any text your extension writes to stderr is automatically drained by the runtime and published as `RuntimeEvent::ExtensionError` events. This is the recommended channel for logging, debug output, and diagnostic information. There is no size limit on stderr — it is read line-by-line and forwarded to the event bus.

In Python:
```python
import sys
print("debug: searching index...", file=sys.stderr, flush=True)
```

In Node.js:
```javascript
console.error("debug: searching index...");
```

### 9.2 Testing with the `validate` Command

Before deploying an extension, validate the manifest:

```bash
gestalt extension validate path/to/my-ext/gestalt.extension.toml
```

This parses the TOML, checks field validity, permission constraints, and capability consistency without spawning any process.

### 9.3 Using RuntimeEventBus for Observability

The runtime publishes events for every significant lifecycle moment. Subscribe to the event bus in tests or observers:

```
RuntimeEvent::ExtensionDiscovered    → extension found on disk
RuntimeEvent::ExtensionLoaded        → initialized successfully
RuntimeEvent::ExtensionRejected      → failed to load
RuntimeEvent::ExtensionError         → stderr line from the process
RuntimeEvent::ProcessSpawned         → process PID
RuntimeEvent::ProcessExited          → process exit code
RuntimeEvent::RpcRequest             → outgoing RPC call
RuntimeEvent::RpcResponse            → incoming RPC response
RuntimeEvent::PermissionDecision     → permission check result
```

In tests, you can subscribe and inspect:

```rust
let event_bus = RuntimeEventBus::new();
let mut sub = event_bus.subscribe();
// ... run extension ...
while let Ok(evt) = sub.try_recv() {
    match &*evt {
        RuntimeEvent::ExtensionError { message, .. } => {
            println!("stderr: {}", message);
        }
        _ => {}
    }
}
```

### 9.4 Common Pitfalls

- **Missing newlines:** Every response must end with `\n`. Use `println!` / `write!` with `\n` / `sys.stdout.write(msg + "\n")`.
- **Not flushing stdout:** The runtime reads from stdout with line-buffered I/O, but some languages (especially Python with `print()` when piped) may buffer. Always flush after each response.
- **Stalling on initialize:** The runtime blocks on the initialize handshake. If your extension doesn't respond, it will be killed after 30 seconds.
- **Unhandled methods:** Only recognized methods are `initialize`, `tools/call`, `context/inject`, and `lifecycle/invoke`. Unknown methods should return error code `-32601`.
- **Environment assumptions:** The environment is cleared to safe vars only. Don't assume `API_KEY` or other secrets are inherited — use a dedicated config file instead.
- **Path traversal in tool input:** The host-side input scanner checks tool call arguments for path and URL fields. If your extension receives path arguments, declare the appropriate permissions in the manifest.

### 9.5 Running Extension Tests

The test suite demonstrates the full lifecycle (`crates/gestalt-runtime/tests/runtime_process_extension_tests.rs`):

1. Parse manifest → `ExtensionManifest::parse(&content)`
2. Spawn broker → `ProcessExtensionBroker::spawn(manifest, event_bus).await`
3. Register → `ProcessExtension::new(manifest, broker).register(&mut registry)`
4. Execute tool → `tool.execute(input, &ctx).await`
5. Invoke context → `contributor.contribute(&workspace_root).await`
6. Dispatch hooks → `composed.before_context_build(&ctx).await`
7. Shutdown → `broker.shutdown().await`

Use these patterns in your own integration tests to verify extension behavior end-to-end.

---

## 10. Appendix: The mock-ext Fixture

The canonical reference extension used in the test suite demonstrates all protocol features:

### Manifest (`gestalt.extension.toml`)

```
id = "mock-ext"
name = "Mock Stdio Extension"
version = "0.1.0"
runtime = "stdio"

[entrypoint]
command = "/path/to/mock_ext.sh"

[capabilities]
tools = true
hooks = false
context = true

[permissions]
allow_network = []
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = []

[[tools]]
name = "bash_tool"
description = "Brings hello from bash"
input_schema = { type = "object" }

[[context_injectors]]
name = "bash_context"
```

### Extension Script (`mock_ext.sh`)

```bash
#!/bin/bash
# Mock JSON-RPC stdio extension
while read -r line; do
  # Extract JSON-RPC ID (robust to both string/number IDs)
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then
    req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2)
  fi

  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)

  if [ "$method" = "initialize" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"capabilities\":{}},\"id\":\"$req_id\"}"
  elif [ "$method" = "tools/call" ]; then
    val_secret=${TEST_SECRET:-unset}
    val_path=${PATH:-unset}
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"content\":\"TEST_SECRET=$val_secret PATH=$val_path\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "context/inject" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"content\":\"injected context\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "lifecycle/invoke" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"payload\":{\"content\":\"blocked by mock extension capability\"}},\"id\":\"$req_id\"}"
  fi
done
```

This fixture validates:
- Environment isolation (`TEST_SECRET=unset` confirms the env was cleared; `PATH=` shows safe vars are inherited)
- Basic JSON-RPC handshake and dispatch
- Tool execution with text output
- Context injection returning system-message content
- Lifecycle capability dispatch returning a typed payload
