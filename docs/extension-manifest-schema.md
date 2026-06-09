# Extension Manifest Schema

## Overview

Every Gestalt extension ships a `gestalt.extension.toml` file at the root of its directory. This TOML document declares the extension's identity, how to launch it, what capabilities it exposes (tools, hooks, context injectors), and what permissions it requires. The manifest is parsed by `ExtensionManifest::parse()` and validated by `ExtensionManifest::validate()` before the extension is loaded.

## Complete Schema Reference

### Top-level fields

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `id` | `string` | yes | — | Unique identifier, e.g. `"mock-ext"`. Used in events, trust configuration, and runtime routing. Must be non-empty. |
| `name` | `string` | yes | — | Human-readable name, e.g. `"Mock Stdio Extension"`. Must be non-empty. |
| `version` | `string` | yes | — | Semantic version string, e.g. `"0.1.0"`. |
| `runtime` | `string` | yes | — | Extension runtime protocol. In MVP, only `"stdio"` is accepted. |

```toml
id = "mock-ext"
name = "Mock Stdio Extension"
version = "0.1.0"
runtime = "stdio"
```

### `[entrypoint]`

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `command` | `string` | yes | — | The binary or script to execute. Must be non-empty. Subject to shell validation when `allow_shell = false`. |
| `args` | `Vec<string>` | no | `[]` | Arguments passed to the command on spawn. |

```toml
[entrypoint]
command = "/path/to/extension.sh"
args = ["--verbose", "--config", "prod"]
```

### `[capabilities]`

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `tools` | `bool` | no | `false` | Enables tool registration. Must be `true` if `[[tools]]` entries exist. |
| `hooks` | `bool` | no | `false` | Enables hook registration. Must be `true` if `[[hooks]]` entries exist. |
| `context` | `bool` | no | `false` | Enables context injector registration. Must be `true` if `[[context_injectors]]` entries exist. |

```toml
[capabilities]
tools = true
hooks = false
context = true
```

### `[permissions]`

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `allow_network` | `Vec<string>` | no | `[]` | List of allowed hostnames. Use `"*"` for all hosts. |
| `allow_workspace_read` | `bool` | no | `false` | Read access to files inside the workspace root. |
| `allow_workspace_write` | `bool` | no | `false` | Write access to files inside the workspace root. |
| `allow_shell` | `bool` | no | `false` | Allow shell execution. When `false`, the entrypoint command is validated for metacharacters and known shell names. |
| `allow_all_paths` | `bool` | no | `false` | Bypass all filesystem permission checks. |
| `allowed_paths` | `Vec<string>` | no | `[]` | Additional paths (outside workspace root) the extension may access. |

```toml
[permissions]
allow_network = ["github.com"]
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = ["/tmp/allowed"]
```

### `[[tools]]`

Each tool is an array of tables (`[[tools]]`). A manifest may declare zero or more tools.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | `string` | yes | — | Tool name used in `tools/call` RPC method. |
| `description` | `string` | yes | — | Free-text description of what the tool does. |
| `input_schema` | JSON value | yes | — | JSON Schema object describing the tool's input. |
| `risk` | `string?` | no | `"high"` | Risk level: `"low"`, `"medium"`, `"high"`, or `"critical"`. |
| `read_only` | `bool?` | no | `false` | Whether the tool only reads without side effects. Helps the harness decide parallel execution and retry eligibility. |
| `idempotent` | `bool?` | no | `false` | Whether repeated calls with the same input produce the same result. Only effective when the extension is trusted. |

```toml
[[tools]]
name = "bash_tool"
description = "Brings hello from bash"
input_schema = { type = "object" }
risk = "low"
read_only = true
idempotent = true
```

### `[[hooks]]`

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | `string` | yes | — | Hook name. |
| `lifecycle_point` | `string` | yes | — | Lifecycle point the hook attaches to (e.g., `"before_context_build"`, `"prepare_next_turn"`). |

```toml
[[hooks]]
name = "mock_hook"
lifecycle_point = "before_context_build"
```

### `[[context_injectors]]`

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | `string` | yes | — | Context injector name. |

```toml
[[context_injectors]]
name = "bash_context"
```

## Full Example

The mock extension fixture used in tests provides a complete worked example:

```toml
id = "mock-ext"
name = "Mock Stdio Extension"
version = "0.1.0"
runtime = "stdio"

[entrypoint]
command = "/home/user/project/crates/gestalt-runtime/tests/fixtures/extensions/mock-ext/mock_ext.sh"

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

This declares a single tool (`bash_tool`), a context injector (`bash_context`), and only workspace read access with no network or shell permissions.

## Validation Rules

`ExtensionManifest::validate()` enforces the following rules. All violations return an `Err(String)`:

| Rule | Condition | Error message |
|---|---|---|
| ID non-empty | `self.id.trim().is_empty()` | `"Extension ID cannot be empty"` |
| Name non-empty | `self.name.trim().is_empty()` | `"Extension Name cannot be empty"` |
| Runtime must be "stdio" | `self.runtime != "stdio"` | `"Unsupported runtime: '...'. Only 'stdio' is supported in MVP"` |
| Entrypoint non-empty | `self.entrypoint.command.trim().is_empty()` | `"Entrypoint command cannot be empty"` |
| Tools declared → tools capability | `!self.tools.is_empty() && !self.capabilities.tools` | `"Extension declares tools but capabilities.tools is false"` |
| Hooks declared → hooks capability | `!self.hooks.is_empty() && !self.capabilities.hooks` | `"Extension declares hooks but capabilities.hooks is false"` |
| Context injectors declared → context capability | `!self.context_injectors.is_empty() && !self.capabilities.context` | `"Extension declares context injectors but capabilities.context is false"` |
| Shell metacharacters in entrypoint (allow_shell=false) | `cmd.contains(' \|')` / `cmd.contains('|')` / etc. | `"Entrypoint command requires shell interpretation but allow_shell permission is false"` |
| Shell binary in entrypoint (allow_shell=false) | Command filename matches known shell (case-insensitive): `sh`, `bash`, `zsh`, `ksh`, `csh`, `tcsh`, `cmd`, `cmd.exe`, `powershell`, `powershell.exe`, `pwsh`, `pwsh.exe`, `fish` | `"Entrypoint command is a shell executable but allow_shell permission is false"` |

## Field Constraints

### Deduplication

The manifest schema does not enforce uniqueness on tool/hook/injector names at the deserialization or validation layer. Tool names must be unique for downstream tool registration to succeed — duplicate names will collide in the tool registry.

### Naming conventions

- `id`: The Rust struct uses the parsed TOML value directly. No character restrictions are enforced beyond being non-empty. The id is used in event bus messages, trust configuration, and extension rejection reasons.
- `tool.name`, `hook.name`, `context_injector.name`: Free-form strings. These are passed to the RPC layer as-is.
- `risk`: Must be one of `"low"`, `"medium"`, `"high"`, `"critical"`. Defaults to `"high"` when absent. This is an `Option<String>` — invalid values are caught downstream by `RiskLevel` deserialization.

### Serde defaults

All bool fields default to `false`, all `Vec<String>` fields default to `[]` (empty). The `input_schema` JSON value has no default — it must be provided for every tool. The `risk` field defaults to `None` (which is then mapped to `"high"` by the runtime).
