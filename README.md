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

![screenshot](/assets/gestalt-screenshot.png)



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

`gestalt-runtime` is the reusable composition layer, and `gestalt-cli` packages the CLI binary `gestalt` around it. This layout keeps the execution loop isolated from file I/O, provider wiring, and CLI concerns while still producing a single practical binary for local use.

---

## Security & Execution Boundary

> [!WARNING]
> `NoSandbox` (the default execution backend in v0.1) is **not a security sandbox**. It is a host subprocess runner with basic constraints (timeout enforcement, output capping, environment variable allowlisting, process-group termination, and working directory validation). It does not provide namespace isolation, chroot, or seccomp filtering. 
> 
> Because commands run directly on the host, **bash tool execution defaults to explicit human confirmation** unless the command is on a small, audited read-only allowlist. Always review command inputs before approving.

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

Onboard using the default OpenRouter flow:

```bash
gestalt connect openrouter
```

This interactive command prompts for your OpenRouter API key, stores it securely in your OS keychain, and sets up a default profile for the workspace.

Alternatively, you can validate the included minimal fixture workspace without any provider credentials:

```bash
gestalt --workspace tests/fixtures/workspaces/minimal config validate
```

---

## Provider Connections and Profiles

`gestalt-harness` features a provider connection and credential-backed profile system to securely manage API keys and switch between model environments without storing raw secrets in config files.

- **Connect to a Provider:**
  ```bash
  gestalt connect openrouter
  # Or connect non-interactively:
  gestalt connect openrouter --api-key <key> --set-default
  ```
  This creates a provider connection entry under `~/.config/gestalt/config.toml` referencing the OS keychain without persisting raw secrets.

- **List Profiles:**
  ```bash
  gestalt profiles list
  ```
  Lists all available profiles and highlights the currently active one.

- **Switch Profiles:**
  ```bash
  gestalt profiles use <profile-name>
  ```
  Switches the active profile for the workspace (stored in `.gestalt/config.toml`).

- **Search Discovered Models:**
  ```bash
  gestalt models search <query>
  ```
  Searches across built-in and provider-discovered models.

---

## Workspace Management

Manage and inspect your agent workspace with the following CLI commands:

- **Initialize a Workspace:**
  ```bash
  gestalt init
  # Or force overwrite existing files:
  gestalt init --force
  ```
  Scaffolds a new workspace containing default `.gestalt/config.toml`, `policies.toml`, `workspace.md`, and `memory.md` templates.

- **Check Workspace Status:**
  ```bash
  gestalt status
  ```
  Provides a top-level summary of active options, configuration health, recent runs count, and provider credential warnings.

- **Workspace Diagnostics (`workspace` subcommands):**
  - **Info**: List paths to configuration and data files.
    ```bash
    gestalt workspace info
    ```
  - **Snapshot**: Capture git metadata and workspace state.
    ```bash
    gestalt workspace snapshot
    ```
  - **Doctor**: Run syntactical, file-presence, credential, and permission diagnostics.
    ```bash
    gestalt workspace doctor
    ```

You can use `--format json` to get machine-readable output envelopes for any of these commands.

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