---
title: "Extension Package Contract v1"
status: published
type: version-contract
target: v0.1
owners:
  - gestalt-runtime
  - gestalt-app
authority: implementation-contract
---

# Extension Package Contract v1

## Manifest

`gestalt.extension.toml` must contain integer `manifest_version = 2`. V1,
missing-version, inferred-version, and migration paths are unsupported.

Package and component IDs use lowercase stable identifiers. Package IDs using
the reserved `gestalt` or `harness` prefixes are rejected. Component IDs must
be unique within a package. Lifecycle, command-tool, and MCP components require
an entrypoint; client-product components require a descriptor. Command tools
also require a non-empty description, input schema, risk, `read_only`, and
`idempotent`.

## Trust and Activation

External packages are untrusted by default. A trust pin must contain the exact
package ID and current manifest hash:

```json
{
  "extensions": {
    "trusted": [
      "com.example.review:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    ]
  }
}
```

A bare package ID never inherits the discovered hash and never grants trust.
Changing the manifest invalidates its pin.

`extensions.allow_untrusted` is an experimental development escape hatch. It
does not activate all discovered packages. An untrusted package activates only
when the flag is true and an enabled explicit instance names that package.
Such activation emits an `untrusted_activation` diagnostic. A configured
untrusted instance fails closed when the flag is false.

Instances are resolved in instance-ID order. Each specifies package ID,
enabled state, component overrides, JSON config, and grants. Omitted component
overrides mean enabled. Disabled instances are omitted. An enabled instance
that disables every package component is an error.

## Permissions

Effective permission is the intersection of the component manifest and the
instance grant. Host network policy is an additional upper bound.

| Capability | Enforcement |
|---|---|
| Workspace read/write and extra paths | canonical path checks; traversal and symlink escape are rejected |
| Command tools and lifecycle processes | effective shell permission before process spawn |
| Package MCP over stdio | effective shell permission before server spawn |
| MCP over HTTP | effective network/domain permission before connection |
| Network domains | manifest allowlist ∩ instance grant ∩ host network policy |

Package MCP components use stdio in v0.1. Direct HTTP MCP configuration remains
subject to the same host network gate.

## Generation Safety

Activation candidates have deterministic fingerprints over package identity,
manifest and executable integrity, trust, components, config, grants, and
permissions. Committing a candidate publishes one atomic generation. Active
assistant/tool work retains a lease on its pinned generation until completion;
replaced resources drain only after the last lease is released.

## Conformance Evidence

- `extension_manifest_v2_tests`
- `extension_instance_config_tests`
- `runtime_permissions_tests`
- `command_tool_tests`
- `extension_manager_tests`
- `extension_reload_tests`
- `runtime_cli_tests`
