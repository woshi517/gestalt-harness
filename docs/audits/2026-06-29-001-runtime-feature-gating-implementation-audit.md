---
title: "Audit: Runtime feature gating and legacy removal implementation"
status: complete
created: 2026-06-29
audited_branch: ref/crate-boundary
audited_commit: 505bc09
source_plan: "docs/plans/2026-06-29-001-ref-runtime-feature-gating-and-legacy-removal.md"
---

# Runtime Feature Gating and Legacy Removal Implementation Audit

## Executive Summary

The implementation is not ready to merge against the source plan's definition of done.

The module relocation work is substantially complete: the runtime no longer has a `src/legacy/`
directory, legacy path mounts are gone, and the all-feature runtime test suite passes. The feature
boundary work is incomplete. Runtime feature flags currently hide source modules but do not remove
their dependency trees, the minimal app and CLI profiles do not compile, the trace-disabled context
compaction path changes runtime behavior, and the required CI guardrails were not added.

Implementation-unit status:

| Unit | Status | Summary |
| --- | --- | --- |
| U1: Relocate foundation modules | Complete | Foundation modules have permanent owners and no legacy mounts remain. |
| U2: Relocate optional integrations | Complete | Integration modules were moved and `src/legacy/` was removed. |
| U3: Separate base types from integrations | Partial | Runtime modules are conditionally compiled, but the base profile has behavioral defects, warnings, and incomplete test/bin gating. |
| U4: Make dependencies optional | Not implemented | Integration dependencies remain unconditional and all deny-listed roots remain in the minimal tree. |
| U5: Forward product features | Partial | Features are forwarded, but the required minimal app and CLI profiles fail to compile. |
| U6: Add permanent guardrails | Not implemented | Dependency, source-layout, and compile-matrix checks were not added to the script or CI. |

## Scope and Method

This audit reviewed:

- the source plan and every implementation unit and acceptance criterion;
- changes from `origin/ref/crate-boundary` through commit `505bc09`;
- `gestalt-runtime`, its modules, and its feature-specific call paths;
- feature forwarding in `gestalt-app`, `gestalt-cli`, and `gestalt-tui`;
- dependency declarations and the minimal normal dependency tree;
- local boundary scripts and GitHub Actions configuration;
- code quality, unnecessary complexity, and avoidable work in the runtime crate.

The review was read-only. No implementation files were changed.

## Positive Results

The following work is correct and should be retained:

- `crates/gestalt-runtime/src/legacy/` no longer exists.
- Runtime source has no architectural `legacy_*` module mounts or `#[path = "legacy/..."]`
  attributes.
- Foundation and integration code now resides under `context`, `exec`, `mcp`, `policy`,
  `providers`, `skills`, `tools`, `trace`, and `verify`.
- Runtime default features were changed to `providers`, `tools`, and `trace`.
- `verify` correctly declares a feature dependency on `tools`.
- App and CLI defaults list the full product feature set.
- TUI explicitly enables the full product feature set.
- All six runtime single-feature profiles compile when warnings are permitted.
- `cargo test -p gestalt-runtime --all-features --locked` passes.

These results satisfy most of U1 and U2 and establish a useful base for completing U3 through U6.

## Findings

### F1 — High: Minimal app and CLI profiles do not compile

Evidence:

- `cargo check -p gestalt-cli --no-default-features --locked` fails while compiling
  `gestalt-app`, producing 89 unresolved import, type, and function errors.
- `gestalt-app/src/lib.rs` compiles all product modules unconditionally.
- App modules directly reference APIs gated behind runtime `providers`, `tools`, `skills`,
  `verify`, and `trace` features.
- `build_app_runtime` unconditionally constructs a provider, built-in tool registry, verification
  hooks, skill discovery, and trace evaluator.

Representative locations:

- `crates/gestalt-app/src/lib.rs:30-45`
- `crates/gestalt-app/src/runtime_factory.rs:27-36`
- `crates/gestalt-app/src/runtime_factory.rs:270-296`
- `crates/gestalt-app/src/run.rs`
- `crates/gestalt-app/src/sessions.rs`
- `crates/gestalt-app/src/verify.rs`

Impact:

- U5 acceptance fails.
- The existing CI `Build minimal CLI` step would fail on this branch.
- Disabled integrations cannot return the required typed `FeatureDisabled` error because the
  product cannot be built without those integrations.

Recommendation:

1. Keep command definitions and configuration parsing feature-neutral.
2. Gate app services and dispatch implementations that directly use optional runtime APIs.
3. Return `ConfigError::FeatureDisabled` from disabled command dispatch paths.
4. Gate trace-only run/session persistence code behind `trace`.
5. Gate provider resolution, authentication adapters, and model catalogs behind `providers`.
6. Gate built-in tool construction behind `tools`, skill operations behind `skills`, and verifier
   construction behind `verify`.
7. Add minimal CLI integration tests proving commands remain visible and return
   `FEATURE_DISABLED`.

Required verification:

```bash
cargo check -p gestalt-app --no-default-features --locked
cargo check -p gestalt-cli --no-default-features --locked
cargo test -p gestalt-cli --no-default-features --locked
```

### F2 — High: Runtime dependencies are not optional

Evidence:

`crates/gestalt-runtime/Cargo.toml` declares all integration dependencies unconditionally, and its
features contain no `dep:` entries. The minimal normal dependency tree still contains every package
root from the plan's deny list:

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

Representative location:

- `crates/gestalt-runtime/Cargo.toml:13-52`

Impact:

- The primary goal of U4 is not implemented.
- `--no-default-features` does not produce a minimal embedding profile.
- HTTP/TLS, parser, filesystem-walking, and formatting dependencies are still compiled and shipped
  even when their integrations are disabled.
- Feature declarations communicate isolation that Cargo does not enforce.

Recommendation:

Mark integration-only dependencies optional and connect them to the narrowest feature that directly
uses them:

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

This is a starting map. Confirm ownership with `rg` and the single-feature compile matrix. Keep only
dependencies used by the feature-neutral runtime unconditional.

Required verification:

```bash
cargo tree -p gestalt-runtime --no-default-features --edges normal --prefix none
cargo tree -p gestalt-runtime --no-default-features --features providers --edges normal --prefix none
cargo tree -p gestalt-runtime --no-default-features --features tools --edges normal --prefix none
```

The minimal tree must contain none of the deny-listed package roots.

### F3 — High: Trace-disabled context compaction inserts an empty checkpoint

Evidence:

`context::CompactionCheckpoint` has two feature-dependent definitions:

- with `trace`, it is re-exported from `trace::context_artifacts` and has a complete Markdown
  renderer;
- without `trace`, it is duplicated in `context` and `render_markdown()` returns `String::new()`.

The compaction path calls this renderer and inserts its result as the system checkpoint message.
Without `trace`, successful compaction therefore replaces history with an empty system message.
Subsequent compactions also receive an empty rendering of the previous checkpoint.

The path additionally creates an artifact reference for the checkpoint even though checkpoint
persistence is omitted when `trace` is disabled.

Representative locations:

- `crates/gestalt-runtime/src/context/mod.rs:85-119`
- `crates/gestalt-runtime/src/context/mod.rs:443-518`
- `crates/gestalt-runtime/src/context/mod.rs:1386-1399`
- `crates/gestalt-runtime/src/trace/context_artifacts.rs:8-88`

Impact:

- Base runtime behavior differs from the all-feature runtime.
- Context compaction can discard the generated checkpoint summary.
- The result contradicts the plan's requirement to preserve feature-neutral context construction
  and move shared data primitives out of trace persistence.
- All-feature tests cannot detect this defect because they compile the other type definition.

Recommendation:

Move `CompactionCheckpoint`, `ProjectionManifest`, and `MessageMetadataRef` into a feature-neutral
`context::projection` module. Keep `render_markdown()` with the canonical checkpoint type. Make
`trace::context_artifacts` consume and re-export those types while owning only filesystem
persistence.

Only attach an artifact reference when persistence produced an artifact, or make the reference
explicitly optional based on the configured durability path.

Required verification:

- Add one no-default-features context compaction test.
- Assert that the generated checkpoint message is non-empty and begins with
  `### Session Checkpoint Summary`.
- Assert that trace-disabled compaction does not advertise a nonexistent persisted artifact.

### F4 — Medium: Required clippy check fails

Evidence:

```text
error: module has the same name as its containing module
crates/gestalt-runtime/src/tools/mod.rs:12
pub mod tools;
```

Command:

```bash
cargo clippy -p gestalt-runtime --all-targets --all-features --locked -- -D warnings
```

The relocation created `tools::tools`, which violates the workspace's denied
`clippy::module_inception` lint.

Impact:

- U6 and the definition-of-done clippy criterion fail.
- The relocated module layout adds an unnecessary navigation layer.

Recommendation:

Merge `tools/tools.rs` into `tools/mod.rs`. Its child modules already belong directly to `tools`, and
no external caller requires a `tools::tools` path.

### F5 — Medium: Minimal runtime fails under the repository warning policy

Evidence:

`cargo check -p gestalt-runtime --no-default-features --locked` succeeds with 12 warnings.
With the CI policy applied, it fails:

```bash
RUSTFLAGS="-D warnings" \
  cargo check -p gestalt-runtime --no-default-features --locked
```

Reported issues include:

- feature-specific `Arc` and `Mutex` imports;
- an unused `mcp_server_names` set;
- unnecessary mutable bindings used only by enabled-feature branches;
- a dummy `Option<()>` skill-state binding;
- a trace-only `snapshot_path`;
- an MCP-only match binding;
- feature-specific helper functions compiled when their only caller is absent.

Representative locations:

- `crates/gestalt-runtime/src/activation.rs:420`
- `crates/gestalt-runtime/src/activation.rs:1091`
- `crates/gestalt-runtime/src/activation.rs:1213`
- `crates/gestalt-runtime/src/builder.rs:285`
- `crates/gestalt-runtime/src/builder.rs:489-498`
- `crates/gestalt-runtime/src/composition_hooks.rs:225-240`
- `crates/gestalt-runtime/src/composition_hooks.rs:1361`
- `crates/gestalt-runtime/src/tool_catalog.rs:111`
- `crates/gestalt-runtime/src/tool_catalog_planner.rs:4`

Impact:

- A correct minimal CI job with `-D warnings` would fail.
- Conditional compilation currently leaves dead scaffolding in several profiles.

Recommendation:

Apply `#[cfg]` to the smallest complete declaration or block:

- gate imports with the features that use them;
- gate complete local bindings instead of introducing dummy `Option<()>` values;
- construct mutable values inside feature-specific blocks or use shadowing only when needed;
- gate helpers with their sole consumer's feature;
- use feature-specific match arms so disabled arms do not bind unused fields.

Do not suppress these warnings globally.

### F6 — Medium: Permanent dependency and layout guardrails are missing

Evidence:

`scripts/check-deps.sh` still checks only:

- the five-package workspace set;
- path-dependency boundaries;
- absence of TUI dependencies in the minimal CLI tree;
- old compatibility crate aliases.

It does not check:

- whether `crates/gestalt-runtime/src/legacy` exists;
- architectural legacy module/path patterns;
- the runtime minimal dependency deny list;
- package-root matching from `cargo tree`.

The CI workflow does not run:

- the runtime minimal profile;
- each runtime single feature;
- runtime or workspace all-features;
- CLI all-features;
- clippy with all targets and all features.

Representative locations:

- `scripts/check-deps.sh:1-63`
- `.github/workflows/ci.yml:39-58`

Impact:

- U6 is not implemented.
- `bash scripts/check-deps.sh` reports success despite U4 being entirely unsatisfied.
- A future legacy layout or dependency regression would not be detected.

Recommendation:

Extend `scripts/check-deps.sh` with exact structural checks and package-root dependency matching.
Extend CI with the plan's compile matrix. Keep the checks direct; no new dependency inspection tool
is needed.

### F7 — Medium: The unimplemented `otel` marker remains

Evidence:

- `gestalt-runtime` still declares `otel = []`.
- `gestalt-app` still forwards `otel = ["gestalt-runtime/otel"]`.
- `build_app_runtime` still conditionally reports `"otel"` as an enabled host feature.

Representative locations:

- `crates/gestalt-runtime/Cargo.toml:52`
- `crates/gestalt-app/Cargo.toml:43`
- `crates/gestalt-app/src/runtime_factory.rs:67-68`

Impact:

- The explicit U5 requirement to remove the empty capability is incomplete.
- Runtime feature reporting can claim support for an integration that does not exist.

Recommendation:

Remove `otel` from runtime, app, feature forwarding, and feature reporting. Reintroduce it only when
P3.6 adds a real module and optional OpenTelemetry dependencies.

### F8 — Medium: Formatting check fails

Evidence:

```bash
cargo fmt --all --check
```

reports formatting changes across the feature-gating implementation, including `activation.rs`,
`builder.rs`, `context/mod.rs`, `lib.rs`, `runtime.rs`, and relocated modules.

