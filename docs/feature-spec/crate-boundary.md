# Feature Specification: Crate Boundary Consolidation and TUI Extraction

## Status

Implemented; retained as the feature rationale for the five-crate consolidation.
Current crate READMEs and the workspace manifest govern the implemented
boundary.

## Summary

Gestalt currently has a fragmented multi-crate layout:

```text
gestalt-cli
gestalt-context
gestalt-core
gestalt-exec
gestalt-mcp
gestalt-models
gestalt-policy
gestalt-runtime
gestalt-skills
gestalt-tools
gestalt-trace
gestalt-verify
```

This structure was useful while building harness primitives independently, but it now creates unnecessary architectural surface area. Several crates are not true product/package boundaries; they are implementation domains of the runtime or CLI. The result is higher maintenance cost, harder embedding, more confusing dependency ownership, and a risk of accidental bloat in products that only want the harness runtime.

This feature restructures Gestalt into a smaller, clearer crate architecture:

```text
crates/
  gestalt-core        # pure harness primitives
  gestalt-runtime     # embeddable runtime composition and concrete harness behavior
  gestalt-app         # shared app services: config, auth, run/session helpers
  gestalt-cli         # minimal command-line shell
  gestalt-tui         # terminal UI shell
```

The main architectural objective is to separate **harness primitives**, **runtime implementation**, **application services**, and **presentation shells**.

---

## Problem Statement

The current crate structure introduces too many public boundaries for a young harness.

Many crates represent implementation concerns rather than independently reusable packages:

* `gestalt-context` is runtime context-building behavior.
* `gestalt-models` is provider adapter implementation.
* `gestalt-policy` is concrete policy parsing and enforcement.
* `gestalt-tools` and `gestalt-exec` are concrete runtime capabilities.
* `gestalt-mcp` is an optional integration backend.
* `gestalt-trace` is runtime observability and persistence.
* `gestalt-skills` supports runtime context construction.
* `gestalt-verify` is a runtime/app-level verification capability.

Keeping each of these as its own crate makes the project feel larger than it is and increases coordination overhead. It also makes it less obvious which crates are safe for embedding in desktop products, remote runners, SDKs, or minimal CLI installations.

At the same time, the TUI currently lives inside `gestalt-cli`. Although it is feature-gated, this still couples terminal UI concerns to the CLI crate and makes future product surfaces harder to cleanly embed.

---

## Goals

1. Reduce crate fragmentation.
2. Preserve a clean embeddable runtime boundary.
3. Keep `gestalt-core` pure, small, and I/O-free.
4. Move concrete harness behavior into `gestalt-runtime`.
5. Move shared application services into a new `gestalt-app` crate.
6. Keep `gestalt-cli` as a thin command-line shell.
7. Move the TUI into a dedicated `gestalt-tui` crate.
8. Support minimal installs that do not include TUI or unnecessary integration dependencies.
9. Make future desktop products and remote runners depend on `gestalt-runtime` or `gestalt-app`, not `gestalt-cli`.

---

## Non-Goals

1. Do not redesign the agent loop.
2. Do not change provider/tool/policy semantics.
3. Do not remove existing CLI commands.
4. Do not rewrite the TUI.
5. Do not move concrete provider, filesystem, HTTP, subprocess, tokenization, or tracing logic into `gestalt-core`.
6. Do not create a new plugin system as part of this refactor.
7. Do not change user-facing behavior unless required by the crate split.

---

## Target Crate Architecture

```text
crates/
  gestalt-core
  gestalt-runtime
  gestalt-app
  gestalt-cli
  gestalt-tui
```

### `gestalt-core`

Pure harness primitives.

Owns:

```text
agent loop
session types
message types
content blocks
provider traits
tool traits
policy traits
approval traits
event types
error types
cancel token
core config-independent types
```

Does not own:

```text
HTTP clients
provider adapters
filesystem tools
subprocess execution
MCP clients
policy file parsing
JSONL trace writing
tokenizers
skill discovery
CLI config loading
TUI state
```

`gestalt-core` must remain the stable foundation for any runtime or host.

---

### `gestalt-runtime`

Concrete embeddable runtime composition.

Owns:

```text
runtime host
agent runtime builder
context pipeline implementation
provider adapters
built-in tools
subprocess execution
policy engine implementation
MCP integration
skills integration
trace writing/reading
verification registry
extension runtime
runtime event bus
runtime diagnostics
```

This crate should be the default dependency for users who want to embed the harness engine.

Examples:

```rust
use gestalt_runtime::api::v1::{AgentRuntime, AgentRuntimeBuilder, RuntimeConfig};
```

