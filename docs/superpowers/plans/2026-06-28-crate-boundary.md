# Crate Boundary Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate Gestalt into five coherent crates while preserving CLI/TUI behavior, keeping `gestalt-core` I/O-free, and making runtime and app embedding explicit.

**Architecture:** Rename the published CLI package to `gestalt-cli` while keeping its installed binary named `gestalt`. Add `gestalt-app` for reusable product services and `gestalt-tui` for the standalone terminal binary; bare `gestalt` delegates to `gestalt-tui`, while explicit CLI subcommands continue to dispatch normally. Move implementation crates into `gestalt-runtime` in dependency order, then make optional integrations Cargo features and enforce the final graph in the existing dependency audit.

**Tech Stack:** Rust 2021, Cargo workspace/features, Tokio, Clap, Ratatui, Crossterm, Bash/JQ dependency audit.

---

## Decisions And Resolved Gaps

- Rename the package from `gestalt-harness` to `gestalt-cli`, preserve the `gestalt` executable, and update package metadata tests, release scripts, docs, and CI in the same migration.
- Make bare `gestalt` the default TUI entrypoint. Preserve `gestalt tui` as an explicit alias without adding a forbidden CLI-to-TUI dependency. Both paths launch the separately installed `gestalt-tui` executable and return an actionable error when it is absent.
- Put workspace examples under the owning packages, because the workspace root is a virtual manifest: `crates/gestalt-runtime/examples/embed_runtime.rs` and `crates/gestalt-app/examples/embed_app.rs`.
- Keep `providers`, built-in `tools`, and tracing in the runtime's default profile. Gate `mcp`, `skills`, `verify`, and `otel`; forward those features through app and CLI.
- Make the existing default CLI retain current commands by enabling `mcp`, `skills`, and `verify`. `--no-default-features` is the minimal profile.
- Move app report data types out of CLI output code. `gestalt-app` owns serializable report models; `gestalt-cli` owns rendering and process exit behavior.
- Do not preserve facade crates after their migration commit. Temporary re-exports hide incomplete migrations and defeat the five-crate acceptance criterion.
- Treat current `gestalt-core` filesystem and subprocess use as an implementation gap. Move `GitWorkspaceSnapshotter` to runtime and make concrete tools return fully materialized `ToolExecutionResult` values so core never writes artifacts.

## Final File Map

```text
crates/gestalt-core/
  src/snapshot.rs                 # WorkspaceSnapshot and WorkspaceSnapshotter trait only
  src/tool.rs                     # Tool traits and data types only

crates/gestalt-runtime/
  src/context/                    # former gestalt-context plus runtime context orchestration
  src/exec/                       # former gestalt-exec
  src/mcp/                        # former gestalt-mcp
  src/policy/                     # former gestalt-policy
  src/providers/                  # former gestalt-models
  src/skills/                     # former gestalt-skills
  src/tools/                      # former gestalt-tools
  src/trace/                      # former gestalt-trace
  src/verify/                     # former gestalt-verify
  src/workspace_snapshot.rs       # GitWorkspaceSnapshotter

crates/gestalt-app/
  src/config.rs
  src/auth.rs
  src/catalog.rs
  src/models.rs
  src/profiles.rs
  src/runtime_factory.rs
  src/run.rs
  src/runs.rs
  src/sessions.rs
  src/workspace.rs
  src/reports.rs

crates/gestalt-cli/
  src/main.rs                     # Clap definitions and dispatch
  src/output.rs                   # text/JSON rendering
  src/approval.rs                 # CLI terminal approval
  src/chat.rs                     # CLI line-oriented shell
  src/slash.rs                    # CLI chat command parsing

crates/gestalt-tui/
  src/main.rs                     # standalone binary/config bootstrap
  src/lib.rs
  src/app.rs
  src/approval.rs
  src/bridge.rs
  src/services.rs
  src/state.rs
  src/update.rs
  src/screens/
  src/widgets/
```

### Task 1: Rename The CLI Package

**Files:**
- Modify: `scripts/check-deps.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `crates/gestalt-cli/Cargo.toml`
- Modify: `crates/gestalt-cli/tests/package_metadata_tests.rs`
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/release-checklist.md`
- Test: `crates/gestalt-cli/tests/package_metadata_tests.rs`