Impact:

- The first CI formatting step fails.
- Review noise obscures semantic changes.

Recommendation:

Run `cargo fmt --all`, inspect the resulting diff, and keep formatting changes in the current
feature-gating commit rather than mixing them into later behavioral fixes.

### F9 — Low: Feature-specific integration targets are not gated

Evidence:

- `mock_mcp_server` has no `required-features = ["mcp"]` binary declaration.
- Runtime integration test crates do not have crate-level feature gates.

Representative locations:

- `crates/gestalt-runtime/src/bin/mock_mcp_server.rs`
- `crates/gestalt-runtime/Cargo.toml`
- `crates/gestalt-runtime/tests/runtime_mcp_tests.rs`
- `crates/gestalt-runtime/tests/runtime_mcp_findings_tests.rs`
- `crates/gestalt-runtime/tests/skill_activation_tests.rs`

Impact:

- Feature-specific targets are considered in profiles where they cannot be useful.
- `cargo test --no-default-features` cannot become a reliable base-profile check until tests are
  assigned to their required features.

Recommendation:

Add a `[[bin]]` declaration with `required-features = ["mcp"]` and crate-level `#![cfg(...)]`
attributes to integration tests that exclusively exercise optional features.

### F10 — Low: Internal code still routes through compatibility aliases

Evidence:

The plan requires internal callers to use owning module paths, but internal MCP and skill modules
still import aliases exposed from the crate root:

- `crate::mcp_error`
- `crate::model`
- `crate::transport`
- `crate::skill_manifest`

`lib.rs` also adds a `skill_contributor` compatibility module used by one runtime integration test.

Representative locations:

- `crates/gestalt-runtime/src/lib.rs:28-31`
- `crates/gestalt-runtime/src/lib.rs:76-97`
- `crates/gestalt-runtime/src/mcp/client.rs:5-11`
- `crates/gestalt-runtime/src/mcp/registry.rs`
- `crates/gestalt-runtime/src/skills/discovery.rs:1`
- `crates/gestalt-runtime/tests/skill_activation_tests.rs:22`

Impact:

- Internal ownership remains less explicit than the target module map.
- Compatibility exports become harder to remove because implementation code depends on them.

Recommendation:

Use `super::error`, `super::model`, `super::transport`, and `super::manifest` inside owning modules.
Update the test to use `gestalt_runtime::skills::contributor::SkillContributorState`, then remove the
`skill_contributor` compatibility module. Retain root exports only for confirmed external API
requirements.

## Code Quality and Efficiency Audit

### Unnecessary complexity

`ToolCatalogPlanner::plan` sorts descriptors and then calls `plan_descriptors`, which sorts the same
vector again.

Location:

- `crates/gestalt-runtime/src/tool_catalog_planner.rs:76-85`

Recommendation:

Remove the first sort and keep ordering enforcement in `plan_descriptors`, the shared entry point.

### Feature-gating scaffolding

Several disabled-feature paths introduce placeholder values solely to satisfy later shared code,
for example `let skill_state_handle: Option<()> = None`. This is both noisier and responsible for
strict-build failures.

Recommendation:

Gate the complete consumer path. Avoid placeholder types whose only purpose is making a disabled
feature compile.

### Duplicate source-of-truth types

The feature-dependent `CompactionCheckpoint` definitions are the largest avoidable complexity in
the current runtime. Besides duplicating approximately 25 lines of model definition, they have
already diverged behaviorally.

Recommendation:

One feature-neutral model and renderer; trace owns only I/O.

### Broad exports

`lib.rs` broadly re-exports every optional module with `pub use module::*`, then adds named aliases
for many submodules. This maximizes the public API surface and makes feature ownership unclear.

Recommendation:

After required external consumers are identified, replace broad exports with the smallest named
API set. Do not break established public consumers solely for aesthetic cleanup; remove aliases
that are used only internally or by repository tests.

### Relocation artifact

`tools::tools` is a directory-layout artifact rather than a meaningful domain boundary.

Recommendation:

Merge it into `tools`; the resulting module is shorter, clearer, and clippy-compliant.

## Ponytail Complexity Findings