Feature-gated modules:

```text
runtime::mcp       behind feature = "mcp"
runtime::otel      behind feature = "otel"
runtime::skills    optional if needed
runtime::verify    optional if needed
```

---

### `gestalt-app`

Shared application/service layer.

Owns:

```text
config loading and merging
auth and credential resolution
provider catalog resolution
model selection helpers
workspace initialization/status
run/session helpers
run log resolution
session lineage services
app-level report models
app-level orchestration helpers
```

This crate exists because both `gestalt-cli` and `gestalt-tui` need shared application behavior, but neither should depend on the other.

Desktop products may also choose to depend on `gestalt-app` when they want Gestalt’s config/auth/session conventions instead of directly composing `gestalt-runtime`.

---

### `gestalt-cli`

Minimal command-line shell.

Owns:

```text
clap command definitions
text/json output formatting
CLI argument parsing
command dispatch
exit code behavior
terminal-neutral user messages
```

Does not own:

```text
TUI rendering
runtime construction internals
provider adapters
tool implementations
policy engine internals
trace writer internals
session storage internals
```

The CLI should be a thin shell over `gestalt-app`.

---

### `gestalt-tui`

Terminal UI shell.

Owns:

```text
ratatui/crossterm integration
TUI app state
TUI widgets
TUI screens
TUI approval popup
TUI event log rendering
TUI session browser
TUI diagnostics drawer
terminal lifecycle handling
```

Depends on:

```text
gestalt-app
gestalt-runtime
gestalt-core
ratatui
crossterm
```

Must not depend on:

```text
gestalt-cli
```

This prevents TUI concerns from bloating the minimal CLI and prevents CLI parsing/output concerns from leaking into the terminal app.

---

## Current Crate Consolidation Map

| Current Crate     | Target Location                                           | Rationale                                                                     |
| ----------------- | --------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `gestalt-core`    | Keep as `gestalt-core`                                    | Pure harness primitives and stable trait boundary.                            |
| `gestalt-runtime` | Keep and expand                                           | Becomes the concrete engine and runtime composition layer.                    |
| `gestalt-cli`     | Keep but shrink                                           | Becomes a thin shell over `gestalt-app`.                                      |
| `gestalt-context` | `gestalt-runtime::context`                                | Context construction is concrete runtime behavior.                            |
| `gestalt-models`  | `gestalt-runtime::providers`                              | Provider adapters are runtime integrations, not core primitives.              |
| `gestalt-policy`  | `gestalt-runtime::policy`                                 | Policy traits stay in core; concrete policy engine/parser belongs in runtime. |
| `gestalt-tools`   | `gestalt-runtime::tools`                                  | Built-in tools are runtime capabilities.                                      |
| `gestalt-exec`    | `gestalt-runtime::exec` or `gestalt-runtime::tools::exec` | Subprocess execution is concrete runtime behavior.                            |
| `gestalt-mcp`     | `gestalt-runtime::mcp`                                    | MCP is an optional runtime integration backend.                               |
| `gestalt-skills`  | `gestalt-runtime::skills` or `gestalt-app::skills`        | Skills support context/runtime behavior, not core.                            |
| `gestalt-trace`   | `gestalt-runtime::trace`                                  | JSONL trace writing/reading is runtime I/O.                                   |
| `gestalt-verify`  | `gestalt-runtime::verify`                                 | Verification depends on concrete runtime/tool behavior.                       |
| `gestalt-tui`     | New crate                                                 | Terminal UI should be separate from CLI.                                      |
| `gestalt-app`     | New crate                                                 | Shared config/auth/session/application services.                              |

---

## Architectural Decisions

### AD-001: Keep `gestalt-core` small and pure

Decision:

`gestalt-core` remains the home of primitives, traits, events, messages, sessions, and the agent loop. It must not absorb concrete runtime implementations.

Rationale:

The core crate is the long-term stability boundary. It should be suitable for embedding, testing, mocking, and potentially alternative runtimes. Adding provider HTTP clients, filesystem tools, subprocess execution, tokenizers, trace writers, or MCP clients would make core heavier and less reusable.

Consequence:

Traits and abstract types stay in core. Implementations move to runtime.

---

### AD-002: Make `gestalt-runtime` the concrete engine

Decision:

Most current implementation crates should become modules under `gestalt-runtime`.

Rationale:

Context construction, provider adapters, built-in tools, policy enforcement, tracing, verification, skills, MCP, and execution are all runtime behavior. Keeping them in one runtime crate makes dependency ownership clearer and reduces crate sprawl.

Consequence:

`gestalt-runtime` becomes larger, but it becomes the obvious crate for anyone embedding the harness engine.

---

### AD-003: Introduce `gestalt-app` as the host/application service layer

Decision:

Create a new `gestalt-app` crate for config, auth, provider catalog, run/session helpers, workspace services, and app-level models.

Rationale:

`gestalt-cli` currently owns too much reusable application behavior. The TUI, desktop apps, and remote runners should not depend on CLI just to reuse config loading or session orchestration.

Consequence:

`gestalt-cli` and `gestalt-tui` both depend on `gestalt-app`.

---

### AD-004: Extract TUI into `gestalt-tui`

Decision:

Move terminal UI code out of `gestalt-cli` into a new `gestalt-tui` crate.

Rationale:

The TUI is a presentation shell with heavy terminal-specific dependencies and state. It should not live inside the CLI crate, even if currently feature-gated. Future desktop products, remote runners, and minimal CLI installs should not inherit TUI coupling.

Consequence:

`gestalt-tui` depends on `gestalt-app`, not `gestalt-cli`.

---

### AD-005: Keep CLI as a minimal shell

Decision:

`gestalt-cli` should own command parsing, output formatting, command dispatch, and process exit behavior only.

Rationale:

A CLI binary should not be the application core. Keeping it thin makes it easier to maintain, test, and replace with other frontends.

Consequence:

Most existing CLI modules must move into `gestalt-app`.

---

### AD-006: Feature-gate optional runtime integrations

Decision:

Optional integrations such as MCP, OpenTelemetry, skills, and verification should be runtime features.

Example:

```toml
[features]
default = ["providers", "tools"]
mcp = []
otel = []
skills = []
verify = []
```

Rationale:

Minimal embedding should not pay for integrations it does not use.

Consequence:

`gestalt-runtime` becomes configurable for different product profiles.

---

### AD-007: Do not make `gestalt-tui` depend on `gestalt-cli`

Decision:

The TUI crate must not depend on the CLI crate.

Rationale:

If `gestalt-tui` depends on `gestalt-cli`, the project simply moves bloat sideways. The clean dependency direction is:

```text
gestalt-cli -> gestalt-app
gestalt-tui -> gestalt-app
gestalt-app -> gestalt-runtime
gestalt-runtime -> gestalt-core
```

Consequence:

Any shared TUI/CLI services must live in `gestalt-app`, not either shell crate.

---

## Target Dependency Direction

Allowed dependency flow:

```text
gestalt-core
   ↑
gestalt-runtime
   ↑
gestalt-app
   ↑        ↑
gestalt-cli gestalt-tui
```

More explicitly:

```text
gestalt-runtime -> gestalt-core

gestalt-app -> gestalt-runtime
gestalt-app -> gestalt-core

gestalt-cli -> gestalt-app
gestalt-cli -> gestalt-runtime
gestalt-cli -> gestalt-core

gestalt-tui -> gestalt-app
gestalt-tui -> gestalt-runtime
gestalt-tui -> gestalt-core
```

Forbidden dependencies:

```text
gestalt-core -> gestalt-runtime
gestalt-core -> gestalt-app
gestalt-core -> gestalt-cli
gestalt-core -> gestalt-tui

gestalt-runtime -> gestalt-app
gestalt-runtime -> gestalt-cli
gestalt-runtime -> gestalt-tui

gestalt-app -> gestalt-cli
gestalt-app -> gestalt-tui

gestalt-tui -> gestalt-cli
gestalt-cli -> gestalt-tui
```

---

## Proposed Module Layout

### `gestalt-core`

```text
src/
  agent.rs
  approval.rs
  cancel.rs
  context.rs          # traits/types only
  error.rs
  event.rs
  message.rs
  policy.rs           # traits/types only
  provider.rs         # traits/types only
  session.rs
  tool.rs             # traits/types only
  usage.rs
```

### `gestalt-runtime`

```text
src/
  lib.rs
  runtime.rs
  host.rs
  builder.rs
  registry.rs

  context/
    mod.rs
    pipeline.rs
    token_budget.rs
    compaction.rs
    projection.rs

  providers/
    mod.rs
    openai.rs
    anthropic.rs
    openrouter.rs
    ollama.rs
    common.rs

  tools/
    mod.rs
    registry.rs
    read.rs
    write.rs
    edit.rs
    search.rs
    bash.rs
    fetch.rs

  exec/
    mod.rs
    process.rs
    sandbox.rs

  policy/
    mod.rs
    engine.rs
    parser.rs
    rules.rs

  trace/
    mod.rs
    jsonl.rs
    manifest.rs
    replay.rs

  mcp/
    mod.rs
    client.rs
    transport.rs

  skills/
    mod.rs
    discovery.rs
    parser.rs
    activation.rs

  verify/
    mod.rs
    registry.rs
    verifiers.rs

  extensions/
    mod.rs
    discovery.rs
    lifecycle.rs
    broker.rs
```

