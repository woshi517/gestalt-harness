# `gestalt-kernel`

Invariant, sans-I/O microkernel for the Gestalt agent harness.

`gestalt-kernel` owns only mechanism, state integrity, and trust boundaries. It performs **zero** network I/O, **zero** filesystem I/O, **zero** subprocess spawning, and **zero** async runtime scheduling.

## Hard Constraints

- `#![forbid(unsafe_code)]`
- `#![deny(missing_docs)]`
- `<= 2` direct dependencies (`serde`, `serde_json`) — no Tokio, no HTTP client, no OS-specific crates.

## Scope & Responsibilities

| Module | Responsibility | Key Types & Traits |
| :--- | :--- | :--- |
| `lib.rs` | Core kernel state machine driving Turns, Steps, Phases, policy checks, and steering | `KernelRuntime`, `PhaseLoopContext` |
| `tree.rs` | Append-only, content-addressed `SessionTree` with branching (`/fork`, `/rewind`) | `SessionTree`, `SessionNode`, `SessionSlice`, `NodePayload`, `TurnStatus`, `BranchAction` |
| `effect.rs` | Typed effect bus yielded by user-space loops; never executed directly by loops | `Effect`, `EffectEnvelope`, `StepResult`, `ModelKind`, `SideEffectClass`, `SaveMode`, `PauseReason`, `BudgetCap`, `BudgetSnapshot`, `Usage` |
| `policy.rs` | Fail-closed pre-dispatch evaluation of budgets, tool ACLs, and verification rules | `PolicyGate`, `PolicyRule`, `PolicyDecision`, `PolicyOverride`, `SandboxPolicy`, `BatchItemDecision` |
| `extension.rs` | Unified user-space contract for loops, decorators, builders, tools, triggers, policies, and UI | `Extension`, `ExtensionRegistry`, `AgentLoop`, `AgentLoopFactory`, `LoopDecorator`, `ContextBuilder`, `ContextLayer`, `Preset`, `ToolSpec`, `TriggerSpec`, `UISurface` |
| `steering.rs` | Mid-run steering, follow-ups, and clean interruption with orphan tool-call synthesis | `SteeringInbox`, `SteeringSignal`, `SteeringKind`, `SteeringSource`, `synthesize_aborted_results` |
| `event.rs` | Protocol-neutral kernel-to-host directives and client event stream | `DriverDirective`, `DriverResponse`, `EventStream`, `RunEvent` |
| `replay.rs` | Offline deterministic trace verification and divergence reporting | `ReplayEngine`, `ReplayMode`, `ReplayReport`, `ReplayDivergence`, `DivergenceKind` |
| `sha256.rs` | Pure-Rust SHA-256 and canonical JSON hashing for `SessionTree` node digests | `Sha256`, `canonical_json`, `compute_node_digest`, `sha256_hex` |
| `error.rs` | Structured kernel and extension error types | `KernelError`, `ExtensionError` |

## What Does *Not* Belong Here

- LLM prompts, chat `messages[]` formatting, or `SKILL.md` / `MEMORY.md` conventions.
- Concrete `AgentLoop` implementations, tools, or vendor provider adapters (`anthropic`, `openai`, `jev`).
- Tokio, HTTP/SSE, OS sandbox drivers, or TUI/RPC/ACP protocol servers.

## Verification

```bash
# Run kernel unit & invariant tests
cargo test -p gestalt-kernel

# Audit direct dependency ceiling (must remain <= 2: serde, serde_json)
cargo tree -p gestalt-kernel --depth 1
```