```text
context/mod.rs:L85-119: delete: duplicated checkpoint model with an empty renderer. One feature-neutral checkpoint type.
tools/mod.rs:L12: shrink: tools::tools module inception. Merge tools.rs into mod.rs.
lib.rs:L28-31: delete: skill_contributor compatibility module used by one test. Update the test import.
tool_catalog_planner.rs:L76-85: shrink: descriptors are sorted twice. Keep the sort in plan_descriptors.
activation.rs:L948-949: delete: duplicated section comment. Nothing replaces it.
builder.rs:L285: delete: dummy Option<()> feature placeholder. Gate its consumers directly.
```

`net: -45 lines possible.`

## Acceptance-Criteria Matrix

| Criterion | Result | Evidence |
| --- | --- | --- |
| No runtime `src/legacy/` directory | Pass | Directory does not exist. |
| No architectural legacy mounts/names | Pass | Focused `rg` returns no matches. |
| Runtime all-feature tests | Pass | All unit and integration tests completed successfully. |
| Runtime minimal check | Partial | Compiles with warnings; fails under CI `-D warnings`. |
| Every runtime single feature compiles | Pass with warnings | All six feature checks exit successfully. |
| Runtime all-feature check | Pass | Covered by successful all-feature test build. |
| Minimal tree excludes integration dependencies | Fail | Every deny-listed package root remains. |
| App minimal check | Fail | Optional runtime APIs are referenced unconditionally. |
| CLI minimal check | Fail | Fails with 89 app errors. |
| CLI all-feature check | Not run | No conclusion drawn; run after the minimal app boundary is repaired. |
| TUI full product profile | Manifest only | Explicit full feature set is present; the TUI acceptance command was not run. |
| Disabled CLI integrations return typed error | Fail/unreachable | Minimal CLI does not compile. |
| `otel` marker removed | Fail | Runtime and app still declare/report it. |
| Dependency/layout guardrails | Fail | Script and CI were not extended. |
| Formatting | Fail | `cargo fmt --all --check` reports diffs. |
| Clippy all targets/all features | Fail | `tools::tools` module inception. |

## Recommended Remediation Order

The following order minimizes rework and keeps failures attributable:

1. **Fix the feature-neutral context model.**
   Move shared projection/checkpoint types and rendering into `context::projection`; leave trace I/O
   in `trace`.
2. **Make runtime dependencies optional.**
   Add `optional = true` and explicit `dep:` feature ownership, then validate the dependency trees.
3. **Clean the runtime compile matrix.**
   Gate complete declarations and blocks until every profile passes with `-D warnings`.
4. **Fix the tools module layout.**
   Merge `tools/tools.rs` into `tools/mod.rs`.
5. **Gate app product services.**
   Make the app minimal profile compile while retaining feature-neutral configuration and command
   contracts.
6. **Implement typed disabled-feature dispatch.**
   Keep CLI commands visible and return `ConfigError::FeatureDisabled`.
7. **Remove `otel`.**
   Delete the empty marker and reporting path.
8. **Gate optional targets and tests.**
   Add binary `required-features` and crate-level integration-test gates.
9. **Add guardrails.**
   Extend `check-deps.sh` and CI only after the intended boundaries pass locally.
10. **Format and run the complete definition of done.**

Avoid adding facade traits, factories, or placeholder no-op implementations solely to bridge feature
profiles. Conditional compilation at the actual integration boundary is sufficient.

## Required Final Verification

Run the following from a clean worktree:

```bash
cargo fmt --all --check
bash scripts/check-deps.sh

cargo check -p gestalt-runtime --no-default-features --locked
cargo check -p gestalt-runtime --no-default-features --features providers --locked
cargo check -p gestalt-runtime --no-default-features --features tools --locked
cargo check -p gestalt-runtime --no-default-features --features mcp --locked
cargo check -p gestalt-runtime --no-default-features --features skills --locked
cargo check -p gestalt-runtime --no-default-features --features verify --locked
cargo check -p gestalt-runtime --no-default-features --features trace --locked
cargo check -p gestalt-runtime --all-features --locked

cargo check -p gestalt-app --no-default-features --locked
cargo check -p gestalt-cli --no-default-features --locked
cargo check -p gestalt-cli --all-features --locked
cargo check -p gestalt-tui --locked

cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

Also verify the minimal dependency tree programmatically against exact package-root lines. A passing
compile alone does not prove dependency isolation.

## Merge Recommendation

**Do not merge in the current state.**

Re-audit after F1 through F8 are resolved and the complete verification block passes. F9 and F10
should be completed in the same feature-boundary change because both are explicit source-plan
requirements and are small once the main boundaries are correct.
