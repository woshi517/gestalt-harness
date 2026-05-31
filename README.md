# gestalt-harness

**Status: Scaffold — no implementation code yet**

`gestalt-harness` is a lightweight, local-first AI agent harness written in Rust. It is optimized for knowledge work — synthesizing academic papers, PDFs, architecture docs, web research, and Markdown notes — while remaining natively capable at system execution, coding, and tool calling via bash and MCP.

Rather than building a heavy, magic-filled orchestration framework, `gestalt-harness` delivers a small, transparent execution harness. It establishes explicit permission gates before any tool runs, provides deterministic context assembly, tracks every session event to a human-readable JSONL log, and allows complete user auditability.

## Documentation

- [Product Requirements Document](docs/gestalt-harness-prd.md)
- [Architecture Document](docs/gestalt-harness-architecture.md)
- [Implementation Roadmap](docs/gestalt-harnes-implementation-roadmap.md)
- [Architecture Decision Records Index](docs/adrs/README.md)
- [Contributor Guidelines](CONTRIBUTING.md)

## Quick Start

To verify that the workspace compiles:

```bash
cargo check --workspace
```

## Development Commands

Ensure all checks pass before submitting a pull request:

```bash
# Verify formatting
cargo fmt --all --check

# Lint the workspace
cargo clippy --workspace --all-targets -- -D warnings

# Run all tests
cargo test --workspace

# Audit workspace dependency directions (ADR-001)
bash scripts/check-deps.sh
```

## License

This project is licensed under the MIT License.
