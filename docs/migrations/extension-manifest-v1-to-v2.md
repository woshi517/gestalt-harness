# Extension Manifest V1 To V2 Migration

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
