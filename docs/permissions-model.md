# Permissions Model

## Overview

Gestalt's permissions model is **host-side gating, not OS sandboxing**. Extensions declare what they _want_ to do in their manifest, and the runtime enforces those boundaries at the Rust level — before a tool executes, before a process spawns, and before a network request leaves the host. There is no seccomp, no cgroups, no namespace isolation. The model is honest about this: it provides verification gates, not a security boundary.

## Permission Types

### Filesystem

| Field | Type | Default | Description |
|---|---|---|---|
| `allow_workspace_read` | `bool` | `false` | Read access to files inside the workspace root |
| `allow_workspace_write` | `bool` | `false` | Write access to files inside the workspace root |
| `allowed_paths` | `Vec<String>` | `[]` | Additional paths (outside workspace) the extension can access |
| `allow_all_paths` | `bool` | `false` | Bypass all filesystem checks — access any path |

Path resolution uses `canonicalize()` on both the requested path and the workspace root to prevent `../` traversal attacks. If a path falls within the workspace root, `allow_workspace_read` or `allow_workspace_write` is checked. If it falls outside, the `allowed_paths` list is checked. If `allow_all_paths` is `true`, all checks pass immediately.

### Network

| Field | Type | Default | Description |
|---|---|---|---|
| `allow_network` | `Vec<String>` | `[]` | Hostnames the extension may connect to |

The wildcard `"*"` grants all network access. Otherwise, exact string matching against the host portion of the requested URL is performed (after extracting the host via `url::Url::parse`).

### Shell

| Field | Type | Default | Description |
|---|---|---|---|
| `allow_shell` | `bool` | `false` | Whether the extension may execute shell commands |

Shell access is validated both at manifest parse time (entrypoint command must not be a shell) and at tool execution time (the `check_shell_permission` function is called when shell execution is requested).

### Environment

