---
title: "Gestalt Configuration v1"
status: published
type: version-contract
target: v0.1
owners:
  - gestalt-app
authority: implementation-contract
---

# Gestalt Configuration v1

`gestalt.json` with integer `"version": 1` is the only harness configuration
format. Unknown fields and removed aliases are errors.

## Layers

Values resolve in this order:

1. built-in defaults;
2. global `$XDG_CONFIG_HOME/gestalt/gestalt.json`;
3. workspace `gestalt.json`;
4. supported `GESTALT_*` environment variables;
5. CLI flags.

Invalid higher-priority values fail instead of falling back. Config explanation
records the winning layer, source location, default/override state, and
redaction state for every reported value.

## Section Maturity

| Section | Maturity |
|---|---|
| `version`, `defaults`, `profiles`, `providers`, `prompt`, `context`, `tools`, `policies`, `observe` | stable |
| `mcp`, `skills`, `extensions` | experimental |
| `workspace` | internal metadata |
| `tui` | client-specific |

Provider `api_key` and credential-bearing headers are experimental and always
redacted from reports. Stable configuration should use `auth_ref` or
`api_key_env`. A provider may specify exactly one of `api_key`, `api_key_env`,
or `auth_ref`; specifying more than one is an error. The removed `secret:`
auth-reference syntax is rejected.

## Example

```json
{
  "$schema": "docs/schemas/gestalt.schema.json",
  "version": 1,
  "defaults": {
    "profile": "default",
    "mode": "confirm"
  },
  "profiles": {
    "default": {
      "provider": "openai",
      "model": "gpt-5.5-preview"
    }
  },
  "providers": {
    "openai": {
      "api_key_env": "OPENAI_API_KEY"
    }
  },
  "policies": {
    "paths": {
      "allow_read": ["."],
      "allow_write": ["."]
    },
    "bash": {
      "allow": ["git", "cargo"],
      "deny": ["sudo"]
    },
    "network": {
      "allow_domains": ["api.openai.com"]
    }
  }
}
```

When both global and workspace policies define allow lists, every workspace
entry must already exist in the corresponding global list. This applies to
path reads, path writes, Bash commands, and network domains. Deny lists are
combined by union.

`context.management.capture` controls context-report contribution capture.
Its values are `disabled`, `hash_only`, `redacted`, and `full_for_replay`;
`hash_only` is the default. `full_for_replay` stores raw contribution content
and must be enabled explicitly. See
[Context Build Report v1](./context-build-report.md).

## Errors

| Condition | Stable code |
|---|---|
| Missing version | `CONFIG_VERSION_MISSING` |
| Non-integer version | `CONFIG_VERSION_INVALID` |
| Version other than 1 | `CONFIG_VERSION_UNSUPPORTED` |
| Known legacy TOML path | `UNSUPPORTED_LEGACY_CONFIG` |
| Unknown field or invalid value | `CONFIG_ERROR` |

Known `.gestalt/config.toml`, `.gestalt/policies.toml`, and global
`gestalt/config.toml` files are rejected before their contents are read. They
are never parsed, seeded, rewritten, deleted, or migrated.
`gestalt.extension.toml` is an extension manifest and is not legacy harness
configuration.

Removed v1 aliases are also rejected as unknown fields:
`context.workspace_file`, `context.memory_file`, `policies.bash.yolo_allow`,
`policies.bash.always_confirm`, and `policies.bash.always_deny`.

## Future Versions

A future schema version must add an explicit `(from_version, to_version)`
reader/migration registration, golden source and destination fixtures,
idempotence tests, unknown-field tests, and rollback documentation. No
migration command exists until a second supported schema version is implemented.

## Conformance Evidence

| Criteria | Evidence |
|---|---|
| H3A-F01, B02 | `config_schema_tests`: generated schema drift, default round trip, valid fixtures, unknown fields, and distinct version failures |
| H3A-F02, B06 | `config_rejects_removed_aliases`, `config_rejects_secret_auth_ref`, schema absence scan, and no migration command |
| H3A-F03, F07 | This contract's maturity and future-version sections |
| H3A-F04, B01, B05 | Config precedence/provenance tests, all policy monotonicity/union tests, credential exclusivity tests, and CLI redaction tests |
| H3A-F05, F06, B03 | App config/connect tests and CLI workspace/main contract tests for load, mutation, doctor, and projection |
| H3A-B04 | `runtime_cli_tests` extension-manifest activation fixture |
