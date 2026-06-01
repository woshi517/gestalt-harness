# gestalt-harness

<p align="center">
  <img src="https://img.shields.io/badge/version-v0.1--rc-blue?style=flat-square" alt="Version" />
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="License" />
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square&logo=rust&logoColor=white" alt="Rust" />
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS-lightgrey?style=flat-square&logo=linux&logoColor=white" alt="Platform" />
  <img src="https://img.shields.io/badge/status-release--candidate-yellow?style=flat-square" alt="Status" />
  <img src="https://img.shields.io/badge/local--first-%E2%9C%93-brightgreen?style=flat-square" alt="Local First" />
</p>

> A local-first AI agent harness written in Rust — built for practical knowledge and engineering work.

`gestalt-harness` handles reading project docs, validating configs, replaying traces, running tools under policy, and keeping the full session auditable from start to finish.

---

## Table of Contents

- [Philosophy](#philosophy)
- [Architecture](#architecture)
- [Supported Platforms](#supported-platforms)
- [Install](#install)
- [Quick Start](#quick-start)
- [Documentation](#documentation)
- [Development Commands](#development-commands)
- [License](#license)

---

## Philosophy

`gestalt-harness` treats agent execution as infrastructure, not theater.

| Principle | Description |
|---|---|
| **Local first** | The default workflow assumes your code, docs, traces, and configuration live on your machine. |
| **Explicit permissions** | Tools run only after policy and approval decisions — no hidden framework magic. |
| **Deterministic enough to inspect** | Config loading, context assembly, and replayable traces should be understandable by a human reading the repo. |
| **Small surface area** | The harness prefers clear crate boundaries and predictable operational behavior over orchestration sprawl. |

---

## Architecture

The system is split into a small core and concrete adapters around it.

`gestalt-core` defines the agent loop, traits, events, and session contracts. The surrounding crates — `gestalt-models`, `gestalt-tools`, `gestalt-context`, `gestalt-policy`, and `gestalt-trace` — provide real implementations around that core.

`gestalt-cli` is the composition root. It is packaged as `gestalt-harness` and installs the `gestalt` binary. This layout keeps the execution loop isolated from file I/O, provider wiring, and CLI concerns while still producing a single practical binary for local use.

---

## Supported Platforms

| Platform | Supported |
|---|---|
| Linux x86-64 | ✅ |
| macOS | ✅ |
| Bash-compatible shells | ✅ |
| Windows | ❌ (not yet) |

---

## Install

Install from a local checkout:

```bash
cargo install --locked --path crates/gestalt-cli
gestalt --help
```

> When the crate is published, the package name will be `gestalt-harness` and the installed executable will remain `gestalt`.

---

## Quick Start

Validate the included minimal fixture workspace without any provider credentials:

```bash
gestalt --workspace tests/fixtures/workspaces/minimal config validate
```

---

## Documentation

| Document | Description |
|---|---|
| [Product Requirements Document](docs/gestalt-harness-prd.md) | Goals, scope, and requirements |
| [Architecture Document](docs/gestalt-harness-architecture.md) | System design and crate layout |
| [Implementation Roadmap](docs/gestalt-harnes-implementation-roadmap.md) | Milestones and planned work |
| [Release Checklist](docs/release-checklist.md) | Steps to cut a release |
| [Changelog](CHANGELOG.md) | Version history |
| [Architecture Decision Records](docs/adrs/README.md) | ADR index |
| [Contributor Guidelines](CONTRIBUTING.md) | How to contribute |

---

## Development Commands

Run the full local release-readiness gate before opening a pull request:

```bash
# Verify formatting
cargo fmt --all --check

# Lint the workspace
cargo clippy --workspace --all-targets -- -D warnings

# Run all tests
cargo test --workspace

# Audit dependency direction and crate budgets
bash scripts/check-deps.sh

# Prove an isolated local install works
bash scripts/install-smoke.sh

# Measure the release binary size
bash scripts/check-binary-size.sh
```

---

## License

This project is licensed under the [MIT License](LICENSE).