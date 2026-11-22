# Gestalt

A microkernel AI agent harness for dynamic semantics, written in Rust.

Gestalt separates an invariant, sans-I/O microkernel (`gestalt-kernel`, <= 2,000 LOC, `<= 2` direct dependencies, `#![forbid(unsafe_code)]`) from a reusable Tokio host runtime (`gestalt-runtime`) and an optional, batteries-included userland (`gestalt-std` + `gestalt-cli`). All agent loops, context builders, model paradigms, tools, and UI surfaces run in user space through a unified `Extension` contract.

> **Project Status:** Phase 1 (`v0.1` Proof Gate) and Phase 2 Milestones **M2.1** (Container & Micro-VM Sandboxes) & **M2.2** (Event & Schedule `Trigger` Runtime) are complete. Next up: Milestone **M2.3** (MCP Code Mode, Expanded Model Paradigms & `curator` Replay Gating).

## Workspace Architecture

| Crate | Role | Constraints |
| :--- | :--- | :--- |
| `crates/gestalt-kernel` | Sans-I/O microkernel (`SessionTree`, `Effect` bus, fail-closed `PolicyGate`, `Steering` inbox, `EventStream`, deterministic replay, `Extension` traits) | `<= 2,000` code LOC, `<= 2` direct deps (`serde`, `serde_json`), `#![forbid(unsafe_code)]` |
| `crates/gestalt-runtime` | Reusable Tokio I/O host (`DriverDirective` runner, `TriggerManager`, `ext_ipc` stdio JSON-RPC, MCP bridge, `gestalt.toml` trust gate, `artifacts.rs` staging/ledger, `BlobStore`, `CredentialStore`, `ProviderRegistry`, `bwrap`/`Seatbelt`/`docker`/`Firecracker`/`apple_vz` sandboxes) | Zero hardcoded loops, tools, vendors, or UI |
| `crates/gestalt-std` | Optional Cargo-feature-gated userland (`loops`, `tools`, `providers`, `surfaces`, `tui`): `ChatLoop`, `DecisionRouter`, `goal_check`/`compaction`/`curator` decorators, 8 workspace tools, `interval`/`cron_schedule`/`fs_watch`/`webhook` triggers, `anthropic`/`openai`/`jev` adapters, and `print`/`rpc`/`acp`/`replay`/`tui` surfaces | Registers strictly via public `Extension`, `TriggerHandler`, and `ProviderRegistry` contracts |
| `crates/gestalt-cli` | Thin CLI entrypoint (`gestalt`) wiring `gestalt-runtime` and selected `gestalt-std` features | Minimal composition root |

## Core Design Principles

1. **Sans-I/O Microkernel:** `gestalt-kernel` performs no network I/O, no subprocess spawning, and no async runtime scheduling. It evaluates policies, maintains the SHA-256-chained `SessionTree`, and yields `DriverDirective`s to `gestalt-runtime`.
2. **Fail-Closed Security & Sandboxing:** Every `Model`, `Tool`, `Save`, and `Spawn` effect passes through `PolicyGate` before dispatch. In-process file tools are guarded by `WorkspacePathGuard`, and subprocess tools (`bash`, out-of-process extensions) run inside OS sandboxes (`bubblewrap` on Linux, `sandbox-exec` Seatbelt on macOS) with `protected_paths` mounted read-only.
3. **Multi-Paradigm Dynamic Semantics:** Loops are user-space state machines (`ChatLoop`, `DecisionRouter` for fast non-generative System One models, or custom Rust/out-of-process loops) that can switch mid-session via `/mode` or `Effect::Switch` while preserving full `SessionTree` history.
4. **Deterministic Offline Replay:** `gestalt replay <session.jsonl>` verifies `SessionTree` hash chains and re-executes recorded `AgentLoop`s against logged `StepResult`s and blob-snapshotted `ArtifactStoreView` entries with zero network calls.

## Prerequisites

- **Rust:** MSRV `1.85+` (Edition 2024), `rustfmt`, `clippy`
- **OS Sandbox & Tools:** `bwrap` (`bubblewrap` on Linux) or `sandbox-exec` (macOS), `git`, and `tokei` (for kernel LOC CI checks)

## Building & Verifying

```bash
# Check all workspace crates and targets
cargo check --workspace --all-targets

# Run formatting, clippy, and tests
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Audit kernel LOC and direct dependency ceiling
tokei crates/gestalt-kernel/src
cargo tree -p gestalt-kernel --depth 1
```

## CLI & Protocol Surfaces

- `gestalt` — Interactive terminal UI (`ratatui` + `crossterm`, gated behind `--features tui`) with Turn/Step transcript cards, per-step and global (`Ctrl+O`) step minimizing, inline diff previews, `/tree` (`Esc Esc`) branch navigation, `/session` (`/resume`) session switcher, `/model` & `/thinking` selector modals, trigger commands (`/loop`, `/schedule`, `/watch`, `/webhook`, `/triggers`), and actionable model/credential/streaming error diagnostics
- `gestalt -p "<prompt>" [--json] [--preset <name>] [--model <provider/model>] [--sandbox <os|docker|microvm|auto>]` — Non-interactive print / NDJSON mode
- `gestalt --mode rpc` — Headless stdio JSON-RPC 2.0 server (including `trigger/*` lifecycle methods)
- `gestalt --mode acp` — Agent Client Protocol (ACP v1) stdio server (including `_gestalt/trigger/*` extensions)
- `gestalt replay <session.jsonl> [--reexecute-ext] [--json]` — Deterministic offline replay
- `gestalt rollback <stage_id>` — Atomic rollback of promoted `Effect::Save` artifacts via `.gestalt/ledger.jsonl`

## Documentation

- [Architecture Specification](docs/ARCHITECTURE.md)
- [Product Requirements Document (PRD)](docs/PRD.md)
- [Technical Roadmap](docs/TECHNICAL_ROADMAP.md)
- [Rust Code Quality Guidelines](AGENTS.md)

## License

Licensed under the [GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).
