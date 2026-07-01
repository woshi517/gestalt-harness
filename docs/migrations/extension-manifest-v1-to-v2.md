# Extension Manifest V1 To V2 Migration

> **Status:** Historical pre-release reference. Manifest V1 was never part of a
> stable Gestalt release and is unsupported under
> [ADR-031](../adrs/ADR-031-v0-1-greenfield-compatibility-cutoff.md). The
> runtime does not parse or migrate V1 manifests. This document is retained
> temporarily for historical context and must not be linked as an active
> support path.

V1 manifests describe one process extension. V2 manifests describe one package with one or more typed components.

Minimal lifecycle migration:

```toml
manifest_version = 2

[package]
id = "com.example.review"
name = "Review"
version = "1.0.0"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "review_ext"]
```

Command-tool-only package:

```toml
manifest_version = 2

[package]
id = "com.example.tools"
name = "Tools"
version = "1.0.0"

[[components]]
id = "echo"
kind = "command-tool"
description = "Echo JSON"
input_schema = { type = "object" }
risk = "Low"
read_only = true
idempotent = true

[components.entrypoint]
command = "/bin/cat"
```

MCP-only package:

```toml
manifest_version = 2

[package]
id = "com.example.mcp"
name = "MCP"
version = "1.0.0"

[[components]]
id = "server"
kind = "mcp-server"

[components.entrypoint]
command = "node"
args = ["server.js"]
```

Optional components use `optional = true`. Optional failure degrades package health instead of rejecting the candidate.
---
title: Historical Extension Manifest V1 to V2 Migration
status: historical
type: migration
target: pre-v0.1
owners:
  - gestalt-runtime
---

> Historical only. Stable v0.1 does not accept or migrate extension manifest
> V1. See [ADR-031](../adrs/ADR-031-v0-1-greenfield-compatibility-cutoff.md).
