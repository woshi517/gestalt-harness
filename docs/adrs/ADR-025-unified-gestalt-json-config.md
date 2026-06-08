# ADR-025: Unified `gestalt.json` Configuration

**Status:** Accepted

## Context

The harness configuration was split across multiple files and multiple consumers:

- `~/.config/gestalt/config.toml` (global config)
- `<root>/.gestalt/config.toml` (workspace config)
- `<root>/.gestalt/policies.toml` (policy rules + hidden prompt overrides)

This split produced three problems:

1. **Users cannot inspect or edit harness behavior in one place.** Settings were spread across files with different formats and ownership boundaries.
2. **Boundaries were leaky.** Prompt overrides lived inside `policies.toml` but were consumed outside the policy layer in `run.rs`.
3. **Every config-writing command was format- and path-specific.** `connect.rs`, `profiles.rs`, and `runtime.rs` each mutated TOML config directly, spreading write-path assumptions and preventing schema evolution.

## Decision

Consolidate all user-owned harness configuration into a single JSON document:

- **Workspace config:** `<root>/gestalt.json`
- **Global config:** `~/.config/gestalt/gestalt.json`

The unified file owns: defaults, profiles, providers, tools, context, observe, prompt, policies, extensions, and TUI settings.

Layering precedence (lowest to highest): built-in defaults < global `gestalt.json` < workspace `gestalt.json` < `GESTALT_*` env vars < CLI flags.

### What stays separate

- **`gestalt.extension.toml`** — extension manifests remain TOML. They describe extension packages, not workspace choices.
- **System keychain** — API key secrets. Config only stores an `auth_ref: "secret:provider/<name>"` pointer.
- **`.gestalt/workspace.md` / `.gestalt/memory.md`** — content files, with paths configurable from `context.workspace_file` and `context.memory_file`.
- **`.gestalt/runs/`** — runtime artifacts.
- **Model catalog** — compiled into `gestalt-models` crate.

### Migration strategy

1. JSON-first loading: prefer `gestalt.json` for both global and workspace layers.
2. Legacy TOML fallback: when no `gestalt.json` exists, the loader reverts to `.gestalt/config.toml` + `.gestalt/policies.toml` (workspace) or `~/.config/gestalt/config.toml` (global).
3. Transparent seeding: when a mutating command (`profiles use`, `connect`, `runtime enable/disable`) writes `gestalt.json` for the first time, it seeds from the existing legacy TOML files so no data is lost.
4. Global bootstrap: if neither the canonical JSON nor the legacy TOML global config exists, `load_effective_config()` creates a minimal `~/.config/gestalt/gestalt.json` (`{"version": 1}`).

### Schema shape

```json
{
  "version": 1,
  "defaults": { "provider": "...", "model": "...", "mode": "...", "max_turns": 50, "profile": "default" },
  "profiles": { "default": { "provider": "openrouter", "model": "openrouter/free" } },
  "providers": { "openrouter": { "kind": "openrouter", "base_url": "...", "api_key_env": "..." } },
  "tools": { "bash_timeout_secs": 60, "max_output_tokens": 4000, "sandbox_type": "none" },
  "context": { "max_context_window": 120000, "reserved_output_tokens": 8000, "workspace_file": ".gestalt/workspace.md", "memory_file": ".gestalt/memory.md" },
  "observe": { "run_log_dir": ".gestalt/runs", "log_format": "jsonl" },
  "prompt": { "override": null, "override_file": null },
  "policies": { "paths": { ... }, "bash": { ... }, "network": { ... } },
  "extensions": { "explicit_loads": [], "disabled": [], "trusted": [], "allow_untrusted": false },
  "tui": { "diagnostics": { "max_log_lines": 1000 } }
}
```

A machine-readable JSON Schema is generated from the Rust types via `schemars` and checked in at `docs/schemas/gestalt.schema.json`.

### Implementation

- All JSON read/write converges on shared helpers in `config.rs`: `write_workspace_config_file()` and `mutate_workspace_config_file()`.
- `load_effective_config()` loads JSON first, falls back to legacy TOML, applies env vars, then CLI overrides.
- `EffectiveConfig` carries a `config_path` field tracking the highest-precedence source file actually read, used by `workspace info` and `policy validate` for accurate reporting.
- Extension trust/disable/enable settings live in `gestalt.json` `extensions` key; extension package manifests remain independent `gestalt.extension.toml` files.

## Consequences

### Positive

- Single file to inspect and edit harness behavior.
- Deterministic override behavior: users can trace every value to its source via `gestalt config explain`.
- Shared write path eliminates format-specific assumptions across CLI commands.
- Machine-readable schema enables editor validation and future compatibility tooling.
- Transparent migration preserves existing settings without user intervention.

### Neutral

- JSON does not support comments, but config files are primarily machine-mutated after initial scaffold.
- Legacy TOML files remain loadable during the compatibility window but are never re-written.
- Snake_case keys preserved for minimal migration churn (matching existing config keys).

### Negative

- Adding new config sections requires updating the shared schema, loaders, and `explain_config()` tracing.
- The compatibility window adds complexity to the loader (legacy path helpers, seed functions, fallback logic).
- Global bootstrap on first config read writes to disk during what users perceive as a read-only operation.
