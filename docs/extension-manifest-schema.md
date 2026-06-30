# Extension Package Manifest Schema

## Overview

`gestalt.extension.toml` is a V2 package manifest. Stable v0.1 accepts only
`manifest_version = 2`; there is no V1 compatibility path and no manifest
inference fallback.

The manifest is parsed by `ExtensionManifestV2::parse()` and validated by
`ExtensionManifestV2::validate()` in
`crates/gestalt-runtime/src/extension/package.rs`.

## Top-Level Shape

```toml
manifest_version = 2

[package]
id = "com.example.review"
name = "Review Extensions"
version = "1.0.0"

[compatibility]
gestalt = "v0.1"

[[components]]
id = "reviewer"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "review_ext"]
```

## Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `manifest_version` | integer | yes | Must be `2`. |
| `[package].id` | string | yes | Lowercase package ID; must not use reserved `gestalt`/`harness` prefixes. |
| `[package].name` | string | yes | Human-readable package name. |
| `[package].version` | string | yes | Package version string. |
| `[compatibility].gestalt` | string | no | Informational host compatibility marker. It does not restore V1 behavior. |
| `[[components]]` | table array | yes | One or more component declarations. |

## Component Kinds

The supported component kinds are:

- `gestalt-lifecycle`
- `command-tool`
- `mcp-server`
- `skill`
- `client-product`

## Component Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `id` | string | yes | Unique within the package. |
| `kind` | string | yes | One of the supported component kinds. |
| `optional` | bool | no | Defaults to `false`. |
| `entrypoint` | table | conditional | Required for lifecycle, command-tool, and MCP server components. |
| `descriptor` | string | conditional | Required for `client-product` components. |
| `description` | string | conditional | Required for `command-tool` components. |
| `input_schema` | JSON | conditional | Required for `command-tool` components. |
| `risk` | string | conditional | Required for `command-tool` components. |
| `read_only` | bool | conditional | Required for `command-tool` components. |
| `idempotent` | bool | conditional | Required for `command-tool` components. |
| `permissions` | table | no | Per-component permission overrides. |

## Permissions

Permissions are declared per component and copied into the resolved runtime
component:

| Field | Type | Default | Notes |
|---|---|---|---|
| `allow_network` | array<string> | `[]` | Host allowlist; `*` matches any host. |
| `allow_workspace_read` | bool | `false` | Workspace read access. |
| `allow_workspace_write` | bool | `false` | Workspace write access. |
| `allow_shell` | bool | `false` | Enables shell execution. |
| `allow_all_paths` | bool | `false` | Bypasses filesystem checks. |
| `allowed_paths` | array<string> | `[]` | Extra allowed filesystem paths. |

## Validation Rules

Validation fails when:

- `manifest_version` is missing, non-integer, or not `2`;
- the package ID, name, or version is empty;
- component IDs are duplicated;
- a lifecycle, command-tool, or MCP server component is missing an entrypoint;
- a client-product component is missing a descriptor;
- a command-tool component is missing its required tool metadata;
- `allow_shell = false` and the entrypoint requires shell interpretation or names a shell binary.

## Example

```toml
manifest_version = 2

[package]
id = "com.example.docs"
name = "Docs Search"
version = "1.2.0"

[[components]]
id = "search"
kind = "command-tool"
description = "Search docs"
input_schema = { type = "object", properties = { query = { type = "string" } }, required = ["query"] }
risk = "low"
read_only = true
idempotent = true

[components.entrypoint]
command = "python3"
args = ["-m", "docs_search"]

[components.permissions]
allow_network = ["api.openai.com"]
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = []
```