Environment isolation is a built-in property of the process extension broker, not a configurable permission. Every spawned extension process starts with a clean environment via `cmd.env_clear()`, then selectively inherits a safe allowlist of variables (see [Environment Isolation](#environment-isolation) below).

## How Permissions Are Declared

Permissions are declared in the `[permissions]` section of `gestalt.extension.toml`:

```toml
[permissions]
allow_network = ["github.com", "api.openai.com"]
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = ["/tmp/scratch", "/var/log/gestalt"]
```

Every field defaults to the most restrictive value: no network, no filesystem access, no shell.

## How Permissions Are Enforced

Permissions are checked at three distinct layers:

### 1. Manifest Validation Time (`manifest.rs`)

When an extension manifest is loaded and validated via `ExtensionManifest::validate()`, the runtime checks:

- `id` and `name` must be non-empty
- `runtime` must equal `"stdio"` (the only supported runtime in MVP)
- `entrypoint.command` must be non-empty
- If `allow_shell` is `false`, the entrypoint command must not contain shell metacharacters or name a known shell binary (see [Shell Command Validation](#shell-command-validation))
- If tools/hooks/context injectors are declared, their corresponding `capabilities.*` field must be `true`

### 2. Broker Spawn Time (`process_extension.rs`)

Before spawning the extension's child process, the broker:

- Calls `cmd.env_clear()` and re-adds only safe environment variables
- Validates the entrypoint again against `allow_shell`
- The spawn itself is gated — if the entrypoint violates shell permissions during manifest validation, the extension is rejected before the process starts

### 3. Tool Execution Time (`process_extension.rs`)

Every tool invocation goes through `check_input_permissions()` which recursively walks the JSON input looking for path-like and network-like keys:

- **Filesystem keys**: `path`, `file`, `dir`, `dest`, `src`, `target`, `output` — triggers `check_path_permission()`
- **Network keys**: `url`, `host`, `uri`, `address` — triggers `check_network_permission()`
- **Shell**: the broker's `tools/call` handler calls `check_shell_permission()` before executing shell commands

#### The three check functions (in `permissions.rs`):

**`check_path_permission(manifest, workspace_root, path, write, event_bus)`**
1. If `allow_all_paths` → OK
2. If path canonicalizes inside workspace root → check `allow_workspace_read` (read) or `allow_workspace_write` (write)
3. Otherwise, check `allowed_paths` list
4. Publishes `RuntimeEvent::PermissionDecision`

**`check_network_permission(manifest, host, event_bus)`**
1. Iterates `allow_network` looking for `"*"` or exact hostname match
2. Publishes `RuntimeEvent::PermissionDecision`

**`check_shell_permission(manifest, event_bus)`**
1. Checks `allow_shell` boolean
2. Publishes `RuntimeEvent::PermissionDecision`

### Read vs Write Intent Detection

When `check_input_permissions` encounters a file-system-like key, it infers write intent if the key contains any of: `write`, `dest`, `output`, `target`. Otherwise it defaults to read. This drives whether `allow_workspace_read` or `allow_workspace_write` is required.

## Input Argument Scanning

The `check_input_permissions` function in `process_extension.rs` performs a recursive scan of the tool's JSON input:

```
serde_json::Value::Object(map)  → iterate key-value pairs
serde_json::Value::Array(arr)   → recurse into each element
```

For each key whose lowercase form contains a path-like substring — `path`, `file`, `dir`, `dest`, `src`, `target`, `output` — the string value is treated as a filesystem path and passed to `check_path_permission`. If the key also contains `write`, `dest`, `output`, or `target`, the intent is flagged as write.

For each key whose lowercase form contains a network-like substring — `url`, `host`, `uri`, `address` — the string value is parsed as a URL (via `url::Url::parse`) and the host portion is passed to `check_network_permission`.

Keys whose values are objects or arrays are recursed into. Non-string values for matched keys are skipped (e.g., a key named "path" whose value is an object rather than a string).

This works for both top-level parameters and nested structures within `input_schema`.

## Audit Trail

Every permission check publishes a `RuntimeEvent::PermissionDecision` to the event bus:

```rust
RuntimeEvent::PermissionDecision {
    extension_id: String,    // e.g., "mock-ext"
    capability: String,      // "filesystem" | "network" | "shell"
    permission: String,      // "read" | "write" | "connect" | "execute"
    resource: Option<String>, // the path, hostname, or None for shell
    granted: bool,
    reason: Option<String>,  // error message if denied, None if granted
}
```

All events are stored in the bus's history (`Vec<RuntimeEvent>` via `Arc<Mutex<...>>`) and can be inspected programmatically or written to logs.

## Environment Isolation

When spawning an extension's child process, the runtime clears the entire environment and only preserves a safe allowlist:

```
PATH, HOME, USER, LOGNAME, SHELL, TERM, LANG,
LC_ALL, LC_CTYPE, TMPDIR, TEMP, TMP
```

These are copied from the parent process via `std::env::var()`. No other environment variables — including secrets, API keys, or session tokens from the parent — are passed to the extension. This prevents accidental credential leakage to extensions.

## Shell Command Validation

When `allow_shell` is `false`, the manifest validation enforces two rules on `entrypoint.command`:

**Metacharacter detection:** The command must not contain any of: space, `|`, `&`, `;`, `>`, `<`. Presence of these indicates the command requires shell interpretation (e.g., piping, chaining, redirection).

**Known shell detection:** The command's filename (extracted via `Path::file_name()`) is compared case-insensitively against:

```
sh, bash, zsh, ksh, csh, tcsh, cmd, cmd.exe,
powershell, powershell.exe, pwsh, pwsh.exe, fish
```

If the command names a shell directly, it is rejected unless `allow_shell` is `true`.

## Limitations & Honesty

This permissions model is **host-side verification, not OS-level sandboxing**. Specifically:

- **No seccomp** — extensions can make arbitrary system calls
- **No cgroups** — extensions share CPU/memory with the runtime
- **No namespace isolation** — extensions share the same filesystem, PID, and network namespaces
- **No kernel-level enforcement** — all checks are done in the Rust runtime before passing data to the extension; a malicious extension that has already been spawned can ignore path/network checks because it operates at the same OS level as the host

The model prevents _accidental_ policy violations and provides an audit trail. It does not provide a security boundary against a malicious extension that intentionally bypasses the host-side checks. For true sandboxing, extensions would need to be run in containers, VMs, or with seccomp profiles.

## Configuration

Users control which extensions run via `config.toml` under `[extensions]`:

```toml
[extensions]
# Extension IDs to trust unconditionally
trusted = ["my-editor-ext", "formatter-v2"]
# Allow all project-local extensions (unsafe — use trusted list instead)
allow_untrusted = false
# Explicit paths to extension manifest directories
explicit_loads = ["./vendor/exts/linter"]
# Extension IDs to disable without removing
disabled = ["beta-ext-needs-work"]
```

- **`trusted`**: Extensions in this list bypass the trust gate. Loaded even if they're project-local.
- **`allow_untrusted`**: When `true`, all project-local extensions are loaded regardless of trust. Off by default.
- **`explicit_loads`**: Paths to extension directories that should be discovered regardless of their location.
- **`disabled`**: Extensions whose IDs match are loaded but immediately disabled.

The trust gate works as follows: after discovery, each extension is marked as project-local if it lives under the project directory. Project-local extensions are rejected unless their ID is in `trusted` or `allow_untrusted` is `true`. Explicitly-loaded extensions are always trusted.
