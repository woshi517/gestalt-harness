# Gestalt

**An open-source, microkernel AI agent harness written in Rust — built for developers who outgrew hard-coded LLM loops.**

> **Project Status:** Phase 1 (`v0.1` Proof Gate) and Phase 2 Milestones **M2.1** (Container & Micro-VM Sandboxes) & **M2.2** (Event & Schedule `Trigger` Runtime) are complete. Next up: Milestone **M2.3** (MCP Code Mode, Expanded Model Paradigms & `curator` Replay Gating).

---

## Why Gestalt?

Most AI agent harnesses today are trapped between two architectural extremes:

1. **Hard-coded generative loops (`messages[] -> LLM -> tools -> repeat`).** They work great for basic chat, until you want to route fast decisions through a non-generative System One classifier, swap control-flow strategies mid-session, or run custom state machines. Suddenly you are fighting the framework or bolting on brittle hacks.
2. **"Everything-is-a-plugin" graphs.** They give you flexibility by dissolving the core—making even the agent loop, security checks, and sandboxing optional plugins. The result is a fragile web of config overlays with no real root of trust.

**Gestalt takes the Linux kernel approach.**

It draws a hard line between an **invariant, sans-I/O microkernel** and a **fully programmable user space**:

- **The kernel owns mechanism and trust.** It records every step in a content-addressed session tree, gates every side effect through a fail-closed policy engine, and enforces OS-level sandboxing. It knows nothing about prompts, chat roles, markdown skills, or LLM vendors.
- **User space owns intelligence and UX.** Every agent loop, context builder, model paradigm, tool, trigger, and UI surface lives in user space behind one unified `Extension` contract. The built-in `general` coding agent, TUI, and workspace tools use the *exact same* public extension API as your own custom Rust crates or out-of-process JSON-RPC / MCP extensions—with zero private backdoors.

Because the default userland boots out of the box with working tools and loops, you get a fast, terminal-native agent immediately—and a clean, auditable foundation you can reshape (or let the agent extend via `/reload`) without ever forking the core.

---

## What You Can Build & Do With Gestalt

### 1. Mix Model Paradigms in a Single Session
Not every step needs a 2-second generative LLM call. In Gestalt, an `AgentLoop` is just a state machine that yields typed `Effect`s (`Model`, `Tool`, `Save`, `Spawn`, `Switch`, `Pause`).
- Route routine classification or triage through a fast, non-generative `Decision` model (`DecisionRouter` / `triage` preset) without constructing `messages[]`.
- When confidence is low or deep reasoning is needed, yield `Effect::Switch` (or run `/mode`) to hand off seamlessly to a full generative `ChatLoop`—sealing the current `Phase` and preserving 100% of your `SessionTree` history.

### 2. Security That Prompts and Plugins Can't Bypass
Safety in Gestalt is not a plugin you have to remember to enable:
- **Fail-closed `PolicyGate`:** Every `Model`, `Tool`, `Save`, and `Spawn` effect is checked against budgets, tool allowlists, and mandatory rules *before* execution. If a check errors or times out, it denies by default.
- **OS & VM Sandboxing:** Subprocess tools (`bash` and out-of-process extensions) run inside real isolation boundaries—`bubblewrap` on Linux, `sandbox-exec` (Seatbelt) on macOS, or Docker / Firecracker / Apple Virtualization—with `protected_paths` mounted read-only so shell commands cannot tamper with harness controls or credentials.
- **Staged Self-Modification:** Persistent mutations (`Effect::Save`) are staged with SHA-256 content digests before promotion and tracked in `.gestalt/ledger.jsonl` for atomic rollback via `gestalt rollback <stage_id>`.

### 3. Deterministic Offline Replay
Debugging agent failures by re-running live API calls is slow, expensive, and non-deterministic. Gestalt persists every `Turn`, `Step`, `Effect`, and `StepResult` into an append-only, SHA-256-chained `SessionTree` JSONL file alongside a content-addressed blob store.

Run `gestalt replay <session.jsonl>` to verify hash chains and re-execute recorded `AgentLoop`s against logged `StepResult`s with **zero network calls**.

### 4. Batteries Included, Yet Completely Optional
Use `gestalt` directly in your terminal, embed it in an editor or CI pipeline, or strip out `gestalt-std` and bring your own loops:
- **Interactive TUI (`ratatui` + `crossterm`):** Streaming text and reasoning traces, inline diff previews, mid-run `/steer` and clean `Ctrl+C` interruption, `/tree` branch navigation, `/fork` & `/rewind`, `/model` & `/thinking` selectors, and proactive triggers (`/loop`, `/schedule`, `/watch`, `/webhook`).
- **Headless & Protocol-Native:** Non-interactive print/NDJSON (`-p --json`), stdio JSON-RPC 2.0 (`--mode rpc`), and Agent Client Protocol v1 (`--mode acp`).
- **Auditable Core:** `gestalt-kernel` enforces `#![forbid(unsafe_code)]`, performs zero I/O, and has at most **2 direct dependencies** (`serde` and `serde_json`).

---

## Workspace Architecture

| Crate | Role | Constraints |
| :--- | :--- | :--- |
| `crates/gestalt-kernel` | Sans-I/O microkernel (`SessionTree`, `Effect` bus, fail-closed `PolicyGate`, `Steering` inbox, `EventStream`, deterministic replay, `Extension` traits) | Sans-I/O, domain-pure, `<= 2` direct deps (`serde`, `serde_json`), `#![forbid(unsafe_code)]` |
| `crates/gestalt-runtime` | Reusable Tokio I/O host (`DriverDirective` runner, `TriggerManager`, `ext_ipc` stdio JSON-RPC, MCP bridge, `gestalt.toml` trust gate, `artifacts.rs` staging/ledger, `BlobStore`, `CredentialStore`, `ProviderRegistry`, `bwrap`/`Seatbelt`/`docker`/`Firecracker`/`apple_vz` sandboxes) | Zero hardcoded loops, tools, vendors, or UI |
| `crates/gestalt-std` | Optional Cargo-feature-gated userland (`loops`, `tools`, `providers`, `surfaces`, `tui`): `ChatLoop`, `DecisionRouter`, `goal_check`/`compaction`/`curator` decorators, 8 workspace tools, `interval`/`cron_schedule`/`fs_watch`/`webhook` triggers, `anthropic`/`openai`/`jev` adapters, and `print`/`rpc`/`acp`/`replay`/`tui` surfaces | Registers strictly via public `Extension`, `TriggerHandler`, and `ProviderRegistry` contracts |
| `crates/gestalt-cli` | Thin CLI entrypoint (`gestalt`) wiring `gestalt-runtime` and selected `gestalt-std` features | Minimal composition root |

---

## Prerequisites

- **Rust:** MSRV `1.85+` (Edition 2024), `rustfmt`, `clippy`
- **OS Sandbox & Tools:** `bwrap` (`bubblewrap` on Linux) or `sandbox-exec` (macOS), and `git`

## Building & Verifying

```bash
# Check all workspace crates and targets
cargo check --workspace --all-targets

# Run formatting, clippy, and tests
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Audit kernel direct dependency ceiling
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
