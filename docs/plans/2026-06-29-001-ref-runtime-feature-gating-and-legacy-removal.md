---
title: "refactor: Complete runtime feature gating and remove legacy modules"
status: planned
created: 2026-06-29
depth: standard
origin: "Follow-up to docs/plans/2026-06-28-crate-boundary.md"
---

# Complete Runtime Feature Gating and Remove Legacy Modules

## Goal

Finish the remaining crate-boundary work by:

- making `gestalt-runtime --no-default-features` exclude optional runtime integrations and their dependency trees;
- moving every file under `crates/gestalt-runtime/src/legacy/` into an owning runtime module; and
- adding structural and dependency checks so neither regression can return.

This is a relocation and feature-boundary change. Preserve default CLI/TUI behavior and public behavior unless a feature is explicitly disabled.

## Current State

The crate layout is consolidated, but the boundary is incomplete:

- `crates/gestalt-runtime/src/lib.rs` mounts nine `legacy_*` modules with `#[path]` and flattens them through broad re-exports.
- `gestalt-runtime` declares `mcp`, `skills`, and `verify` features, but all runtime dependencies are unconditional and all legacy modules compile in every profile.
- `cargo tree -p gestalt-runtime --no-default-features --edges normal` still includes the HTTP/TLS provider stack, MCP support, built-in tool dependencies, skills parsers, verification Markdown parsing, and trace dependencies.
- Base runtime modules directly name MCP, skills, and trace types. Cargo manifest changes alone will not compile until these API references are separated.
- `otel` is an empty, forwarded feature with no OpenTelemetry implementation or dependency. The actual capability is still deferred in `docs/gestalt-harnes-implementation-roadmap.md` under P3.6.

## Scope

In scope:

- module relocation and ownership cleanup inside `gestalt-runtime`;
- runtime features for `providers`, `tools`, `mcp`, `skills`, `verify`, and `trace`;
- optional Cargo dependencies tied to the features that use them;
- feature forwarding through `gestalt-app` and `gestalt-cli`;
- a full-feature product profile for `gestalt-tui`;
- compile, test, dependency-tree, and source-layout guardrails;
- removal of the unimplemented `otel` marker until P3.6 provides a real integration.

Out of scope:

- implementing OpenTelemetry export;
- redesigning runtime behavior or public data models;
- compatibility aliases for `legacy_*` paths, which were never public;
- new facade traits or factories whose only purpose is feature gating.

## Target Module Ownership

Move code into the module that owns its behavior. Where a same-named `.rs` file already exists, convert it to a directory module and merge the implementations.

| Current path | Target path | Notes |
| --- | --- | --- |
| `legacy/context/*` | `context/*` | Move current `context.rs` to `context/mod.rs`, then merge context primitives. |
| `legacy/exec/lib.rs` | `exec.rs` | Keep subprocess execution as a runtime primitive used by built-in tools. |
| `legacy/mcp/*` | `mcp/*` | Move current `mcp.rs` to `mcp/mod.rs`; move `mcp_discovery.rs` to `mcp/discovery.rs`. |
| `legacy/models/*` | `providers/*` | Provider adapters, auth, catalog, SSE, schema adaptation, and provider registry belong here. |
| `legacy/policy/lib.rs` | `policy/mod.rs` | Move current `policy.rs` to `policy/mod.rs` and merge the concrete policy engine. |
| `legacy/skills/*` | `skills/*` | Move `skill_contributor.rs` to `skills/contributor.rs`. |
| `legacy/tools/*` | `tools/*` | Built-ins, backends, path handling, and registry. Keep `tool_output/` as its sibling runtime-owned materialization boundary. |
| `legacy/trace/*` | `trace/*` | Trace persistence, evaluation, fixtures, manifests, and metrics. |
| `legacy/verify/*` | `verify/*` | Verification registry and built-in verifiers. |

Do not preserve a `legacy` directory, `legacy_*` module names, or `#[path = "legacy/..."]` attributes.