- [ ] **Step 1: Change the package contract test first**

Change the package assertion in `package_metadata_tests.rs`:

```rust
#[test]
fn package_metadata_matches_public_install_identity() {
    assert!(
        CARGO_TOML.contains("name = \"gestalt-cli\""),
        "package name should be gestalt-cli"
    );
    assert!(
        CARGO_TOML.contains("name = \"gestalt\""),
        "binary name should remain gestalt"
    );
}
```

Run: `cargo test -p gestalt-harness --test package_metadata_tests`

Expected: FAIL because the current package name is `gestalt-harness`.

- [ ] **Step 2: Rename the CLI package without renaming its binary**

Change only the package identity in `crates/gestalt-cli/Cargo.toml`; keep the library and binary targets:

```toml
[package]
name = "gestalt-cli"

[lib]
name = "gestalt_cli"
path = "src/lib.rs"

[[bin]]
name = "gestalt"
path = "src/main.rs"
```

Update the README publishing note, changelog package references, release checklist, and the `scripts/check-deps.sh` package budget key from `gestalt-harness` to `gestalt-cli`. Product prose may continue to call the project `gestalt-harness`; Cargo package/install references must use `gestalt-cli`.

Run: `cargo test -p gestalt-cli --test package_metadata_tests`

Expected: PASS and confirm that the installed executable remains `gestalt`.

- [ ] **Step 3: Add the minimal profile to CI**

Add these steps after the workspace build:

```yaml
      - name: Build minimal CLI
        run: cargo check -p gestalt-cli --no-default-features

```

- [ ] **Step 4: Verify the rename and current boundary audit**

Run: `cargo test -p gestalt-cli --test package_metadata_tests`

Run: `bash scripts/check-deps.sh`

Expected: both PASS.

- [ ] **Step 5: Record the current green baseline**

Run: `cargo test --workspace --locked`

Expected: PASS. The pre-rename baseline on 2026-06-28 passes.

- [ ] **Step 6: Commit**

```bash
git add scripts/check-deps.sh .github/workflows/ci.yml crates/gestalt-cli/Cargo.toml crates/gestalt-cli/tests/package_metadata_tests.rs README.md CHANGELOG.md docs/release-checklist.md
git commit -m "refactor: rename cli package and define crate boundaries"
```

### Task 2: Introduce `gestalt-app` And Separate Reports From Rendering

**Files:**
- Create: `crates/gestalt-app/Cargo.toml`
- Create: `crates/gestalt-app/src/lib.rs`
- Create: `crates/gestalt-app/src/reports.rs`
- Modify: `Cargo.toml`
- Modify: `crates/gestalt-cli/src/output.rs`
- Test: move service-level tests from `crates/gestalt-cli/tests/` to `crates/gestalt-app/tests/`

- [ ] **Step 1: Add a failing app report contract test**

Create `crates/gestalt-app/tests/report_contract_tests.rs`:

```rust
use gestalt_app::reports::{ConnectReport, RunIndexEntry, RunsListReport};
use serde::Serialize;

#[test]
fn app_reports_serialize_without_cli_types() {
    fn assert_serializable<T: Serialize>() {}

    assert_serializable::<ConnectReport>();
    assert_serializable::<RunIndexEntry>();
    assert_serializable::<RunsListReport>();
}
```

Run: `cargo test -p gestalt-app --test report_contract_tests`

Expected: FAIL because the package does not exist.

- [ ] **Step 2: Create the package and public module surface**

Use this initial manifest:

```toml
[package]
name = "gestalt-app"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lints]
workspace = true

[dependencies]
gestalt-core = { path = "../gestalt-core" }
gestalt-runtime = { path = "../gestalt-runtime" }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
```

Start `src/lib.rs` as:

```rust
pub mod reports;
```

- [ ] **Step 3: Move report structs, not renderers**

Move data-only report structs and their `Serialize` implementations from `crates/gestalt-cli/src/output.rs` into `crates/gestalt-app/src/reports.rs`. Leave `OutputFormat`, `JsonEnvelope`, `print_report`, `render_event`, and all terminal formatting in CLI. Import app reports in CLI with:

```rust
pub use gestalt_app::reports::*;
```

