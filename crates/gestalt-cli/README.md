# `gestalt-cli`

Thin CLI composition root (`gestalt` binary) wiring `gestalt-kernel`, `gestalt-runtime`, and feature-selected `gestalt-std` modules.

## Hard Constraints

- `#![forbid(unsafe_code)]`
- Minimal composition root: parses CLI flags, loads layered `gestalt.toml` configuration and workspace trust, initializes secret-redacting `tracing` output on `stderr`, registers selected extensions/providers/tools/triggers, and dispatches to the requested surface.

## Cargo Features

Mirrors `gestalt-std` features (`default = ["loops", "tools", "providers", "surfaces", "tui"]`) so custom distributions can compile headless or minimal binaries without changing source code.

## CLI Modes

- `gestalt` — Interactive TUI (`--features tui`).
- `gestalt -p "<prompt>" [--json]` — Single-Turn headless print / NDJSON mode.
- `gestalt --mode rpc` — Headless stdio JSON-RPC 2.0 server.
- `gestalt --mode acp` — Agent Client Protocol (ACP v1) stdio server.
- `gestalt replay <session.jsonl> [--reexecute-ext] [--json]` — Deterministic offline replay.
- `gestalt rollback <stage_id>` — Atomic rollback of promoted `Effect::Save` artifacts via `.gestalt/ledger.jsonl`.

Key global flags: `--preset <name>`, `--model <provider/model>`, `--session <path>`, `--ext <[name=]cmd>` (or `mcp:<cmd>`), `--sandbox <os|docker|microvm>`, `--trust-workspace`, `--unsafe-no-sandbox`.

## Integration Tests

`gestalt-cli` also declares the workspace-level integration test suites in `Cargo.toml`:
- `triage_benchmark` (`tests/integration/triage_benchmark.rs`)
- `self_extension_proof` (`tests/integration/self_extension_proof.rs`)
- `proof_gate_suite` (`tests/integration/proof_gate_suite.rs`)

```bash
cargo test -p gestalt-cli
```
