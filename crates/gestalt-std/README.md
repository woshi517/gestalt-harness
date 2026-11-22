# `gestalt-std`

Optional, Cargo-feature-gated standard userland for Gestalt.

`gestalt-std` provides the batteries-included loops, context builders, composable decorators, workspace tools, triggers, model provider adapters, and UI/protocol surfaces. Crucially, it **dogfoods the exact same public contracts** (`gestalt_kernel::Extension`, `gestalt_runtime::RuntimeRegistry`, `gestalt_runtime::ProviderRegistry`, `gestalt_runtime::TriggerHandler`) as any external user-space crate—with zero private kernel backdoors.

## Hard Constraints

- `#![forbid(unsafe_code)]`
- `#![deny(missing_docs)]`
- Every subsystem is gated behind Cargo features so embedders can include only what they need.

## Cargo Features

| Feature | Default | Contents |
| :--- | :--- | :--- |
| `loops` | Yes | `ChatLoop`, `DecisionRouter`, `StandardBuilder`, `SlidingWindowBuilder`, `DecisionStateBuilder`, `CompactionDecorator`, `GoalCheckDecorator`, `CuratorDecorator`, `StdExtension`, slash commands, and `interval`/`cron_schedule`/`fs_watch`/`webhook` triggers |
| `tools` | Yes | 8 workspace tools (`read`, `write`, `edit`, `glob`, `grep`, `bash`, `bash_output`, `bash_kill`) via `StdToolsExtension`, plus `EvolutionToolsExtension` (`ReplayVerifyTool`) |
| `providers` | Yes | `AnthropicProvider`, `OpenAiProvider`, `JevProvider` (`register_std_providers`) |
| `surfaces` | Yes | `print`, `replay`, `rpc` (stdio JSON-RPC 2.0), and `acp` (Agent Client Protocol v1) surfaces plus `SurfaceExitCode` |
| `tui` | Yes | Interactive terminal UI (`ratatui` + `crossterm`), implies `surfaces` |

## Scope & Responsibilities

- **Loops & Presets (`extensions::loops`, `extensions::presets`):** `ChatLoop` (generative multi-turn loop), `DecisionRouter` (fast non-generative `ModelKind::Decision` router with `Effect::Switch` escalation), `general` and `triage` presets, and `std_ui_surface`.
- **Context Builders & Decorators (`extensions::builders`, `extensions::decorators`):** 3-tier cache-stable prompt assembly (`StandardBuilder`), `SlidingWindowBuilder`, `DecisionStateBuilder`, lossless `CompactionDecorator`, `GoalCheckDecorator`, and `CuratorDecorator`.
- **Tools (`extensions::tools`):** `StdToolsExtension` registering `read`, `write`, `edit`, `glob`, `grep`, `bash`, `bash_output`, and `bash_kill` with `WorkspacePathGuard` and `BackgroundTaskManager`.
- **Triggers & Commands (`extensions::triggers`, `extensions::commands`):** `IntervalTriggerHandler`, `CronScheduleTriggerHandler`, `FsWatchTriggerHandler`, `WebhookTriggerHandler`, and standard slash commands (`/model`, `/thinking`, `/login`, `/logout`, `/compact`, `/goal`, `/gate`, `/skills`).
- **Providers (`extensions::providers`):** SSE/HTTP adapters for Anthropic Messages, OpenAI Responses/Chat Completions, and Jev Decision APIs.
- **Surfaces (`surfaces::*`):** Protocol and UI adapters (`tui`, `print`, `rpc`, `acp`, `replay`) that consume `RuntimeHost`, `RunEvent`, `PauseHandler`, and `UISurface` generically—independent of `loops`, `tools`, or `providers`.

## Verification

```bash
# Full feature set
cargo test -p gestalt-std

# Minimal surface-only build check
cargo check -p gestalt-std --no-default-features --features surfaces
```