- [ ] **Step 4: Make the report contract pass**

Run: `cargo test -p gestalt-app --test report_contract_tests`

Expected: PASS.

- [ ] **Step 5: Move pure service tests to their owner**

Move config, model discovery, profile, run/session, and workspace service tests into `crates/gestalt-app/tests/`. Keep process-level command/output tests in `crates/gestalt-cli/tests/`.

Run: `cargo test -p gestalt-app`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/gestalt-app crates/gestalt-cli/src/output.rs crates/gestalt-cli/tests
git commit -m "refactor: introduce shared app service crate"
```

### Task 3: Move Reusable CLI Services Into `gestalt-app`

**Files:**
- Move: `crates/gestalt-cli/src/{auth,config,connect,doctor,model_cache,models,profiles,provider_catalog,providers,run,runtime,sessions,verify,workspace}.rs`
- Split: `crates/gestalt-cli/src/{context,runs}.rs`
- Modify: `crates/gestalt-app/src/lib.rs`
- Modify: `crates/gestalt-cli/src/{lib,main,chat,slash,output}.rs`
- Test: `crates/gestalt-app/tests/`, `crates/gestalt-cli/tests/`

- [ ] **Step 1: Move closed service modules**

Move the listed modules to `crates/gestalt-app/src/`, rename `provider_catalog.rs` to `catalog.rs`, and rename CLI-specific `build_cli_runtime` to `build_runtime`. Replace internal `crate::...` imports with app-local modules and external callers with `gestalt_app::...`.

During this task, move every non-presentation dependency used by those modules from `crates/gestalt-cli/Cargo.toml` to `crates/gestalt-app/Cargo.toml`, including the nine old implementation crates, `dirs`, `toml`, `toml_edit`, `keyring`, `rpassword`, `reqwest`, `chrono`, `uuid`, `sha2`, `schemars`, `async-trait`, and `tokio-util`. Remove each dependency from CLI when `rg` confirms no remaining CLI-owned module imports it. Task 8 replaces the old crate dependencies with runtime features.

`gestalt-app/src/lib.rs` must export:

```rust
pub mod auth;
pub mod catalog;
pub mod config;
pub mod connect;
pub mod doctor;
pub mod model_cache;
pub mod models;
pub mod profiles;
pub mod reports;
pub mod run;
pub mod runtime_factory;
pub mod sessions;
pub mod verify;
pub mod workspace;
```

- [ ] **Step 2: Split mixed presentation/service modules**

Move path resolution, indexing, pruning, deletion, and summary logic from `runs.rs` to `gestalt-app::runs`. Keep `tail` printing and `OutputFormat` branching in CLI.

Move context inspection computation to `gestalt-app::context`; return `ContextExplainReport` rather than printing it.

- [ ] **Step 3: Keep CLI ownership narrow**

Reduce `crates/gestalt-cli/src/lib.rs` to presentation modules:

```rust
pub mod approval;
pub mod chat;
pub mod cost;
pub mod export;
pub mod output;
pub mod policy;
pub mod replay;
pub mod slash;
pub mod tools;
pub mod trace;
```

- [ ] **Step 4: Verify both layers**

Run: `cargo test -p gestalt-app`

Expected: PASS.

Run: `cargo test -p gestalt-cli`

Expected: PASS with unchanged CLI golden/output contracts.

- [ ] **Step 5: Commit**

```bash
git add crates/gestalt-app crates/gestalt-cli
git commit -m "refactor: move application services out of cli"
```

### Task 4: Extract The Standalone TUI

**Files:**
- Create: `crates/gestalt-tui/Cargo.toml`
- Create: `crates/gestalt-tui/src/main.rs`
- Move: `crates/gestalt-cli/src/tui/**`
- Move: `crates/gestalt-cli/tests/tui_*`
- Modify: `crates/gestalt-cli/src/main.rs`
- Create: `crates/gestalt-cli/tests/default_entrypoint_tests.rs`
- Modify: `Cargo.toml`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add a failing package isolation test**

Create `crates/gestalt-tui/tests/package_boundary_tests.rs`:

```rust
#[test]
fn tui_package_does_not_link_cli_library() {
    let manifest = include_str!("../Cargo.toml");
    assert!(!manifest.contains("gestalt-cli"));
}
```

Run: `cargo test -p gestalt-tui --test package_boundary_tests`

Expected: FAIL because the package does not exist.

- [ ] **Step 2: Create the TUI manifest**

```toml
[package]
name = "gestalt-tui"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "gestalt-tui"
path = "src/main.rs"

[lints]
workspace = true

[dependencies]
gestalt-app = { path = "../gestalt-app" }
gestalt-core = { path = "../gestalt-core" }
gestalt-runtime = { path = "../gestalt-runtime" }
ratatui = { workspace = true }
crossterm = { workspace = true }
clap = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
```

- [ ] **Step 3: Move the TUI unchanged, then replace its service imports**

Move `crates/gestalt-cli/src/tui/**` under `crates/gestalt-tui/src/`. Replace `crate::{auth,config,connect,context,run,runs,sessions,verify}` with `gestalt_app::{...}`. Move the three TUI tests and remove `#[cfg(feature = "tui")]` gates so CI actually executes them.

- [ ] **Step 4: Make TUI the default `gestalt` action**

Make the top-level Clap subcommand optional. When it is `None` or explicitly `Some(Tui)`, call the same launcher using `std::process::Command`; all other subcommands keep their current dispatch. Resolve the executable from `GESTALT_TUI_BIN` when set, otherwise use `gestalt-tui`. Forward workspace, run target, prompt, and API-key arguments. Map `ErrorKind::NotFound` to:

```text
gestalt-tui is not installed; run `cargo install gestalt-tui`
```

Add `crates/gestalt-cli/tests/default_entrypoint_tests.rs`:

```rust
use std::process::Command;

#[test]
fn bare_gestalt_launches_tui() {
    let status = Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .env("GESTALT_TUI_BIN", "true")
        .status()
        .expect("gestalt should launch the configured TUI executable");

    assert!(status.success());
}

#[test]
fn help_does_not_launch_tui() {
    let output = Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .arg("--help")
        .env("GESTALT_TUI_BIN", "false")
        .output()
        .expect("gestalt --help should remain a CLI operation");

    assert!(output.status.success());
}
```

This makes TUI the default human entrypoint while retaining `gestalt tui`, `gestalt --help`, and every explicit CLI subcommand without creating a Cargo dependency.

- [ ] **Step 5: Verify extraction and minimal CLI**

Add the standalone TUI build after the workspace build in `.github/workflows/ci.yml`:

```yaml
      - name: Build standalone TUI
        run: cargo check -p gestalt-tui
```

Run: `cargo test -p gestalt-tui`

Expected: PASS, including the former gated tests.

Run: `cargo test -p gestalt-cli --test default_entrypoint_tests`

Expected: PASS.

Run: `cargo tree -p gestalt-cli --no-default-features --edges normal`

Expected: output contains neither `ratatui` nor `crossterm`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml .github/workflows/ci.yml crates/gestalt-tui crates/gestalt-cli
git commit -m "refactor: extract standalone tui crate"
```

### Task 5: Consolidate Leaf Implementation Crates Into Runtime

**Files:**
- Move: `crates/gestalt-exec` to `crates/gestalt-runtime/src/exec`
- Move: `crates/gestalt-policy` to `crates/gestalt-runtime/src/policy`
- Move: `crates/gestalt-models` to `crates/gestalt-runtime/src/providers`
- Move: `crates/gestalt-mcp` to `crates/gestalt-runtime/src/mcp`
- Move: `crates/gestalt-skills` to `crates/gestalt-runtime/src/skills`
- Modify: `crates/gestalt-runtime/{Cargo.toml,src/lib.rs}`
- Move corresponding tests to `crates/gestalt-runtime/tests/`

- [ ] **Step 1: Move one leaf at a time**

For each source crate, use this loop manually: move source and tests, add the module to runtime, replace `gestalt_<name>::` with `gestalt_runtime::<module>::`, remove the path dependency, run that module's former tests, then commit. Use these mappings:

```text
gestalt_exec::      -> gestalt_runtime::exec::
gestalt_policy::    -> gestalt_runtime::policy::
gestalt_models::    -> gestalt_runtime::providers::
gestalt_mcp::       -> gestalt_runtime::mcp::
gestalt_skills::    -> gestalt_runtime::skills::
```

Preserve the MCP mock server as `crates/gestalt-runtime/src/bin/mock_mcp_server.rs`.

- [ ] **Step 2: Verify after every leaf**

Run after each move: `cargo test -p gestalt-runtime`

Expected: PASS before proceeding to the next crate.

- [ ] **Step 3: Commit each independently**

```bash
git commit -m "refactor(runtime): absorb exec implementation"
git commit -m "refactor(runtime): absorb policy implementation"
git commit -m "refactor(runtime): absorb provider implementations"
git commit -m "refactor(runtime): absorb mcp integration"
git commit -m "refactor(runtime): absorb skills integration"
```

### Task 6: Consolidate Dependent Implementation Crates Into Runtime

**Files:**
- Move: `crates/gestalt-tools` to `crates/gestalt-runtime/src/tools`
- Move: `crates/gestalt-verify` to `crates/gestalt-runtime/src/verify`
- Move: `crates/gestalt-trace` to `crates/gestalt-runtime/src/trace`
- Move: `crates/gestalt-context` to `crates/gestalt-runtime/src/context`
- Modify all consumers and tests

- [ ] **Step 1: Move in dependency order**

Use exactly this order and import mapping:

```text
gestalt_tools::     -> gestalt_runtime::tools::
gestalt_verify::    -> gestalt_runtime::verify::
gestalt_trace::     -> gestalt_runtime::trace::
gestalt_context::   -> gestalt_runtime::context::
```

`tools` follows `exec`; `verify` follows `tools`; `trace` follows `policy`; `context` follows `trace`. Merge existing runtime context orchestration with former context primitives under submodules instead of creating duplicate `context` modules.

- [ ] **Step 2: Remove the stale core dev-dependency**

Delete `gestalt-verify` from `crates/gestalt-core/Cargo.toml`; no core source uses it.

- [ ] **Step 3: Verify after every move**

Run: `cargo test -p gestalt-runtime`

Expected: PASS after each source crate is removed.

Run: `rg 'gestalt_(exec|policy|models|mcp|skills|tools|verify|trace|context)' crates --glob '*.rs' --glob 'Cargo.toml'`

Expected: no old crate imports or dependencies.

- [ ] **Step 4: Commit each independently**

Use one commit per absorbed crate so regressions can be bisected.

### Task 7: Make `gestalt-core` Actually I/O-Free

**Files:**
- Modify: `crates/gestalt-core/src/{lib,snapshot,tool}.rs`
- Modify: `crates/gestalt-core/src/agent/executor.rs`
- Create: `crates/gestalt-runtime/src/workspace_snapshot.rs`
- Create: `crates/gestalt-runtime/src/tools/output.rs`
- Modify all `Tool` implementations and mocks
- Modify: `scripts/check-deps.sh`

- [ ] **Step 1: Add a source-level purity regression test**

Add this repository boundary check to `scripts/check-deps.sh`:

```bash
if rg -n \
  'std::fs|tokio::fs|tokio::process|std::process::Command|reqwest::' \
  crates/gestalt-core/src; then
  printf 'ERROR: gestalt-core contains concrete I/O\n' >&2
  exit 1
fi
```

Run: `bash scripts/check-deps.sh`

Expected: FAIL on `snapshot.rs` and `tool.rs`.

- [ ] **Step 2: Move the concrete snapshotter**

Keep only `WorkspaceSnapshot` and `WorkspaceSnapshotter` in core. Move `GitWorkspaceSnapshotter`, git subprocess execution, recursive file walking, and content hashing to `gestalt_runtime::workspace_snapshot`.

- [ ] **Step 3: Remove artifact I/O from the core tool contract**

Change:

```rust
async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
```

to:

```rust
async fn execute(
    &self,
    input: Value,
    ctx: &ToolContext,
) -> Result<ToolExecutionResult, ToolError>;
```

Move truncation, artifact persistence, metadata reads, and hashing into `gestalt_runtime::tools::output::materialize`. Built-in, MCP-backed, extension-backed, and test tools call that runtime helper before returning. Core's executor only handles timeout, cancellation, retry, and error classification.

- [ ] **Step 4: Verify purity and behavior**

Run: `bash scripts/check-deps.sh`

Expected: PASS for the core purity checks.

Run: `cargo test -p gestalt-core`

Expected: PASS.

Run: `cargo test -p gestalt-runtime`

Expected: PASS, including artifact spillover and snapshot tests.

- [ ] **Step 5: Commit**

```bash
git add crates/gestalt-core crates/gestalt-runtime scripts/check-deps.sh
git commit -m "refactor: enforce io-free core boundary"
```

### Task 8: Add Runtime Features And Embedding Examples

**Files:**
- Modify: `crates/gestalt-runtime/Cargo.toml`
- Modify: `crates/gestalt-app/Cargo.toml`
- Modify: `crates/gestalt-cli/Cargo.toml`
- Create: `crates/gestalt-runtime/examples/embed_runtime.rs`
- Create: `crates/gestalt-app/examples/embed_app.rs`
- Add `#[cfg(feature = "...")]` at optional module boundaries

- [ ] **Step 1: Define runtime features**

```toml
[features]
default = ["providers", "tools"]
providers = ["dep:reqwest", "dep:tokio-stream", "dep:eventsource-stream"]
tools = ["dep:encoding_rs", "dep:glob", "dep:libc", "dep:regex", "dep:similar", "dep:walkdir"]
mcp = []
skills = ["dep:serde_yaml"]
verify = ["tools", "dep:pulldown-cmark"]
otel = []
```

Mark only dependencies uniquely required by a gated module as optional. Do not gate trace persistence, policy, context, or exec; they are required runtime behavior.

- [ ] **Step 2: Forward app and CLI features**

Add to `crates/gestalt-app/Cargo.toml`:

```toml
[features]
default = ["mcp", "skills", "verify"]
mcp = ["gestalt-runtime/mcp"]
skills = ["gestalt-runtime/skills"]
verify = ["gestalt-runtime/verify"]
otel = ["gestalt-runtime/otel"]
```

Add to `crates/gestalt-cli/Cargo.toml`:

```toml
[features]
default = ["mcp", "skills", "verify"]
mcp = ["gestalt-app/mcp", "gestalt-runtime/mcp"]
skills = ["gestalt-app/skills", "gestalt-runtime/skills"]
verify = ["gestalt-app/verify", "gestalt-runtime/verify"]
otel = ["gestalt-app/otel", "gestalt-runtime/otel"]
```

Commands for disabled optional integrations must still parse and return a typed “feature not enabled” error.

- [ ] **Step 3: Add compile-checked examples**

The runtime example constructs `RuntimeConfig` and `AgentRuntimeBuilder` using local fake provider/tool implementations, with no CLI or app import. The app example loads fixture config and builds a runtime via `runtime_factory`, with no CLI or TUI import.

Run: `cargo check -p gestalt-runtime --example embed_runtime`

Expected: PASS.

Run: `cargo check -p gestalt-app --example embed_app`

Expected: PASS.

- [ ] **Step 4: Verify feature profiles**

Run: `cargo check -p gestalt-cli --no-default-features`

Expected: PASS.

Run: `cargo check -p gestalt-runtime --all-features`

Expected: PASS.

Run: `cargo tree -p gestalt-cli --no-default-features --edges normal`

Expected: no Ratatui or Crossterm.

- [ ] **Step 5: Commit**

```bash
git add crates/gestalt-runtime crates/gestalt-app crates/gestalt-cli
git commit -m "feat: define minimal runtime and app profiles"
```

### Task 9: Remove Old Members And Finalize Guardrails

**Files:**
- Modify: `Cargo.toml`
- Modify: `scripts/check-deps.sh`
- Modify: `Cargo.lock`
- Delete: the nine absorbed crate directories

- [ ] **Step 1: Make the audit require exactly five workspace packages**

Compare sorted workspace package names to:

```text
gestalt-app
gestalt-core
gestalt-cli
gestalt-runtime
gestalt-tui
```

The CLI package and directory are both `gestalt-cli`; its binary target remains `gestalt`.

Add direct forbidden-edge assertions to `scripts/check-deps.sh` using Cargo metadata:

```bash
assert_no_path_dep() {
  local package=$1
  local forbidden=$2
  if jq -e --arg package "$package" --arg forbidden "$forbidden" '
    .packages[]
    | select(.name == $package)
    | .dependencies[]
    | select(.kind == null and .name == $forbidden)
  ' "$metadata_file" >/dev/null; then
    printf 'ERROR: %s must not depend on %s\n' "$package" "$forbidden" >&2
    return 1
  fi
}
```

Apply it to every forbidden edge in the feature spec. Also enforce the terminal dependency boundary:

```bash
minimal_cli_tree=$(cargo tree -p gestalt-cli --no-default-features --edges normal --prefix none)
if grep -Eq '^(ratatui|crossterm) v' <<<"$minimal_cli_tree"; then
  printf 'ERROR: minimal CLI includes terminal UI dependencies\n' >&2
  exit 1
fi
```

- [ ] **Step 2: Remove absorbed workspace members**

Remove `gestalt-context`, `gestalt-exec`, `gestalt-mcp`, `gestalt-models`, `gestalt-policy`, `gestalt-skills`, `gestalt-tools`, `gestalt-trace`, and `gestalt-verify` from the workspace and filesystem.

- [ ] **Step 3: Regenerate and verify metadata**

Run: `cargo metadata --no-deps --format-version 1`

Expected: exactly five workspace packages.

Run: `bash scripts/check-deps.sh`

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates scripts/check-deps.sh
git commit -m "refactor: remove absorbed implementation crates"
```

### Task 10: Update Architecture, Install, And Release Documentation

**Files:**
- Modify: `README.md`
- Modify: `docs/gestalt-harness-architecture.md`
- Modify: `docs/gestalt-harnes-implementation-roadmap.md`
- Modify: `docs/release-checklist.md`
- Modify: `crates/gestalt-core/README.md`
- Modify: `crates/gestalt-runtime/README.md`
- Create: `crates/gestalt-app/README.md`
- Create: `crates/gestalt-tui/README.md`
- Modify: `scripts/install-smoke.sh`

- [ ] **Step 1: Replace the obsolete crate graph**

Document only the five final crates and the allowed direction:

```text
gestalt-cli      -> gestalt-app -> gestalt-runtime -> gestalt-core
gestalt-tui      -> gestalt-app -> gestalt-runtime -> gestalt-core
```

- [ ] **Step 2: Document install profiles**

Document:

```bash
cargo install gestalt-cli
cargo install gestalt-cli --no-default-features
cargo install gestalt-tui
```

Explain that bare `gestalt` launches `gestalt-tui` by default and `gestalt tui` is an explicit alias. Explicit CLI subcommands do not launch the TUI.

- [ ] **Step 3: Expand install smoke coverage**

Keep the existing isolated CLI install and add a separate isolated TUI install. Verify `gestalt --help`, minimal config validation, and `gestalt-tui --help`. Then verify the default entrypoint without opening a terminal:

```bash
GESTALT_TUI_BIN=true "$install_root/bin/gestalt"
```

Expected: exit status `0`, proving bare `gestalt` delegates to the configured TUI executable.

- [ ] **Step 4: Run final verification**

Run: `cargo fmt --all --check`

Run: `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`

Run: `cargo test --workspace --all-features --locked`

Run: `cargo check -p gestalt-cli --no-default-features --locked`

Run: `bash scripts/check-deps.sh`

Run: `bash scripts/install-smoke.sh`

Expected: all commands PASS.

- [ ] **Step 5: Commit**

```bash
git add README.md docs crates scripts/install-smoke.sh
git commit -m "docs: document consolidated crate architecture"
```

## Acceptance Traceability

| Feature-spec criterion | Plan task |
|---|---|
| CLI package is `gestalt-cli` and its binary remains `gestalt` | Task 1 |
| Workspace builds and tests pass | Tasks 1, 10 |
| Core/runtime/app dependency direction | Tasks 1, 7, 9 |
| TUI does not depend on CLI | Task 4 |
| Minimal CLI excludes terminal dependencies | Tasks 1, 4, 8 |
| Bare `gestalt` launches the separate, behavior-equivalent TUI | Task 4 |
| Existing CLI commands remain | Tasks 3, 4, 8 |
| Runtime/app embedding examples compile | Task 8 |
| Old crates are absent | Task 9 |
| Public docs explain ownership/install profiles | Task 10 |