### `gestalt-app`

```text
src/
  lib.rs
  config/
    mod.rs
    loader.rs
    schema.rs
    profiles.rs

  auth/
    mod.rs
    keychain.rs
    env.rs
    resolver.rs

  catalog/
    mod.rs
    providers.rs
    models.rs

  workspace/
    mod.rs
    init.rs
    status.rs
    doctor.rs

  runs/
    mod.rs
    resolve.rs
    list.rs
    inspect.rs
    delete.rs

  sessions/
    mod.rs
    list.rs
    inspect.rs
    continue_run.rs
    branch.rs
    resume.rs

  services/
    runtime_factory.rs
    run_prompt.rs
    chat.rs

  reports/
    mod.rs
    cli_reports.rs
    app_reports.rs
```

### `gestalt-cli`

```text
src/
  main.rs
  lib.rs
  commands/
    mod.rs
    run.rs
    chat.rs
    config.rs
    auth.rs
    providers.rs
    models.rs
    workspace.rs
    runs.rs
    sessions.rs
    tools.rs
    policy.rs
    trace.rs
    verify.rs
  output/
    mod.rs
    text.rs
    json.rs
    errors.rs
```

### `gestalt-tui`

```text
src/
  lib.rs
  main.rs
  app.rs
  approval.rs
  bridge.rs
  state.rs
  update.rs
  services.rs
  screens/
    mod.rs
    chat.rs
  widgets/
    mod.rs
    event_log.rs
    status_bar.rs
    approval_popup.rs
    session_switcher.rs
    diagnostics_drawer.rs
```

---

## Minimal Install Strategy

The default installation should be minimal and should not include TUI dependencies.

Recommended profiles:

```bash
cargo install gestalt-cli --no-default-features
```

Installs:

```text
minimal CLI
core runtime
basic providers/tools
no TUI
no terminal UI deps
```

Full local terminal app:

```bash
cargo install gestalt-tui
```

Or, if keeping one package name:

```bash
cargo install gestalt-harness --features tui
```

Recommended long-term package split:

```text
gestalt-cli      binary: gestalt
gestalt-tui      binary: gestalt-tui
gestalt-runtime  library
gestalt-app      library
gestalt-core     library
```

---

## Migration Plan

### Phase 1: Establish new crates

Create:

```text
crates/gestalt-app
crates/gestalt-tui
```

Add them to the workspace.

Do not remove old crates yet.

---

### Phase 2: Extract app services from `gestalt-cli`

Move reusable non-presentation modules from `gestalt-cli` into `gestalt-app`.

Candidate modules:

```text
config
auth
connect
models
provider_catalog
providers
run
runs
sessions
workspace
runtime builder helpers
```

Keep CLI-specific output/report rendering in `gestalt-cli` unless reused by TUI or desktop.

Goal:

```text
gestalt-cli becomes command parser + output shell
```

---

### Phase 3: Extract TUI

Move:

```text
crates/gestalt-cli/src/tui/**
```

to:

```text
crates/gestalt-tui/src/**
```

Replace `crate::config`, `crate::run`, `crate::sessions`, `crate::auth`, and `crate::runs` dependencies with `gestalt-app` imports.

Goal:

```text
gestalt-tui depends on gestalt-app
gestalt-tui does not depend on gestalt-cli
```

---

### Phase 4: Consolidate implementation crates into runtime

Move current crates into `gestalt-runtime` modules.

Suggested order:

1. `gestalt-exec` -> `gestalt-runtime::exec`
2. `gestalt-policy` -> `gestalt-runtime::policy`
3. `gestalt-context` -> `gestalt-runtime::context`
4. `gestalt-tools` -> `gestalt-runtime::tools`
5. `gestalt-models` -> `gestalt-runtime::providers`
6. `gestalt-trace` -> `gestalt-runtime::trace`
7. `gestalt-skills` -> `gestalt-runtime::skills`
8. `gestalt-mcp` -> `gestalt-runtime::mcp`
9. `gestalt-verify` -> `gestalt-runtime::verify`

Each move should preserve existing tests before deleting the old crate.

---

### Phase 5: Remove old crates

After all imports are updated and tests pass, remove these workspace members:

