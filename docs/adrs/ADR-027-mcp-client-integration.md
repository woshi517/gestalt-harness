# ADR-027: Model Context Protocol (MCP) Client Integration

**Status:** Accepted

## Context

Integrating external tool providers is critical to the extensibility of `gestalt-harness`. The Model Context Protocol (MCP) provides a standardized way to configure, discover, and execute external tools over standard stdio JSON-RPC. 

However, integrating MCP hosts at scale poses several design challenges:
1. **Performance and Context Bloat:** Registering hundreds of tools from multiple MCP servers directly to the LLM context window increases token usage and latency. We need a way to dynamically search and register tools (progressive discovery).
2. **Server Lifecycle Management:** Starting every configured MCP server eagerly on agent runtime startup incurs high startup cost, connects servers that might never be queried, and surfaces connection errors when prompts do not use MCP. A lazy connection model is required.
3. **Identity & Collision Scoping:** If two different MCP servers expose a tool with the same name (e.g., `search`), the system must prevent namespace collisions and ensure inspecting/selecting one does not leak or expose the other.
4. **Security and Risk Defaults:** Server-provided tool schemas must not be allowed to self-declare as low-risk (e.g. claiming to be read-only) to downgrade validation policies. The host must retain control of risk classification, using a secure fallback model of `Medium` default risk and restricting downgrades to `Low` risk to explicitly trusted host-side annotations.
5. **Observability & Eventing**: Host integrations must publish lifecycle and execution events to the event bus for telemetry and tracking.

## Decision

We chose to implement the MCP client integration via a dedicated `gestalt-mcp` crate and runtime orchestration integration in `gestalt-runtime`.

### 1. Lazy Server Lifecycle & Concurrency Guarding

- MCP servers default to `McpLifecycleMode::Lazy` (rather than `AlwaysOn`). They start disconnected, and the harness only initiates standard stdio handshakes when `get_client()` is requested.
- Prompts (e.g. `run_prompt`) do not eagerly query the full tool catalog, preventing prompt execution from eagerly connecting lazy servers.
- `McpRegistry` manages client pooling using `Arc<tokio::sync::OnceCell<Result<Arc<McpClient>, McpError>>>` per configured server. This guarantees that concurrent first-use calls to get a client resolve safely and never spawn duplicate client processes.

### 2. MCP Identity & Namespace Scoping

- All MCP tools are represented internally by their canonical ID format: `mcp:<server_name>:<tool_name>`.
- The `ComposedToolCatalog` and `ToolCatalogPlanner` use the canonical tool ID namespace to resolve tools. The planner strictly filters tools during selection by matching the canonical ID or unique provider name to prevent exposing same-named tools from different servers.

### 3. Progressive Tool Discovery

- When configured with `mcp_discovery_threshold`, the `ToolCatalogPlanner` compares the total number of cached MCP tools against the threshold.
- If the threshold is exceeded, the planner hides all MCP tools from the baseline catalog and instead registers a lightweight `search_tools` tool.
- The model uses `search_tools` to find relevant tools, inspects their schemas via `get_tool_details`, and selects them. The planner dynamically exposes selected tools to the catalog on subsequent turns.

### 4. Secure Risk Calculation & Host-Side Annotations

- MCP tools are assigned `RiskLevel::Medium` by default. Under no circumstances does a server trust level or server-declared schema alone allow downgrading to `RiskLevel::Low`.
- Host-side tool annotations can be configured in `gestalt.json` under `tool_annotations`:
  ```json
  "mcp": {
    "servers": {
      "my-server": {
        "tool_annotations": {
          "my_tool": {
            "read_only": "true"
          }
        }
      }
    }
  }
  ```
- The `McpBackedTool` loads these annotations with `AnnotationSource::BuiltInTrusted`. The risk calculator downgrades a tool's risk to `RiskLevel::Low` only when the server's `trust_level` is `"high"` and either a verified `"read_only"` or `"idempotent"` host-side annotation is present.

### 5. Event Bus Integration

- `McpRegistry` and `McpClient` execute callbacks to publish events.
- `AgentRuntimeBuilder` registers listeners that translate these into `RuntimeEvent`s published to the global event bus:
  - `McpServerConnecting` (emitted on initialization start)
  - `McpServerConnected` (emitted on successful initialization)
  - `McpToolCatalogRefreshed` (emitted on schema listing updates)
  - `McpToolListChanged` (emitted on notification of tools list change from server)
  - `McpToolCallStarted` (emitted before calling `call_tool`)
  - `McpToolCallCompleted` (emitted on call completion, recording duration and success status)

## Consequences

### Positive

- **Low Startup Overhead:** Lazy lifecycle keeps process execution and handshakes out of prompt execution critical paths.
- **Explicit Risk Control:** Servers cannot bypass risk policies. High-trust servers require host-side opt-in annotations (`read_only` / `idempotent`) to reduce risk to `Low`.
- **Namespace Safety:** Multi-server naming conflicts are resolved automatically by scoping names via `mcp:<server_name>:<tool_name>`.
- **Telemetry Observability:** The host can monitor MCP durations, errors, and list mutations directly on the runtime event bus.

### Neutral

- **JSON Configuration Expansion:** `gestalt.json` now includes `mcp` server lists, lifecycle, trust levels, and tool annotations, increasing configuration schema surface area.

### Negative

- **Stdio Child Processes**: Managing stdio child processes requires background tasks and process cleanup on drop (`kill_on_drop`). If a process hangs during shutdown, it must be forcefully terminated to prevent resource leaks.
