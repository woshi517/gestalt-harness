# Extension Development Guide

**Status:** Living Document  
**Applies to:** gestalt-harness v0.1  
**Primary Model:** V2 Package & Components  
**Runtime protocol:** Lifecycle Protocol V2 / MCP / stdio

## 1. What an extension is now

An extension is a V2 package with one or more typed components. Stable v0.1
supports only `manifest_version = 2`; there is no V1 loader or migration path.

Supported component kinds:

- `gestalt-lifecycle`
- `command-tool`
- `mcp-server`
- `skill`
- `client-product`

Use `gestalt-lifecycle` when the component needs to participate in the
runtime's typed lifecycle protocol. Use `command-tool` for a direct executable
tool. Use `mcp-server` when the component is an external MCP server.

## 2. Package shape

See [Extension Package Manifest Schema](extension-manifest-schema.md) for the
full field list. The short version:

```toml
manifest_version = 2

[package]
id = "com.example.review"
name = "Review Extensions"
version = "1.0.0"

[[components]]
id = "reviewer"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "review_ext"]
```

Per-component permissions live under `[components.permissions]` and are
enforced host-side before process launch or tool execution.

## 3. Lifecycle Protocol V2

Process-backed lifecycle components speak the V2 JSON-RPC protocol over
newline-delimited stdin/stdout. The supported methods are:

- `initialize`
- `capabilities/describe`
- `lifecycle/invoke`
- `shutdown`
- `$/cancelRequest`

See [Gestalt Lifecycle Protocol V2](jsonrpc-extension-protocol.md) for the wire
format and request/response payloads.

There is no legacy compatibility path in H4A.

## 4. Permissions and trust

Permissions are declared per component in the manifest and intersected with
the configured instance grants before the component starts.

- Filesystem checks use `check_path_permission_effective`
- Network checks use `check_network_permission_effective`
- Shell checks use `check_shell_permission_effective`

The runtime publishes `RuntimeEvent::PermissionDecision` for each check.
Trust requires an exact package-ID/manifest-hash pin. Bare trusted IDs do not
grant trust. `allow_untrusted` is an experimental development escape hatch and
still requires an enabled explicit instance; it never auto-activates all
discovered packages. See the
[v0.1 extension contract](v0.1/extensions.md) for activation and generation
semantics.

## 5. Working with the runtime

Use `ExtensionManifestV2` and the V2 extension discovery path in
`crates/gestalt-runtime/src/discovery.rs`. Process-backed lifecycle components
are launched through `ProcessExtensionBroker`, while in-process native
composition uses `RuntimeModule`.

## 6. Testing

Current V2-focused tests:

- `crates/gestalt-runtime/tests/extension_manifest_v2_tests.rs`
- `crates/gestalt-runtime/tests/lifecycle_protocol_v2_tests.rs`
- `crates/gestalt-runtime/tests/runtime_builder_tests.rs`

These cover manifest rejection, protocol negotiation, and `RuntimeModule`
registration. For runtime behavior, prefer the existing targeted crate tests
over adding new compatibility fixtures.