## Feature Contract

Use additive Cargo features. The runtime's default embedding profile keeps the crate-boundary specification's provider, built-in tool, and trace behavior:

```toml
[features]
default = ["providers", "tools", "trace"]
providers = ["dep:eventsource-stream", "dep:reqwest", "dep:tokio-stream"]
tools = ["dep:encoding_rs", "dep:regex", "dep:reqwest", "dep:similar", "dep:url"]
mcp = []
skills = ["dep:serde_yaml"]
verify = ["tools", "dep:pulldown-cmark"]
trace = []
```

This list is a starting contract, not permission to guess dependency ownership. After relocation, use `rg` and single-feature builds to place every dependency on the narrowest feature that compiles. Dependencies used by the base runtime remain non-optional. Dependencies shared by two optional features must be enabled by both features.

Rules:

- `--no-default-features` compiles the runtime engine and extension lifecycle without providers, built-in tools, MCP, skills, verification, or trace persistence.
- `verify` may depend on `tools` because patch verification uses the tool patch parser.
- A feature may depend on another feature only when its compiled code directly requires it.
- Default `gestalt-cli` and `gestalt-tui` behavior remains unchanged by enabling the full existing product set.
- Disabled CLI integrations must still parse and return the existing typed "feature not enabled" error rather than disappearing from command help.
- Remove `otel` from runtime, app, CLI, and runtime feature reporting. P3.6 must reintroduce it with a real `otel` module and optional OpenTelemetry dependencies.

## Implementation Units

### U1. Relocate Foundation Modules

Goal: remove legacy naming from always-compiled runtime foundations without changing behavior.

Files:

- `crates/gestalt-runtime/src/lib.rs`
- `crates/gestalt-runtime/src/context.rs`
- `crates/gestalt-runtime/src/legacy/context/**`
- `crates/gestalt-runtime/src/legacy/exec/lib.rs`
- `crates/gestalt-runtime/src/policy.rs`
- `crates/gestalt-runtime/src/legacy/policy/lib.rs`
- all runtime callers of `legacy_context`, `legacy_exec`, and `legacy_policy`

Work:

- Convert `context.rs` and `policy.rs` into directory modules and merge their legacy-owned files.
- Move the execution implementation to `exec.rs`.
- Replace internal `crate::legacy_*` paths with module-owned paths.
- Keep intentional root-level public re-exports, but replace broad `pub use module::*` exports with the existing named API where practical.
- Do not add compatibility modules.

Acceptance:

- `rg -n 'legacy_(context|exec|policy)' crates/gestalt-runtime` returns no source matches.
- `cargo test -p gestalt-runtime --all-features --locked` passes.

### U2. Relocate Optional Integration Modules

Goal: give every remaining legacy file a permanent owner before adding conditional compilation.

Files:

- `crates/gestalt-runtime/src/legacy/mcp/**`
- `crates/gestalt-runtime/src/legacy/models/**`
- `crates/gestalt-runtime/src/legacy/skills/**`
- `crates/gestalt-runtime/src/legacy/tools/**`
- `crates/gestalt-runtime/src/legacy/trace/**`
- `crates/gestalt-runtime/src/legacy/verify/**`
- `crates/gestalt-runtime/src/mcp.rs`
- `crates/gestalt-runtime/src/mcp_discovery.rs`
- `crates/gestalt-runtime/src/skill_contributor.rs`
- `crates/gestalt-runtime/src/tool_output/**`
- all callers of the moved modules

Work:

- Apply the target module map above.
- Keep tool-output shaping, hashing, artifact metadata, and truncation under `tool_output`; do not move them back into core or built-in tool implementations.
- Replace aliases such as `mcp_error`, `mcp_model`, `model_registry`, and `skill_manifest` at internal call sites with their owning module paths.
- Preserve root re-exports only where current app, CLI, TUI, examples, or external API tests require them.
- Move or update integration tests so their imports use stable module ownership.
- Delete the empty `legacy/` directory after the last move.

