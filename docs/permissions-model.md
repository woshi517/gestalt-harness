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

## Structured Failure Surface

Permission denials and other tool failures that surface to the model carry a typed `ToolFailureKind` plus optional `repair_guidance`. The harness always renders a single line in the tool output that the model can see, in addition to the structured field:

```
[ApprovalDenied] medium-risk tool call
repair: The user denied the approval. Adjust the request or ask before retrying.
```

Mapping of the high-traffic kinds:

| Trigger | `kind` | `repair_guidance` (excerpt) |
|---|---|---|---|
| User denied an approval | `approval_denied` | "Adjust the request or ask before retrying." |
| Policy config denied | `policy_denied` | echoes the policy reason |
| Schema mismatch (basic or strict pass) | `schema_mismatch` | "Expected schema: …" |
| Malformed JSON arguments | `invalid_arguments` | "The arguments could not be parsed as JSON." |
| Tool not in catalog | `tool_not_found` | "Check spelling or ensure the tool is loaded." |
| Duplicate `tool_use_id` in a turn | `duplicate_call_id` | "Use unique IDs per turn." |
| Disallowed namespace (e.g. MCP in yolo mode) | `disallowed_namespace` | "This namespace is not allowed." |
| Tool execution returned an error | `execution_failed` | tool-specific |
| Tool execution timed out | `timeout` | "Retry, or split the call into smaller inputs." |
| Malformed provider streaming output | `unknown` | "Turn could not be completed." |

Only `timeout` is considered **transient** by the executor's retry policy. All other kinds — including `execution_failed` — are permanent, meaning retrying with the same input is unlikely to succeed without model or user intervention.

Failures that occur before tool execution (validation, policy, approval) are classified as `is_pre_execution` and are excluded from first-call success rate calculations in trace metrics.

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

Users control which extensions run via `gestalt.json` under `extensions`:

```json
"extensions": {
  "trusted": ["my-editor-ext", "formatter-v2"],
  "allow_untrusted": false,
  "explicit_loads": ["./vendor/exts/linter"],
  "disabled": ["beta-ext-needs-work"]
}
```

- **`trusted`**: Extensions in this list bypass the trust gate. Loaded even if they're project-local.
- **`allow_untrusted`**: When `true`, all project-local extensions are loaded regardless of trust. Off by default.
- **`explicit_loads`**: Paths to extension directories that should be discovered regardless of their location.
- **`disabled`**: Extensions whose IDs match are loaded but immediately disabled.

The trust gate works as follows: after discovery, each extension is marked as project-local if it lives under the project directory. Project-local extensions are rejected unless their ID is in `trusted` or `allow_untrusted` is `true`. Explicitly-loaded extensions are always trusted.

## Skills

Skills are passive instruction packages (not extensions) that progressively load into the runtime through the [Agent Skills](https://agentskills.io) `SKILL.md` format. They participate in the permission model through a **narrowing overlay** on the visible and executable tool surface.

### What Skills Cannot Do

Skills are not tools and do not register runtime capabilities. A skill cannot:

- Spawn subprocesses or open network connections on its own.
- Grant new permissions that the workspace policy has not already granted.
- Bypass workspace policy or approval evaluation.
- Self-attest trust — discovery source determines trust level, not skill content.

### Skill Trust Levels

A skill's trust level is assigned at discovery time and is derived from where it was found, not from anything in the manifest:

| Trust Level | Source | Auto-Activated on Trigger? |
|-------------|--------|-----------------------------|
| `Explicit`  | User-provided path (CLI flag, config entry) | Yes (trusted) |
| `Workspace` | `.gestalt/skills/` or `.agents/skills/` in the active workspace | Yes (trusted) |
| `Global`    | `~/.config/gestalt/skills/` or `~/.agents/skills/` | No by default; trusted if name is in `skills.trusted` |
| `Downloaded`| Untrusted or registry-fetched source | No by default; trusted if name is in `skills.trusted` |

`Global` and `Downloaded` skills require an explicit user activation (e.g., `/skill <name>` or `gestalt run --skill <name>`) unless the skill's name appears in the `skills.trusted` allow-list in `gestalt.json`. Names in `skills.trusted` are treated as auto-activatable regardless of where they were discovered. Activation surfaces reject any unknown or untrusted name with a deterministic error before mutating state, so a typo or a misconfigured entry never silently drops at runtime.

### Tool Filtering

A skill may declare an `allowed-tools` frontmatter field as a space-separated list of tool names. When one or more skills with `allowed-tools` are active, the visible tool catalog becomes the **intersection** of the base catalog with the union of active-skill allowances:

```text
effective_visible_catalog = base_catalog
                             ∩ (∪ allowed-tools of active skills, if any)
                             ∩ policy-filter
                             ∩ approval-filter
```

A skill with no `allowed-tools` field falls back to the base catalog rather than hiding everything. The narrowest-restrictive rule (intersection) means adding a second skill with `allowed-tools` can only restrict the visible catalog further; it cannot widen access.

Tool filtering is **dynamic per turn**: the `ToolCatalogPlanner` holds a reference to the shared `Arc<Mutex<SkillContributorState>>` and queries `active_descriptors()` on every `plan_descriptors` call. A skill activated or deactivated mid-session through `/skill <name>` or `/skill off <name>` immediately affects the visible tool surface on the next turn without requiring a runtime restart.

### Execution-Time Enforcement

Visibility filtering is not a security boundary by itself — a determined provider could still emit a tool call to a filtered-out tool. The runtime policy engine therefore also enforces the skill overlay at **execution time** in `RuntimePolicyEngine::skill_policy`. If the provider emits a tool call to a tool that no active skill allows, the call is denied with a traceable reason and a `RuntimeEvent::SkillPolicyApplied` is emitted.

The fail-closed semantics ensure that prompt shaping and runtime enforcement stay aligned. A skill cannot grant authority the workspace has not granted, and a tool the user expected to be hidden cannot be smuggled in by the provider.

### Resource Access

Skill resources under `scripts/`, `references/`, and `assets/` are loaded through ordinary file tools (`Read`, `Search`, `Bash`, etc.). The skill does not have its own executor. Resource reads still go through:

1. The active skill's tool allow-list.
2. Workspace policy (path, network, shell).
3. Approval evaluation, if required by mode or risk.

This preserves provenance — every file read is logged in the trace, attributable to a specific tool call, and gateable by the existing policy model.

### Configuration

Users control which skills load via `gestalt.json` under `skills`:

```json
"skills": {
  "explicit_paths": ["./vendor/skills/pdf-processing"],
  "active": ["pdf-processing"],
  "trusted": ["my-internal-skills"]
}
```

- **`explicit_paths`**: Paths to skill directories loaded as `Explicit` trust regardless of their on-disk location. Useful for vendored or registry-checked skills.
- **`active`**: Skill names to activate by default for runs in this workspace. Each name is validated against the discovered set at startup; unknown names fail-fast with a clear error.
- **`trusted`**: Skill names that should be considered auto-activatable even when discovered from `Global` or `Downloaded` sources. This widens activation for those skills but does not widen permission scope — the skill is still constrained by its `allowed-tools` and the existing workspace policy.

To deactivate a skill for a single run, pass `--no-skill <name>` (or omit it from `--skill`). To deactivate permanently, remove it from the discovery path or the config.

**Session deactivation.** In chat mode, `/skill off <name>` records the skill name with a `!` prefix in `CliOverrides::skills` (e.g., `!pdf-processing`). The config loader interprets `!`-prefixed entries as removals from the active set. This mechanism is session-local and does not mutate workspace config.

