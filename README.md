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
- [Skills](#skills)
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

## Configuration

All harness settings live in a single `gestalt.json` file. Two scopes are supported:

| Scope | Path | Purpose |
|-------|------|---------|
| Global | `~/.config/gestalt/gestalt.json` | System-wide defaults (providers, profiles) |
| Workspace | `<project-root>/gestalt.json` | Per-project overrides (policy, context, tools) |

Precedence (lowest to highest): built-in defaults < global `gestalt.json` < workspace `gestalt.json` < `GESTALT_*` env vars < CLI flags. The global file is created automatically on first config-aware CLI use. Use `gestalt config explain` to see which source provides each value.

The JSON Schema is available at `docs/schemas/gestalt.schema.json`.

### Automatic Context Management

When `context.management.enabled` is on, `gestalt-harness` may rewrite the provider-visible prompt before each turn to stay within the usable context window.

- Canonical session history is not deleted.
- Provider-visible context may replace older tool output with compact tombstones.
- If pressure remains high, the runtime may replace an older history range with a structured checkpoint summary.
- Recent turns remain protected by `keep_recent_turns` and `keep_recent_tokens`.
- Projection manifests and compaction checkpoints are persisted to the run artifact directory when durability is enabled.

This creates four practical layers:

- Canonical history: the full committed conversation log.
- Derived artifacts: projection manifests and checkpoint files persisted for replay and debugging.
- Context preparation: the runtime step that clears or compacts older context to fit budget.
- Provider-visible context: the exact prompt projection sent to the model for the next turn.

If you see a tombstone or checkpoint in the prompt, it means the runtime reduced visible context size. The original committed history was not erased.

### Provider Connections and Profiles

`gestalt-harness` features a provider connection and credential-backed profile system to securely manage API keys and switch between model environments without storing raw secrets in config files.

- **Connect to a Provider:**
  ```bash
  gestalt connect openrouter
  # Or connect non-interactively:
  gestalt connect openrouter --api-key <key> --set-default
  ```
  This stores the provider configuration in `~/.config/gestalt/gestalt.json` and the API key in your OS keychain (never written to plaintext config). The config file stores only an `auth_ref: "secret:provider/<name>"` pointer.

- **List Profiles:**
  ```bash
  gestalt profiles list
  ```
  Lists all available profiles and highlights the currently active one.

- **Switch Profiles:**
  ```bash
  gestalt profiles use <profile-name>
  ```
  Sets the active profile in `gestalt.json` (workspace if one exists, otherwise global).

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
  Scaffolds a new workspace containing `gestalt.json` (unified config with defaults, profiles, and policies), plus `.gestalt/workspace.md` and `.gestalt/memory.md` content files.

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

## Extensions & Runtime Composition

Gestalt extensions are packages containing typed runtime components such as command tools, MCP servers, lifecycle components, skills, and optional client/product descriptors. Legacy V1 process extensions remain supported through compatibility activation.

Manage and diagnose extensions with the following CLI commands:

- **List Extensions**:
  ```bash
  gestalt extension list
  ```
  Lists discovered extensions, their paths, versions, and enabled/disabled status.

- **Enable/Disable an Extension**:
  ```bash
  gestalt extension enable <extension-id>
  gestalt extension disable <extension-id>
  ```
  Enables or disables an extension by updating the workspace config.

- **Inspect an Extension**:
  ```bash
  gestalt extension inspect <extension-id>
  ```
  Inspects declared capabilities and permission requirements for a given extension.

- **Validate a Manifest**:
  ```bash
  gestalt extension validate <path-to-gestalt.extension.toml>
  ```
  Performs syntax validation and integrity checking on an extension manifest.

- **Inspect Active Runtime**:
  ```bash
  gestalt runtime inspect
  ```
  Lists the currently active registry snapshot, including loaded tools, hooks, and context contributors.

- **Diagnose Runtime Configuration**:
  ```bash
  gestalt runtime doctor
  ```
  Runs preflight checks on extensions, verifying executables, paths, and manifest safety.

---

## Skills

Skills are passive instruction packages that progressively load task-specific workflows into the runtime. The harness discovers them, exposes only lightweight metadata at startup, and activates full instructions on demand. Skills can declare a tool allow-list to narrow the visible tool catalog while active.

**Discovering skills.** Skills are discovered from explicit paths, the workspace-local `.gestalt/skills/` (and `.agents/skills/`) directory, and the global `~/.config/gestalt/skills/` directory. Manifests follow the [Agent Skills](https://agentskills.io) `SKILL.md` format (YAML frontmatter + Markdown body).

**Managing skills:**

- **List skills**:
  ```bash
  gestalt skill list
  ```
  Lists discovered skills with name, description, trust level, and source path.

- **Inspect a skill**:
  ```bash
  gestalt skill inspect <name>
  ```
  Shows the manifest metadata, `allowed-tools` frontmatter, and the resolved skill root.

- **Validate a skill package**:
  ```bash
  gestalt skill validate <path-to-skill-dir>
  ```
  Performs offline validation against the Agent Skills spec (frontmatter, naming, directory match).

**Activating skills:**

- **Activate for a single run**:
  ```bash
  gestalt run --skill pdf-processing
  ```
  Activates the named skill(s) for that run only.

- **Activate in an interactive session**:
  ```
  /skill pdf-processing      # activate
  /skill off pdf-processing  # deactivate
  ```
  Slash commands toggle the active skill set for the current chat session.

**How it works.** Skill metadata (name, description, trust level, source) is loaded at startup and exposed to the model as a `SessionStatic` index. When a skill is activated — either explicitly, via CLI flag, via slash command, or via deterministic trigger matching on the current task — the full `SKILL.md` body is injected as `ActivationStatic` context, and the active skill set drives tool catalog filtering. Resources (`scripts/`, `references/`, `assets/`) are demand-loaded through ordinary file tools, never auto-injected. Resumes and replays reject runs whose active-skill state does not match the recorded fingerprint.

Skill-declared `allowed-tools` is treated as a narrowing hint and intersected with the workspace policy. Off-skill tool calls are denied at policy time even if the provider emits them.

---

## Documentation

| Document | Description |
|---|---|
| [Product Requirements Document](docs/gestalt-harness-prd.md) | Goals, scope, and requirements |
| [Architecture Document](docs/gestalt-harness-architecture.md) | System design and crate layout |
| [Implementation Roadmap](docs/gestalt-harnes-implementation-roadmap.md) | Milestones and planned work |
| [Config JSON Schema](docs/schemas/gestalt.schema.json) | Machine-readable schema for `gestalt.json` |
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