Acceptance:

- `test ! -d crates/gestalt-runtime/src/legacy`.
- `rg -n '#\\[path\\s*=|legacy_' crates/gestalt-runtime/src crates/gestalt-runtime/tests` returns no architectural legacy matches.
- `cargo test -p gestalt-runtime --all-features --locked` passes.

### U3. Separate Base Runtime Types From Optional Integrations

Goal: make optional modules removable from compilation.

Primary coupling points:

- MCP: `activation.rs`, `builder.rs`, `config.rs`, `extension/mcp_component.rs`, `extension/runtime_snapshot.rs`, `inspect.rs`, `orchestration.rs`, `runtime.rs`, `tool_catalog.rs`, and `tool_catalog_planner.rs`.
- Skills: `config.rs`, `policy.rs`, `runtime.rs`, `skill_contributor.rs`, and `tool_catalog_planner.rs`.
- Trace: `compaction.rs`, `composition_hooks.rs`, `context.rs`, and trace persistence calls.
- Providers: `registry.rs` and app provider/auth/config services.
- Tools and verify: runtime builder registration, app runtime factory, and verification patch parsing.

Work:

- Put optional module declarations and public exports behind their matching `#[cfg(feature = "...")]`.
- Gate fields, constructor arguments, builder methods, and code branches that name optional types.
- Keep feature-neutral runtime configuration and lifecycle behavior in the base build.
- Where context compaction needs a data type but not trace I/O, move that small data primitive into `context` and let `trace` consume it. Do not keep trace persistence compiled merely to share a struct.
- Gate trace writes at the call site; no-op branches should be omitted unless the base API requires a return value.
- Move `CompactionCheckpoint`, `ProjectionManifest`, and `MessageMetadataRef` into a feature-neutral `context::projection` module because base context construction uses them. Keep manifest/checkpoint filesystem persistence and trace evaluation behind `trace`.
- Gate provider registry adapters while keeping the generic core provider traits in `gestalt-core`.
- Keep `tool_output` feature-neutral: the base executor installs `RuntimeToolOutputMaterializer`, and extension/MCP tools also cross that boundary. Its current hashing dependency is already required by the base runtime.
- Add `required-features = ["mcp"]` to `mock_mcp_server` and gate feature-specific integration test crates with crate-level `#![cfg(feature = "...")]`.

Acceptance:

- `cargo check -p gestalt-runtime --no-default-features --locked` passes.
- Each supported feature compiles from the minimal base:

```bash
cargo check -p gestalt-runtime --no-default-features --features providers --locked
cargo check -p gestalt-runtime --no-default-features --features tools --locked
cargo check -p gestalt-runtime --no-default-features --features mcp --locked
cargo check -p gestalt-runtime --no-default-features --features skills --locked
cargo check -p gestalt-runtime --no-default-features --features verify --locked
cargo check -p gestalt-runtime --no-default-features --features trace --locked
```

- `cargo check -p gestalt-runtime --all-features --locked` passes.

### U4. Make Runtime Dependencies Truly Optional

Goal: ensure disabled integrations disappear from Cargo's normal dependency graph.

Files:

- `crates/gestalt-runtime/Cargo.toml`
- `Cargo.lock`

Work:

- Mark integration-only dependencies `optional = true`.
- Connect each optional dependency with explicit `dep:` feature syntax.
- Remove dependencies that relocation proves unused.
- Do not add a dependency solely to inspect the graph; use Cargo metadata and `cargo tree`.
- Keep `dev-dependencies` separate from the normal graph and ensure tests do not accidentally force production features.

Minimum deny list for `gestalt-runtime --no-default-features`:

```text
encoding_rs
eventsource-stream
pulldown-cmark
regex
reqwest
serde_yaml
similar
tokio-stream
toml
walkdir
```

Also deny OpenTelemetry packages while `otel` is unimplemented. Expand this list if feature ownership identifies other integration-only packages.

Acceptance:

```bash
cargo tree -p gestalt-runtime --no-default-features --edges normal --prefix none
cargo tree -p gestalt-runtime --no-default-features --features providers --edges normal --prefix none
cargo tree -p gestalt-runtime --no-default-features --features tools --edges normal --prefix none
```

- The minimal tree contains none of the deny-listed package roots.
- The provider tree contains the HTTP/streaming stack.
- The tools tree contains tool parsing/encoding and HTTP dependencies required by the built-in `web_fetch` tool, but not provider-only streaming dependencies.

### U5. Forward Product Features Without Restoring Runtime Bloat

Goal: preserve product behavior while allowing minimal embeddings.

Files:

- `crates/gestalt-app/Cargo.toml`
- `crates/gestalt-cli/Cargo.toml`
- `crates/gestalt-tui/Cargo.toml`
- feature-sensitive app, CLI, and TUI modules/tests

Work:

- Add `providers`, `tools`, and `trace` forwarding features to app and CLI.
- Make CLI defaults enable `providers`, `tools`, `trace`, `mcp`, `skills`, and `verify`.
- Keep runtime dependencies declared with `default-features = false`.
- Gate app services that directly use optional runtime APIs.
- Keep CLI command definitions available in minimal builds and gate dispatch implementations.
- Make TUI dependencies enable the full product feature set explicitly; TUI does not need a speculative minimal profile.
- Remove all `otel` feature forwarding and the entry in `gestalt-app/src/runtime_factory.rs`.

Acceptance:

```bash
cargo check -p gestalt-app --no-default-features --locked
cargo check -p gestalt-cli --no-default-features --locked
cargo check -p gestalt-cli --all-features --locked
cargo check -p gestalt-tui --locked
```

- Default CLI and TUI tests retain current behavior.
- Minimal CLI integration commands return a typed disabled-feature error.

### U6. Add Permanent Guardrails

Goal: make the intended boundary mechanically enforceable.

Files:

- `scripts/check-deps.sh`
- `.github/workflows/ci.yml`

Work:

- Fail when `crates/gestalt-runtime/src/legacy` exists.
- Fail on `#[path = "legacy/..."]`, `mod legacy_*`, `crate::legacy_*`, or `pub use legacy_*` in runtime source. Do not reject arbitrary fixture strings containing the word "legacy".
- Check the minimal runtime dependency deny list using package-root lines from `cargo tree`; do not substring-match transitive paths.
- Add CI checks for runtime minimal, each single feature, all features, CLI minimal, and workspace all-features.
- Keep the existing five-package and forbidden path-dependency checks.

Acceptance:

```bash
bash scripts/check-deps.sh
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p gestalt-runtime --no-default-features --locked
cargo check -p gestalt-cli --no-default-features --locked
```

All commands pass.

## Implementation Order

1. U1: relocate foundation modules.
2. U2: relocate integrations and delete `legacy/`.
3. U3: break compile-time coupling and add `cfg` boundaries.
4. U4: mark dependencies optional and validate feature isolation.
5. U5: forward features through products and preserve default behavior.
6. U6: add CI and source/dependency guardrails.

Keep relocation commits separate from feature-gating commits. This makes failures attributable and avoids reviewing path churn mixed with conditional behavior.

## Definition of Done

- `crates/gestalt-runtime/src/legacy/` does not exist.
- Runtime source contains no legacy path mounts or internal `legacy_*` module names.
- Runtime modules have clear owners: `context`, `exec`, `mcp`, `policy`, `providers`, `skills`, `tools`, `tool_output`, `trace`, and `verify`.
- `gestalt-runtime --no-default-features` excludes every optional integration module and its integration-only dependencies.
- Every supported runtime feature compiles from the minimal base.
- App and CLI forward features without enabling runtime defaults accidentally.
- Default CLI and TUI behavior remains unchanged.
- The empty `otel` capability marker is removed; its real implementation remains tracked by P3.6.
- Dependency and layout regressions fail locally through `scripts/check-deps.sh` and in CI.
