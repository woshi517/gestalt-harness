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

Path resolution uses `canonicalize()` on both the requested path and the workspace root to prevent `../` traversal attacks. For paths (or parent directories) that do not yet exist, the validation falls back to resolving and canonicalizing their closest existing ancestor path, then normalizes the relative path segments. If the resolved path falls within the workspace root, `allow_workspace_read` or `allow_workspace_write` is checked. If it falls outside, the `allowed_paths` list is checked. If `allow_all_paths` is `true`, all checks pass immediately.

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

Permissions are declared in each component's `[permissions]` section inside
`gestalt.extension.toml`:

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

### 1. Manifest Validation Time (`extension/package.rs`)

When a V2 package manifest is loaded and validated via
`ExtensionManifestV2::validate()`, the runtime checks:

- `manifest_version` is exactly `2`
- package ID, name, and version are non-empty
- component IDs are unique
- lifecycle, command-tool, and MCP server components have entrypoints
- client-product components have descriptors
- command-tool components include their required tool metadata
- shell entrypoints are rejected when `allow_shell` is `false`

### 2. Broker Spawn Time (`process_extension.rs`)

Before spawning a process-backed lifecycle component, the broker:

- clears the child environment with `cmd.env_clear()`
- re-adds only a small safe allowlist of host variables
- validates the entrypoint again against shell permissions

If the entrypoint violates shell permissions, the component is rejected before
the process starts.

### 3. Component Execution Time

Command-tool execution paths recursively scan JSON input for path-like and
network-like keys before dispatch:

- **Filesystem keys:** `path`, `file`, `dir`, `dest`, `src`, `target`, `output`
- **Network keys:** `url`, `host`, `uri`, `address`
- **Shell:** command execution checks `check_shell_permission()` before invoking a shell

The three permission helpers in `permissions.rs` now take `Permissions` plus an
`extension_id`:

**`check_path_permission(&Permissions, extension_id, workspace_root, path, write, event_bus)`**
1. If `allow_all_paths` → OK
2. If the resolved path lies inside the workspace root → check `allow_workspace_read` or `allow_workspace_write`
3. Otherwise, check `allowed_paths`
4. Publishes `RuntimeEvent::PermissionDecision`

**`check_network_permission(&Permissions, extension_id, host, event_bus)`**
1. Iterates `allow_network` looking for `"*"` or exact hostname match
2. Publishes `RuntimeEvent::PermissionDecision`

**`check_shell_permission(&Permissions, extension_id, event_bus)`**
1. Checks `allow_shell`
2. Publishes `RuntimeEvent::PermissionDecision`

### Read vs Write Intent Detection

When `check_input_permissions` encounters a file-system-like key, it infers write intent if the key contains any of: `write`, `dest`, `output`, `target`. Otherwise it defaults to read. This drives whether `allow_workspace_read` or `allow_workspace_write` is required.

## Input Argument Scanning

Command-tool input scanning is recursive:

```
serde_json::Value::Object(map)  → iterate key-value pairs
serde_json::Value::Array(arr)   → recurse into each element
```

For each key whose lowercase form contains a path-like substring, the string
value is treated as a filesystem path and passed to `check_path_permission()`.
If the key also signals write intent, the write path is checked instead of the
read path.

For each key whose lowercase form contains a network-like substring, the string
value is parsed as a URL and the host portion is passed to
`check_network_permission()`.

Keys whose values are objects or arrays are recursed into. Non-string values for
matched keys are skipped.

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

**Known shell detection:** The command is resolved through wrapper binaries (like `env` or `command`) to find the actual underlying binary being run. The filename of this resolved binary (extracted via `Path::file_name()`) is compared case-insensitively against:

```
sh, bash, zsh, ksh, csh, tcsh, cmd, cmd.exe,
powershell, powershell.exe, pwsh, pwsh.exe, fish
```

If the resolved command names a shell directly, it is rejected unless `allow_shell` is `true`. This prevents wrapper-based shell bypass.

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

- **`trusted`**: Extensions in this list bypass the trust gate and are fully trusted. Can match by extension ID or integrity-aware ID:hash format.
- **`allow_untrusted`**: When `true`, untrusted extensions can run but remain untrusted (marked `is_trusted = false` and not promoted to trusted status). Off by default.
- **`explicit_loads`**: Paths to extension directories that should be discovered regardless of their location. Discovery does not imply trust.
- **`disabled`**: Extensions whose IDs match are loaded but immediately disabled.

The trust gate works as follows: after discovery, each extension is validated against the `extensions.trusted` list (either by exact ID or by `<id>:<manifest_hash>` integrity format). Extensions that do not match the trusted list are rejected unless `allow_untrusted` is set to `true`. Unlike the legacy model, explicitly loaded and global extensions do not automatically bypass the trust gate.

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