```text
gestalt-context
gestalt-exec
gestalt-mcp
gestalt-models
gestalt-policy
gestalt-skills
gestalt-tools
gestalt-trace
gestalt-verify
```

---

### Phase 6: Update docs and package guidance

Update:

```text
README.md
architecture docs
crate READMEs
install instructions
embedding examples
```

Add docs for:

```text
minimal CLI install
TUI install
runtime embedding
app-layer embedding
feature flags
crate ownership rules
```

---

## Acceptance Criteria

1. Workspace builds successfully.
2. Existing tests pass.
3. `gestalt-core` has no dependency on runtime, app, CLI, or TUI.
4. `gestalt-runtime` has no dependency on app, CLI, or TUI.
5. `gestalt-app` has no dependency on CLI or TUI.
6. `gestalt-tui` has no dependency on `gestalt-cli`.
7. Minimal CLI build does not compile `ratatui` or `crossterm`.
8. TUI build compiles and runs as a separate crate.
9. Existing CLI commands remain functional.
10. Existing TUI behavior remains functionally equivalent.
11. Runtime embedding examples compile without depending on CLI.
12. Desktop/remote runner embedding can use `gestalt-runtime` or `gestalt-app`.
13. Removed crates no longer appear in workspace members.
14. Public documentation explains the new crate boundaries.

---

## Testing Requirements

### Build checks

```bash
cargo check --workspace
cargo check -p gestalt-core
cargo check -p gestalt-runtime
cargo check -p gestalt-app
cargo check -p gestalt-cli --no-default-features
cargo check -p gestalt-tui
```

### Dependency checks

Verify:

```bash
cargo tree -p gestalt-cli --no-default-features
```

Must not include:

```text
ratatui
crossterm
```

Verify:

```bash
cargo tree -p gestalt-core
```

Must not include:

```text
reqwest
tokio-stream
eventsource-stream
ratatui
crossterm
keyring
rpassword
toml_edit
walkdir
regex
```

### Runtime embedding smoke test

Add an example:

```text
examples/embed_runtime.rs
```

It should construct a runtime without depending on `gestalt-cli` or `gestalt-tui`.

### App embedding smoke test

Add an example:

```text
examples/embed_app.rs
```

It should load config, resolve auth, build a runtime, and run a prompt using `gestalt-app`.

---

## Risks

### Risk: `gestalt-runtime` becomes too large

Mitigation:

Use internal modules and feature flags. The problem is not a large crate; the problem is unclear ownership. A larger but coherent runtime crate is preferable to many small crates with weak boundaries.

---

### Risk: TUI extraction exposes hidden CLI coupling

Mitigation:

Move shared services to `gestalt-app` first. Do not extract TUI directly against `gestalt-cli`.

---

### Risk: Public API churn

Mitigation:

This project is still early. Prefer cleaning boundaries now before downstream users depend on the fragmented crate layout.

---

### Risk: Core accidentally absorbs concrete behavior

Mitigation:

Add a dependency policy and enforce with `cargo tree` checks or CI scripts.

---

## CI Guardrails

Add a script or CI check to enforce dependency direction.

Rules:

```text
gestalt-core must not depend on:
  gestalt-runtime
  gestalt-app
  gestalt-cli
  gestalt-tui
  reqwest
  ratatui
  crossterm
  keyring

gestalt-runtime must not depend on:
  gestalt-app
  gestalt-cli
  gestalt-tui

gestalt-app must not depend on:
  gestalt-cli
  gestalt-tui

gestalt-tui must not depend on:
  gestalt-cli
```

---

## Final Intended Architecture

```text
                         ┌────────────────────┐
                         │    gestalt-cli     │
                         │   command shell    │
                         └─────────┬──────────┘
                                   │
                                   ▼
┌────────────────────┐     ┌────────────────────┐
│    gestalt-tui      ───▶│    gestalt-app     │
│ terminal UI shell  │     │    app services    │
└────────────────────┘     └─────────┬──────────┘
                                     │
                                     ▼
                         ┌────────────────────┐
                         │  gestalt-runtime   │
                         │   concrete engine  │
                         └─────────┬──────────┘
                                   │
                                   ▼
                         ┌────────────────────┐
                         │   gestalt-core     │
                         │   pure primitives  │
                         └────────────────────┘
```

---

## Design Principle

The refactor should be guided by one rule:

```text
Core defines what a harness is.
Runtime implements how the harness works.
App defines how Gestalt is hosted as a product.
CLI and TUI define how humans interact with it.
```

This gives Gestalt a cleaner foundation for:

```text
minimal CLI installs
terminal UI
desktop apps
remote runners
SDK embedding
future hosted products
enterprise integrations
```
