# Changelog

## 0.1.0

Initial Phase 1 release candidate for `gestalt-harness`.

### Highlights

- Local-first `gestalt` CLI for single-agent sessions.
- Explicit policy and approval flow for tool execution.
- Replayable JSONL traces with replay and cost commands.
- Provider, model, and workspace configuration validation paths that work without live credentials in CI.
- Linux and macOS release-readiness checks covering install smoke, dependency budgets, and binary-size auditing.
- **Cache-aware prompt assembly** — separates stable session context from turn-specific context to preserve provider prompt-cache hit rates across turns. Configurable via `prompt.assembly_strategy` in `gestalt.json` (`"snapshot"` / `"dynamic"`). See [ADR-026](docs/adrs/ADR-026-cache-aware-prompt-assembly.md).

### Install and Platform Notes

- Published package name: `gestalt-harness`
- Installed `gestalt` binary name remains unchanged
- Supported platforms for v0.1: Linux x86-64 and macOS
- Local install from a checkout: `cargo install --locked --path crates/gestalt-cli`

### Not in Scope for v0.1

- Registry publishing automation
- Windows support
- MCP enabled by default
- PDF ingestion, skills, vector search, deterministic replay, or other Phase 2 features
