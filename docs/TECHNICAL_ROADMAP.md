# Technical Roadmap: Gestalt v2 (Rust Rewrite)

- **Product:** Gestalt — A Microkernel Agent Harness for Dynamic Semantics
- **Status:** Draft v1.0
- **Aligned Specifications:** [docs/PRD.md](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/PRD.md), [docs/ARCHITECTURE.md](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/ARCHITECTURE.md)
- **Target Stack:** Rust Workspace (MSRV 1.85+, Edition 2024), Minimal Dependencies, Sans-I/O Microkernel

---

## 1. Enforced Architectural Standards

Before any code is written, every crate, PR, and extension MUST uphold these four non-negotiable standards:

1. **The Linux Kernel Philosophy (Kernel + Driver Primitives vs. Default Userland):**
   - **`gestalt-kernel` (Core Kernel):** Owns **only bare-bones mechanism, scheduling, and trust**: `SessionTree` + `Steering` Inbox, typed `Effect` bus, fail-closed `PolicyGate`, `Sandbox` (`protected_paths`) & `Save` boundary, `EventStream` (`StreamDelta`) & offline replay, and extension trait contracts (`AgentLoop`, `AgentLoopFactory`, `LoopDecorator`, `ContextBuilder`, `Extension`, `UISurface`).
   - **`gestalt-runtime` (Bare-Minimum Host Drivers):** Owns **only the essential I/O and OS driver primitives** required to run the kernel on a machine: the Tokio `Effect` driver, `ext_ipc` (stdio JSON-RPC + MCP bus), `gestalt.toml` loader & trust gate, `SandboxDriver` (`bubblewrap`/`Seatbelt`), `BlobStore`, `CredentialStore`, and `ProviderRegistry`.
   - **`gestalt-std` + `gestalt-cli` (Built-In Default Userland):** Shipped in-repo so Gestalt works out of the box as a complete product (standard loops, decorators, builders, presets, tools, vendor provider adapters, and print/RPC/ACP/TUI surfaces), using the exact same public driver and `Extension` traits as third-party code.
   - Neither `gestalt-kernel` nor `gestalt-runtime` may contain chat `messages[]`, LLM-specific output structs (`reasoning`, `tool_calls`), markdown conventions (`MEMORY.md`, `SKILL.md`), specific tool names (`bash`, `read`), vendor adapters, or UI surfaces.
   - Extension points in `gestalt-kernel` stay generic: `LoopConfig::params: Value`, `StepResult::Model { kind, output: Value, usage }`, `RunEvent::StreamDelta { channel, payload }`, `Sandbox::protected_paths`, and namespaced `SessionTree` node kinds (e.g., `"std:compaction"`).
2. **Composable Loop & Self-Extension Architecture:**
   - Adding a new loop type implements `AgentLoopFactory` cleanly without duplicating cross-cutting behaviors.
   - Cross-cutting behaviors (`goal_check`, `compaction`, `curator`) are implemented as `LoopDecorator`s wrapping *any* `Box<dyn AgentLoop>`.
   - `Preset { base_loop, decorators, builder, config }` dynamically wires loops, decorators, and context builders. `ChatLoop` (`"chat"`) is simply the optional default option in `gestalt-std`, never bolted into the kernel or runtime.
   - **Self-Extension Without Recompiling Core Crates:** An agent can author new out-of-process extensions (`loops`, `decorators`, `builders`, `tools`, `policies`, `commands`, `ui` `UISurface` specs) or `skills/` via `Effect::Save`, register them in `<workspace>/.gestalt/gestalt.toml` (subject to `PolicyGate` and workspace trust approval), and hot-load them live via `/reload <ext>`.
3. **Generalized Naming Standard:**
   - Code identifiers, loop IDs, builder names, and presets MUST use vendor-neutral, generalized names (`chat`, `decision_router`, `swe`, `general`, `minimal_shell`, `DecisionStateBuilder`, `StandardBuilder`, `SlidingWindowBuilder`).
   - Third-party product names appear **only** inside concrete provider adapter modules in `gestalt-std/src/extensions/providers/` (e.g., `anthropic.rs`, `openai.rs`, `jev.rs`) and in documentation citations.
4. **Rust Quality, Safety & Minimalism (`AGENTS.md` Compliance):**
   - `#![forbid(unsafe_code)]` across the entire workspace.
   - Zero `.unwrap()` in library/production code (`thiserror` for library errors, `anyhow` for `gestalt-cli`).
   - `gestalt-kernel` hard CI limits: **≤ 2,000 non-blank, non-comment LOC**, **≤ 2 direct crate dependencies** (`serde`, `serde_json`).
   - `cargo test`, `cargo build`, `cargo clippy -- -D warnings`, and `cargo fmt --check` must pass with zero warnings on every milestone.

---

## 2. Rust Workspace Architecture

```text
gestalt-harness/
├── Cargo.toml                        # Workspace root (resolver = "2", release LTO + strip)
├── crates/
│   ├── gestalt-kernel/               # Sans-I/O invariant microkernel (<= 2,000 LOC, <= 2 deps)
│   │   └── src/
│   │       ├── lib.rs                # Public API re-exports & KernelRuntime state machine
│   │       ├── sha256.rs             # ~80-line pure safe Rust SHA-256 + canonical JSON hasher
│   │       ├── tree.rs               # SessionTree (append-only JSONL, /fork, /rewind, Spawn branches)
│   │       ├── effect.rs             # Effect, ModelKind, SideEffectClass, WorkspaceMode, StepResult
│   │       ├── policy.rs             # Immutable fail-closed PolicyGate, BudgetCap, PolicyDecision
│   │       ├── steering.rs           # SteeringSignal (Steer, FollowUp, Abort) & Inbox
│   │       ├── event.rs              # RunEvent (canonical + StreamDelta) & EventStream sink
│   │       ├── extension.rs          # AgentLoop, AgentLoopFactory, LoopDecorator, ContextBuilder, Preset, Extension, ExtensionRegistry
│   │       └── replay.rs             # Deterministic offline replay engine + divergence reporter
│   ├── gestalt-runtime/              # Reusable Tokio I/O host & capability primitives (zero hardcoded features)
│   │   └── src/
│   │       ├── lib.rs                # Public API re-exports, RuntimeError, ProviderErrorKind & CancellationToken
│   │       ├── driver/               # RuntimeHost, RuntimeRegistry, dispatch, session_io (.lock), worktree, replay
│   │       ├── config.rs             # Global/Workspace gestalt.toml loader, merge precedence & trust gate
│   │       ├── artifacts.rs          # ArtifactStoreView snapshots, Effect::Save staging/ledger.jsonl & /rollback
│   │       ├── stdio_rpc.rs          # Shared sandboxed stdio JSON-RPC 2.0 child-process transport
│   │       ├── ext_ipc.rs            # Out-of-process stdio JSON-RPC extension host (describe, loop/step, /reload)
│   │       ├── mcp.rs                # Official MCP 2025-11-25 stdio bridge (negotiate, paginated tools/list, tools/call)
│   │       ├── blobs.rs              # Content-addressed .gestalt/blobs/<sha256> spill, snapshots & preview
│   │       ├── credentials/          # Composable CredentialStore (ApiKey/OAuth/Headers/Custom + CredentialResolver) & StreamRedactor
│   │       ├── provider.rs           # ModelProvider, ProviderContext, ProviderRegistry, SharedHttpClient (JSON/SSE)
│   │       └── sandbox/
│   │           ├── mod.rs            # SandboxDriver trait, WorkspacePathGuard (in-process), protected_paths
│   │           ├── linux_bwrap.rs    # bubblewrap (--unshare-pid/ipc/uts/net, --ro-bind protected_paths)
│   │           └── macos_seatbelt.rs # sandbox-exec dynamic SBPL generator
│   ├── gestalt-std/                  # Unified optional crate (Cargo features: loops, tools, providers, surfaces)
│   │   └── src/
│   │       ├── lib.rs                # Feature-gated re-exports + StdExtension / StdToolsExtension
│   │       ├── extensions/
│   │       │   ├── loops/            # ChatLoop ("chat"), DecisionRouter ("decision_router")
│   │       │   ├── decorators/       # goal_check, compaction, curator
│   │       │   ├── builders/         # StandardBuilder, SlidingWindowBuilder, DecisionStateBuilder
│   │       │   ├── presets.rs        # swe, general, minimal_shell presets + protected_paths defaults
│   │       │   ├── tools/            # read, write, edit, glob, grep, bash, bash_output, bash_kill
│   │       │   └── providers/        # anthropic.rs, openai.rs, jev.rs (ModelProvider impls)
│   │       └── surfaces/
│   │           ├── print.rs          # -p / --json non-interactive surface
│   │           ├── rpc.rs            # --mode rpc stdio JSON-RPC 2.0 server
│   │           ├── acp.rs            # --mode acp Agent Client Protocol v1 server
│   │           ├── replay.rs         # gestalt replay <session.jsonl> surface
│   │           └── tui/              # #[cfg(feature = "tui")] ratatui + crossterm TUI with mouse/scroll
│   └── gestalt-cli/                  # Thin composition root binary (~100 lines)
│       └── src/
│           └── main.rs               # Minimal CLI parser wiring gestalt-runtime + selected gestalt-std features
└── tests/
    ├── golden_replays/               # >= 10 deterministic SessionTree JSONL fixtures
    └── integration/                  # Security/sandbox adversarial suite, ACP/RPC conformance, ext IPC tests
```

---

## 3. Critical Technical Contracts

### 3.1 Sans-I/O Kernel Execution, Trait Contracts & `DriverDirective`
`gestalt-kernel` never awaits futures or performs disk/network I/O. The outer `tokio` driver (`gestalt-runtime/src/driver/`) drives the state machine in a tight loop:

```text
1. driver polls kernel.next_action(pending_results) -> DriverDirective:
   - DispatchEffects(Vec<ApprovedEffect>)   // execute in parallel via providers/tools/spawn
   - VerifyEffect { rule_id, verifier, pending }
                                            // execute verifier (e.g. Jev decision check) first;
                                            // kernel calls PolicyRule::evaluate_verification();
                                            // on error/timeout, fail closed to StepResult::Denied
   - InvokeExternalLoop { ext_name, request }
                                            // async stdio JSON-RPC `loop/step` deferral
   - InvokeExternalBuilder { ext_name, request }
                                            // async stdio JSON-RPC `builder/build` deferral
   - WaitForPause { pause_id, reason, payload }
   - TurnComplete { output }
   - TurnAborted { reason }
2. During DispatchEffects:
   - Providers/tools emit transient RunEvent::StreamDelta { turn_id, phase_id, effect_id, channel, payload }
     directly to EventStream.
   - CancellationToken aborts in-flight HTTP/subprocess I/O within <= 250 ms; kernel synthesizes
     StepResult::Aborted for any unpaired effect_id / tool_call IDs.
3. Completed StepResults (with payloads > 32 KB spilled to .gestalt/blobs/<sha256>) are fed back
   into kernel.next_action(Some(results)), returning serialized JSONL lines for `gestalt-runtime`
   to flush atomically to disk.
```

**Core Kernel & Registry Types (`gestalt-kernel`):**
- **`EffectEnvelope` & `StepResult`:** Every yielded `Effect` is wrapped in `EffectEnvelope { effect_id: String, effect: Effect }` (assigned deterministically via `ctx.next_id()`) so parallel batches correlate 1:1 with `StepResult`:
  - `StepResult` variants (`#[serde(tag = "status", rename_all = "snake_case")]`): `Model { effect_id, kind, output, usage }`, `Tool { effect_id, output, is_error, blob_digest }`, `Saved { effect_id, stage_id, diff_summary, digest }`, `SpawnCompleted { effect_id, child_head_id, worktree_path, diff_blob_digest, output, usage }`, `Switched { effect_id, new_phase_id }`, `PauseResolved { effect_id, resolution }`, `Denied { effect_id, reason }`, `Aborted { effect_id, reason }`, `Error { effect_id, message, retryable }`.
- **`RunEvent` (`event.rs`):**
  - Canonical events: `NodeAppended { node_id, kind }`, `PhaseSwitched { phase_id, preset_id, base_loop_id }`, `PolicyDenied { effect_id, reason }`, `Paused { pause_id, reason, payload }`, `TurnCompleted { turn_id, output, usage }`, `TurnAborted { turn_id, reason }`.
  - Transient events (not persisted to JSONL): `StreamDelta { turn_id, phase_id, effect_id, channel, payload }` and `ToolUi { turn_id, effect_id, renderer, data }`.
- **`PolicyRule`:**
  - `fn evaluate(&self, effect: &EffectEnvelope, session: &SessionSlice) -> PolicyDecision`
  - `fn evaluate_verification(&self, pending: &EffectEnvelope, verifier_result: &StepResult) -> PolicyDecision` (defaults to fail-closed `PolicyDecision::Deny`).
- **`BudgetCap`, `BudgetSnapshot` & `Usage`:**
  - `BudgetCap { max_steps: Option<u64>, max_tokens: Option<u64>, max_cost_micro_usd: Option<u64> }` (where `1_000_000 micro_usd = $1.00 USD`, avoiding float drift in canonical hashes). `Usage { input_units: u64, output_units: u64, cost_micro_usd: u64 }`.
- **`LoopContext` & `ExtensionRegistry`:**
  - `AgentLoop::step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction`.
  - `LoopContext` exposes `run_id()`, `turn_id()`, `phase_id()`, `handoff()`, `config()`, `session() -> &SessionSlice`, `artifacts() -> &ArtifactStoreView`, `budget_remaining() -> &BudgetSnapshot`, `build_context(&self, caps: &ModelCapabilities) -> Value`, `drain_steering()`, `now_ms() -> u64`, and `next_id() -> String`.
  - `ExtensionRegistry` in `gestalt-kernel` stores declarative descriptors for all slots: `loops`, `decorators`, `presets`, `builders`, `policies`, `tools: BTreeMap<String, ToolSpec>`, `commands: BTreeMap<String, CommandSpec>`, and `ui: UISurface` (with `UIRendererSpec`, `UIPanelSpec`, `UIDialogSpec`, `UIActionSpec`). `gestalt-runtime::RuntimeRegistry` binds async execution handlers (`ToolHandler`, `CommandHandler`, `ModelProvider`, `SandboxDriver`) by slot name.
  - `SessionSlice` provides read-only access to the active branch's nodes from root to head; `ArtifactStoreView` provides read-only `get(path) -> Option<ArtifactEntry>` backed by live files (during live runs) or blob snapshots (during replay). `ExtensionError` implements `std::error::Error` + `Display` manually in `gestalt-kernel` to keep kernel dependencies at `<= 2`.
- **Steering Inbox Semantics (`steering.rs`):**
  - `KernelRuntime` appends the canonical `steering` node to `SessionTree` immediately when a `SteeringSignal` is enqueued (guaranteeing crash-safe durability and deterministic replay).
  - `ctx.drain_steering()` returns and marks consumed all pending `Steer` / `UrgentSteer` signals for the active `AgentLoop`'s `ContextBuilder`.
  - Un-drained `Steer` signals carry across `Effect::Switch` into the next Phase; if the Turn ends (`LoopAction::Done`) with un-drained `Steer` signals still in the `Inbox`, `KernelRuntime` auto-promotes them to `FollowUp` for Turn `N+1` so user input is never silently dropped.

### 3.2 Dynamic `Preset` & `LoopDecorator` Resolution
When opening a `Phase` for a preset (e.g., `swe` + `/goal`):
1. Look up `base_loop` in `ExtensionRegistry::loops` (`AgentLoopFactory::create`).
2. Fold `preset.decorators` (plus any runtime decorators like `goal_check`) in order via `LoopDecorator::wrap(inner, &config, &registry)`.
3. Record `{ phase_id, preset_id, base_loop_id, loop_digest, decorator_digests, builder_id, builder_digest, config }` on the `SessionTree` `phase_open` node so offline replay reproduces the exact decorated stack.

### 3.3 `gestalt.toml` Schema, Resolution, Trust Gate & Out-of-Process IPC
- **Complete Reference Schema (`gestalt.toml`):**
  ```toml
  [defaults]
  preset = "swe"                   # Active default preset
  session_dir = ".gestalt/sessions"

  [budget]
  max_steps = 100
  max_tokens = 250000
  max_cost_usd = 5.0               # Converted to integer micro_usd (5_000_000)

  [sandbox]
  network = false                  # Denied by default
  protected_paths = [".gestalt/memories/", ".gestalt/skills/"]

  [policy]
  headless_auto_approve = []       # Explicit Pause reasons/actions allowed in -p mode (default: none)

  [providers.anthropic]
  adapter = "anthropic"            # Optional: defaults to provider key; lets aliases share an adapter
  model = "claude-sonnet-4-5"      # API keys/OAuth/custom creds come ONLY from CredentialStore/env
  input_cost_per_mtok_usd = 3.0
  output_cost_per_mtok_usd = 15.0

  [providers.jev]
  endpoint = "https://api.typesafe.ai/v1/decision"
  model = "jev-1.13.0"

  [presets.custom_swe]
  base_loop = "chat"
  decorators = ["compaction"]
  builder = "standard"
  allowed_tools = ["read", "write", "edit", "glob", "grep", "bash"]
  max_spawn_depth = 1

  [extensions.my_router]
  command = ["python3", "./ext/router.py"]
  protocol = "rpc"                 # "rpc" | "mcp"
  timeout_ms = 5000
  ```
- **Resolution order:** `gestalt-std` (optional) → Global (`~/.config/gestalt/gestalt.toml`, trusted) → Workspace (`<workspace>/.gestalt/gestalt.toml`, untrusted until approved) → CLI `--ext`.
- **Policy monotonicity:** `KernelBaseline <= Global <= Workspace` (later tiers may only narrow `allowed_tools`, lower `BudgetCap`, add `protected_paths`, or add stricter `PolicyRule`s).
- **Trust allowlist:** Stored in `~/.config/gestalt/trusted_workspaces.json` keyed by `(canonical_workspace_path, sha256(gestalt.toml))`. Headless CI accepts `--trust-workspace`.
- **Complete Out-of-Process Stdio JSON-RPC 2.0 Method Table:**
  - `extension/describe` → `{ name, loops, decorators, presets, builders, tools, commands, policies, ui }`
  - `loop/step` `{ phase_id, turn_id, handoff, config, state_token, results }` → `{ action: LoopAction, state_token }` (`state_token` persisted on `SessionTree` for crash recovery & replay)
  - `decorator/wrap_step` `{ decorator_id, phase_id, inner_action, config, state_token }` → `{ action: LoopAction, state_token }`
  - `builder/build` `{ builder_id, session_slice, artifacts, caps }` → `ModelInput` JSON
  - `tool/call` `{ tool_name, args, cwd, blob_dir }` → `StepResult` (worker may emit `stream/delta { channel, payload }` notifications while running)
  - `policy/evaluate` `{ effect, session_slice }` → `PolicyDecision` (timeout/crash fails closed to `Deny`)
  - `command/run` `{ command, args, session_head }` → command result / `SteeringSignal`
  - `extension/shutdown` → graceful teardown (used by `/reload <ext>` between Turns/Phases)

### 3.4 `SessionTree` JSONL Schema, Canonical Hashing & Offline Replay
- **Canonical Node Envelope & Kinds:**
  - Envelope: `{ id, parent_id, turn_id, phase_id, timestamp_ms, kind, payload, digest }` where `digest = sha256(parent_id || kind || canonical_json(payload))`.
  - **Canonical JSON Rule:** `canonical_json` recursively sorts all JSON object keys lexicographically (`BTreeMap`), omits insignificant whitespace, and uses integer representations (`timestamp_ms: u64`, `cost_micro_usd: u64`) in canonical fields to guarantee cross-language hash determinism.
  - **Canonical Node Kinds & Payloads:**
    - `session_open`: `{ schema_version: 1, workspace_root, created_at_ms }`
    - `turn_start`: `{ turn_id, prompt, artifact_snapshots: BTreeMap<String, String> }` (records SHA-256 blob digests of every workspace/skill/memory artifact read during the Turn)
    - `phase_open`: `{ phase_id, preset_id, base_loop_id, loop_digest, decorator_digests, builder_id, builder_digest, config }`
    - `phase_seal`: `{ phase_id, handoff }`
    - `effect`: `{ effect_id, op, ... }`
    - `step_result`: `{ effect_id, status, ... }`
    - `steering`: `{ id, kind, content, source }`
    - `pause`: `{ pause_id, reason, payload }`
    - `pause_resolved`: `{ pause_id, resolution }`
    - `turn_end`: `{ turn_id, status, output, usage }`
    - `branch_head`: `{ branch_id, target_node_id, action }` (for `/fork` & `/rewind`)
    - `custom:<ns>`: user-space namespaced nodes (e.g., `std:compaction { summary, compacted_up_to_node_id }`).
- **Crash-Safe Persistence & Locking:** `gestalt-kernel/src/tree.rs` operates purely on in-memory nodes and JSONL strings; `gestalt-runtime` acquires an advisory `.gestalt/sessions/<session_id>.lock` file and flushes each compact JSON line + `\n` atomically. On load, if the final line is truncated, the loader discards the partial trailing line, validates the SHA-256 chain, and logs a warning.
- **Deterministic Clock, IDs & Offline Replay (`replay.rs`):** `gestalt-kernel` and `AgentLoop`s MUST NEVER call `SystemTime::now()` or OS RNG directly (`ctx.now_ms()`, `ctx.next_id()`). During `gestalt replay <session.jsonl>`:
  - `ArtifactStoreView` resolves all artifact reads strictly from `.gestalt/blobs/<sha256>` using `turn_start.artifact_snapshots`, ensuring repository edits after recording never cause false replay divergence.
  - **Default Mode (`gestalt replay`):** Validates the SHA-256 hash chain, `PolicyGate` invariants, and re-executes all registered in-process Rust loops/builders matching `loop_digest`/`builder_digest` with 0 network calls, while verifying recorded `state_token`/`effect` transitions for out-of-process workers.
  - **External Worker Mode (`gestalt replay --reexecute-ext`):** Spawns external IPC workers inside the network-disabled OS sandbox (`--unshare-net`) with recorded `state_token` and `StepResult` payloads to verify bit-identical `Effect` regeneration.

### 3.5 `Effect::Save` Staging, Promotion & Rollback Ledger (`artifacts.rs`)
- **Module Ownership:** Implemented in `crates/gestalt-runtime/src/artifacts.rs`.
- **`ArtifactPatch` Variants:** `WriteFile { content }`, `ApplyDiff { hunks: Vec<DiffHunk> }` (where `DiffHunk { old_start, old_lines, new_lines, target, replacement }`), `Delete`.
- **Staging (`SaveMode::Stage`):** Writes candidate changes into `.gestalt/staging/<stage_id>/` with SHA-256 digests and returns `StepResult::Saved { effect_id, stage_id, diff_summary, digest }`. Does not touch live `protected_paths`.
- **Promotion (`SaveMode::Promote`) & Rollback:** Evaluated by `PolicyGate`. Before overwriting live files in `protected_paths`, records previous file digests in `.gestalt/ledger.jsonl` and snapshots replaced bytes in `.gestalt/blobs/<sha256>`. Users can revert any promoted change atomically via `/rollback <stage_id>` (TUI) or `gestalt rollback <stage_id>` (CLI).

### 3.6 Execution Modes, `Effect::Pause` & Wire Parity
- **Interactive TUI:** Renders the registered `UISurface` dialog (`Approval`, `Input`, `Checkpoint`) and feeds the user's decision back as `pause_resolved`.
- **Headless JSON-RPC (`--mode rpc`):** Emits `RunEvent::Paused { pause_id, reason, payload }` and waits for `pause/resolve`. Supports full parity methods: `turn/start`, `turn/steer`, `turn/cancel`, `phase/switch`, `pause/resolve`, `session/fork`, `session/rewind`, `extension/reload`, `command/run`, and `event/notify` using standard JSON-RPC 2.0 error codes (`-32602` invalid params, `-32001` policy denied, `-32002` session locked).
- **ACP v1 (`--mode acp`):** Maps `session/prompt` → Turn start (slash commands `/mode`, `/steer`, `/fork`, `/rewind`, `/goal` sent as prompt prefixes or ACP extension methods), `session/cancel` → preemptive `Abort`, `EventStream` → `session/update` (including `agent_thought_chunk` and `ToolUi` payloads), and `Effect::Pause` → `session/request_permission`.
- **Non-Interactive Print (`-p` / `--json`):** Without `--json`, streams assistant text deltas to `stdout` and exits `0`; with `--json`, emits one NDJSON `RunEvent` per line. `Effect::Pause` fails closed (`StepResult::Denied`, exit code `3`) unless its reason/action matches `[policy].headless_auto_approve` in `gestalt.toml`.

### 3.7 Provider Error Handling, `CredentialStore`, Compaction & `BudgetCap`
- **Target Resolution & `ModelCapabilities`:** `Effect::Model { target }` resolves `"<provider_or_alias>/<model>"` or a configured `[providers.<alias>]` entry in `gestalt.toml` (with optional `adapter = "<provider_id>"` so multiple endpoints/models compose over one provider adapter). `ModelProvider::invoke` receives a `ProviderContext` (`credentials`, `http: SharedHttpClient`, `blobs: BlobStore`, `redactor`, `cancel`) so providers can resolve multimodal blobs and reuse TLS/SSE streaming without changing the trait. `ModelCapabilities` exposes `{ max_context_tokens, max_output_tokens, supports_prompt_caching, supports_reasoning, supports_vision }`.
- **Composable `CredentialStore`:** Supports extensible `CredentialRecord` variants (`ApiKey`, `OAuth`, `Headers`, `Custom { scheme, fields, secret_fields }`) and pluggable `CredentialResolver`s alongside standard env vars and `~/.config/gestalt/credentials.toml` (enforcing `0600` permissions on Unix). Redacts secrets across `SessionTree`, `EventStream`, and `tracing` using a stateful sliding-window `StreamRedactor` so secrets split across streaming deltas never leak.
- **Error Classification & Malformed Tool Recovery:** Classifies HTTP/SSE failures into `Retryable` (`429`, `529`, transient `5xx`, network reset), `ContextLengthExceeded` (triggers `compaction` decorator), `AuthError`, and `Fatal`. Unknown tool names or invalid tool argument schemas return `StepResult::Tool { is_error: true, output: "validation error..." }` so the LLM can self-correct on the next Step.
- **Compaction & Curator (`v0.1` vs `v0.2`):** `compaction` reads `LoopConfig::params["compaction_threshold"]` (default `0.80`), `keep_recent_turns` (default `2`), and `compaction_model` (defaults to the Phase's active LLM target). In `v0.1`, `curator` stages candidate `SKILL.md`/`MEMORY.md` patches via `SaveMode::Stage` and requests human/policy approval (`Effect::Pause(Approval)`) before `SaveMode::Promote`; automated multi-trace replay regression gating prior to promotion lands in `v0.2`.
- **Monotonic Budget & `Spawn` Accounting:** `Usage` (`cost_micro_usd`) accumulates monotonically per Turn and Session from `[providers.<id>]` rate tables. `Effect::Spawn` child budgets MUST be `<= parent.budget_remaining`, and child usage is deducted upon `SpawnCompleted`. Fallback token estimator: `chars / 4`.

### 3.8 CLI Surface, Slash Commands, Exit Codes & Logging Hygiene
- **CLI Subcommands & Flags:**
  - `gestalt` (interactive TUI if compiled with `--features tui`)
  - `gestalt -p "<prompt>" [--json] [--preset <name>] [--model <provider/model>] [--session <path>]`
  - `gestalt --mode rpc | acp`
  - `gestalt replay <session.jsonl> [--reexecute-ext] [--json]`
  - `gestalt rollback <stage_id>`
  - Global flags: `--version` (`-V`), `--help` (`-h`), `--ext <cmd>`, `--trust-workspace`, `--unsafe-no-sandbox` (logs visible warning to `SessionTree`).
- **Slash Commands:**
  - Kernel-backed: `/mode <preset>`, `/steer [--urgent] <msg>` (`UrgentSteer` cancels in-flight step I/O and injects steering; plain `/steer` applies between steps), `/stop` (Turn abort), `/fork [<node_id>] [--new-file]`, `/rewind <node_id>` (rewinds conversation state only; emits UI notice that disk files are unchanged), `/reload <ext>`, `/rollback <stage_id>`.
  - Extension-registered (`gestalt-std`): `/goal <criteria>` (activates `goal_check` decorator), `/compact` (triggers immediate compaction), `/gate` (displays active `PolicyGate` rules and `BudgetSnapshot`), `/login <provider>` (prompts via `Effect::Pause(Input)` and writes to `~/.config/gestalt/credentials.toml`).
- **Exit Codes:** `0` (success / replay parity), `1` (runtime/provider error / replay divergence), `2` (CLI/config parse error / corrupt replay JSONL), `3` (policy denial / untrusted workspace / headless pause denied), `130` (SIGINT / aborted Turn).
- **Wire-Safe Observability:** All `tracing` diagnostics write strictly to `stderr` (or `.gestalt/logs/`), **never** `stdout`, and pass through the `CredentialStore` redactor.

### 3.9 Two-Layer Sandbox Enforcement & `WorkspaceMode::Worktree` Lifecycle
- **In-Process File Tools (`read`, `write`, `edit`, `glob`, `grep`):** `WorkspacePathGuard` in `gestalt-runtime/src/sandbox/mod.rs` canonicalizes paths, rejects symlink escapes outside `workspace_root`, blocks `Write`/`Delete` on any `protected_paths` prefix, and dynamically elevates `write` to `SideEffectClass::Irreversible` when overwriting an existing file.
- **Subprocess Tools (`bash`, external MCP/RPC workers):** Executed inside `SandboxDriver` (`bwrap` on Linux, `sandbox-exec` Seatbelt on macOS) with `protected_paths` bind-mounted read-only and `--unshare-net` by default.
- **`WorkspaceMode::Worktree` Lifecycle:** `driver/worktree.rs` creates a detached git worktree under `.gestalt/worktrees/<spawn_id>/` (returning `StepResult::Error` if the workspace is not a git repo), computes a unified diff blob (`diff_blob_digest`) on `SpawnCompleted`, and prunes clean temporary worktrees on session close.

### 3.10 Core Built-In Tools (`StdToolsExtension`)
- **8 Registered Tools:** `read` (`path`, `start_line`, `end_line`, `byte_offset`), `write` (`path`, `content`, `overwrite`), `edit` (`path`, `target`, `replacement` → returns structured unified diff metadata for `UISurface`), `glob` (`pattern`, `root`), `grep` (`pattern`, `path`, `glob_filter`), `bash` (`command`, `cwd`, `timeout_ms`, `background: bool`), `bash_output` (`task_id`), `bash_kill` (`task_id`).
- **Background Task Reaping:** Background `bash` tasks default to Turn-scoped (`scope: "turn" | "session"`), buffer output up to the 32 KB inline spill threshold (spilling excess to `BlobStore`), and are automatically reaped (`SIGKILL` to process group) when their scope ends.

### 3.11 Workspace Artifacts (`AGENTS.md`, `MEMORY.md`, `USER.md`, `SKILL.md`)
- **Tier 2 (`context`):** `<workspace>/AGENTS.md` (capped at 16 KB inline).
- **Tier 3 (`volatile`):** `<workspace>/.gestalt/memories/MEMORY.md` and `USER.md` (8 KB cap each), plus `<workspace>/.gestalt/skills/<name>/SKILL.md` (YAML frontmatter `{ name, description, triggers }` indexed in Tier 3; full body loaded on demand).

### 3.12 `DecisionRouter` & Jev (`ModelKind::Decision`) Contract
- **Jev Request (`DecisionStateBuilder`):** `{ "state": Value, "choices": Vec<{ id, description }>, "rubric": Option<String> }` POSTed to `[providers.jev].endpoint`.
- **Jev Response (`DecisionOutput`):** `{ "choice": String, "confidence": f64, "latency_ms": u64 }`.
- **Fast-Path Action Mapping:** `LoopConfig::params["routes"]` maps each `choice` ID to `{ "action": "tool", "name": "...", "args": {...} }` or `{ "action": "switch", "preset": "..." }`. If `confidence < threshold` (default `0.85`) or if the deterministic `Effect::Tool` returns `is_error: true`, `DecisionRouter` yields `Effect::Switch` to the fallback preset (`swe` / `chat`).

---

## 4. Phased Implementation Plan

### Phase 0: Toolchain & Dependency Provisioning

#### Milestone 0 — Workspace Dependency & Environment Provisioning (Pre-Code)
**Goal:** Provision, pin, and audit all Rust crates, OS sandbox binaries, and CI tools before writing implementation code.
- [x] Verify Rust toolchain (MSRV `1.85+`, Edition 2024), `rustfmt`, `clippy`, and `tokei` (for kernel LOC checks).
- [x] Initialize root `Cargo.toml` (`resolver = "2"`, `#![forbid(unsafe_code)]`, strict clippy lints, release `lto = true`, `strip = true`, `codegen-units = 1`) and scaffold `crates/{gestalt-kernel,gestalt-runtime,gestalt-std,gestalt-cli}/Cargo.toml` manifests with pinned dependency ceilings:
  - `gestalt-kernel`: `serde`, `serde_json` (**≤ 2** direct dependencies; implements `std::error::Error` + `Display` manually to preserve the 2-dep ceiling).
  - `gestalt-runtime`: `serde`, `serde_json`, `thiserror`, `tokio`, `reqwest` (`rustls-tls`, `stream`; no OpenSSL), `toml`, `tracing`.
  - `gestalt-std`: `serde`, `serde_json`, `thiserror`, `tokio`, plus `regex-lite` (or `regex`), `ignore`, `similar` behind `#[cfg(feature = "tools")]`, `gestalt-runtime` HTTP/SSE client behind `#[cfg(feature = "providers")]`, and `ratatui` + `crossterm` behind `#[cfg(feature = "tui")]`.
  - `gestalt-cli`: `anyhow`, `tokio`, `tracing-subscriber`.
- [x] Provision & verify OS runtime/test dependencies: `bwrap` (`bubblewrap` on Linux) / `sandbox-exec` (Seatbelt on macOS) and `git` (for `WorkspaceMode::Worktree` isolation).
- [x] Run `cargo fetch` / `cargo check --workspace --all-targets` to generate and audit `Cargo.lock` (no OpenSSL, no heavy proc-macro bloat, no unapproved transitive native C deps).
- [x] Create CI workflow (`.github/workflows/ci.yml`) enforcing all 7 gates in Section 5 (`ubuntu-latest` + `macos-latest`).
- **Exit Gate:** `cargo check --workspace --all-targets` passes with zero warnings; `cargo tree -p gestalt-kernel --depth 1` lists only `serde` and `serde_json`; `bwrap` (or `sandbox-exec`), `git`, and `tokei` are verified on the host; CI pipeline is active.

### Phase 1: `v0.1` Proof Gate MVP

#### Milestone 1.1 — `gestalt-kernel` Sans-I/O Microkernel (Target: Weeks 1–2)
**Goal:** Complete, audited, 100% unit-tested sans-I/O microkernel under 2,000 LOC.
- [x] Implement `sha256.rs` (pure safe Rust SHA-256 + canonical JSON `BTreeMap` serializer/hasher) so `gestalt-kernel` stays at `<= 2` direct dependencies (`serde`, `serde_json`).
- [x] Implement `tree.rs`: pure in-memory + JSONL line parser/serializer for `SessionTree`, content-addressed digests, `/fork`, `/rewind`, namespaced user node kinds, `artifact_snapshots`, and `Spawn` child branch linkage.
- [x] Implement `effect.rs` & `steering.rs`: `Effect`, `EffectEnvelope`, `ModelKind`, `SideEffectClass`, `WorkspaceMode`, `StepResult`, `SteeringSignal` (`Steer`, `UrgentSteer`, `FollowUp`, `Abort`) Inbox, and orphan `tool_call` abort synthesis.
- [x] Implement `policy.rs`: fail-closed `PolicyGate`, integer `micro_usd` `BudgetCap`/`Usage`, `PolicyRule` (`evaluate` + `evaluate_verification`), `PolicyDecision::RequiresVerification`, and `Spawn` depth/workspace checks.
- [x] Implement `extension.rs`: `AgentLoop`, `AgentLoopFactory`, `LoopDecorator`, `LoopContext` (`&mut dyn LoopContext`, `now_ms()`, `build_context()`), `ContextBuilder`, `Preset`, `Extension`, `ExtensionRegistry` (including `ToolSpec`, `CommandSpec`, `UISurface` sub-types), `SessionSlice`, and `ArtifactStoreView`.
- [x] Implement `event.rs`, `replay.rs` & `lib.rs`: `RunEvent`, `DriverDirective` (including `InvokeExternalLoop` & `InvokeExternalBuilder`), and deterministic offline replay engine with node-level divergence reporting.
- **Exit Gate:** `tokei crates/gestalt-kernel` ≤ 2,000 code LOC; `cargo tree -p gestalt-kernel --depth 1` shows only `serde` + `serde_json`; 100% unit test coverage on policy denial, abort synthesis, and replay determinism.

#### Milestone 1.2 — `gestalt-runtime` Host Primitives & Sandboxes (Target: Weeks 3–4)
**Goal:** Reusable, feature-agnostic Tokio `Effect` runtime, OS sandboxing, credential isolation, and dynamic extension host.
- [x] Implement `config.rs`: Global/Workspace `gestalt.toml` loader, slot merge precedence, monotonic policy validation, and `(workspace_path, config_sha256)` trust gate.
- [x] Implement `driver/` (`mod.rs`, `dispatch.rs`, `session_io.rs`, `worktree.rs`, `replay.rs`) & `lib.rs`: `RuntimeHost` / `RuntimeRegistry` and Tokio `Effect` runner supporting parallel tool/spawn execution, real child `Effect::Spawn` turns, `WorkspaceMode::Worktree` lifecycle, `PolicyDecision::RequiresVerification`, `InvokeExternalLoop`/`Builder` IPC deferrals, external `decorator/wrap_step` hooks, mid-step `UrgentSteer` preemption, advisory `.lock` file handling with stale-PID reclamation, and `<= 250 ms` `CancellationToken` abort handling.
- [x] Implement `artifacts.rs`: `ArtifactStoreView` snapshot hashing into `BlobStore`, `Effect::Save` staging (`.gestalt/staging/`), `ApplyDiff` promotion, `.gestalt/ledger.jsonl` rollback snapshots, and `/rollback`.
- [x] Implement `stdio_rpc.rs`, `ext_ipc.rs` & `mcp.rs`: shared sandboxed stdio JSON-RPC 2.0 transport, out-of-process worker host (`extension/describe`, stateful `loop/step`, `decorator/wrap_step`, `stream/delta`, `/reload <ext>`, per-call IPC timeouts), and official MCP `2025-11-25` stdio bridge (version negotiation, paginated `tools/list`, `tools/call` `content` + `structuredContent`, `ping`, `notifications/cancelled`, defaulting unknown tools to `SideEffectClass::Irreversible`).
- [x] Implement `blobs.rs`, `credentials/` (`mod.rs`, `redactor.rs`), and `provider.rs`: content-addressed `.gestalt/blobs/<sha256>` (`32 KB` spill threshold), composable `CredentialStore` (`ApiKey`/`OAuth`/`Headers`/`Custom` + pluggable `CredentialResolver`, env + `0600` `~/.config/gestalt/credentials.toml`, stateful sliding-window `StreamRedactor`), composable `ModelProvider` + `ProviderContext`, `ProviderRegistry` (`adapter` aliases), and `SharedHttpClient` (JSON/SSE).
- [x] Implement `SandboxDriver` (`linux_bwrap.rs`, `macos_seatbelt.rs`) and `WorkspacePathGuard` (`sandbox/mod.rs`) enforcing canonical path + symlink-escape + read-only `protected_paths` checks for in-process tools, default network isolation, `env_clear()`, and fail-closed missing-binary behavior.
- **Exit Gate:** Fail-closed security & sandbox conformance suite passes 100% on both Linux (`bwrap`) and macOS (`Seatbelt`) CI runners (0 bypasses on `protected_paths`, symlink escapes, and `CredentialStore` reads); `gestalt-runtime` compiles with zero references to `gestalt-std`.

#### Milestone 1.3a — `gestalt-std` Loops, Decorators, Builders, Tools & Providers (Target: Weeks 5–6)
**Goal:** Ship optional core agent loops, context builders, 8 workspace tools, and 3 vendor provider adapters behind Cargo features (`loops`, `tools`, `providers`) with zero private backdoors.
- [x] Implement `loops` & `decorators`: `ChatLoop` (`"chat"` with malformed tool-call self-correction), `DecisionRouter` (`"decision_router"` with route table & fallback escalation), `goal_check`, `compaction` (emitting `"std:compaction"` node), and `curator` (`v0.1` staged `Effect::Save(Stage)` + `Pause(Approval)`).
- [x] Implement `builders` & `presets`: 3-tier cache-stable `StandardBuilder` (`AGENTS.md`, `MEMORY.md`, `USER.md`, `SKILL.md` budgets), `SlidingWindowBuilder`, `DecisionStateBuilder`, generalized presets (`swe`, `general`, `minimal_shell`), default `protected_paths`, and commands (`/goal`, `/compact`, `/gate`, `/login`).
- [x] Implement `tools` (`StdToolsExtension`): `read`, `write`, `edit` (with inline diff metadata), `glob`, `grep`, `bash`, `bash_output`, and `bash_kill` (explicit `cwd`, timeout, process-group abort, scoped background task reaping).
- [x] Implement `providers`: `anthropic` (`Llm` + prompt caching + thinking), `openai` (`Llm`), and `jev` (`Decision`) adapters registering into `ProviderRegistry`.
- **Exit Gate:** `StandardBuilder` byte-prefix cache stability tests pass across multi-turn sessions; `ChatLoop` and `DecisionRouter` compose cleanly with `goal_check` and `compaction`; `gestalt-std` builds cleanly under `--no-default-features` and individual feature subsets.

#### Milestone 1.3b — `gestalt-std` Surfaces (`print`, `rpc`, `acp`, `replay`, `tui`) (Target: Week 7)
**Goal:** Ship all 5 user-facing and protocol surfaces behind `surfaces` and `tui` Cargo features.
- [ ] Implement `-p` / `--json` (`print.rs`), `--mode rpc` (`rpc.rs` with full parity methods & JSON-RPC error codes), `--mode acp` (`acp.rs` ACP v1 server), and `replay` (`replay.rs` default + `--reexecute-ext` modes).
- [ ] Implement interactive TUI (`#[cfg(feature = "tui")]`, `ratatui` + `crossterm`) with transcript viewport, keybindings, empty/loading states, scroll-offset-aware mouse hit-testing (`AGENTS.md`), inline diff previews, and `UISurface` dialog/panel rendering.
- **Exit Gate:** ACP v1 & JSON-RPC 2.0 conformance suites pass; TUI mouse click-offset and keyboard navigation unit tests pass.

#### Milestone 1.4 — `gestalt-cli` Composition Root, Self-Extension & Proof Gate (Target: Week 8)
**Goal:** Ship the thin `gestalt` binary and pass all 5 `v0.1` Proof Gate criteria, including live self-extension and the 50-task System One Triage benchmark.
- [ ] Implement `gestalt-cli/src/main.rs`: minimal CLI parser (`--version`, `--help`, `--model`, `--preset`, `--mode`, `replay`, `rollback`, `--ext`, `--trust-workspace`, `--unsafe-no-sandbox`) wiring `gestalt-runtime` + `gestalt-std` features.
- [ ] Implement 50-task System One Triage benchmark suite (`tests/integration/triage_benchmark.rs`) verifying `DecisionRouter` (≥ 85% routing accuracy, ≤ 150 ms median triage latency, ≥ 30% token cost reduction).
- [ ] Verify **Self-Extension Flow**: agent writes an out-of-process extension (custom loop + tool + `UISurface` spec) into `.gestalt/extensions/`, registers it in `.gestalt/gestalt.toml`, receives trust approval, and activates it live via `/reload <ext>` without recompiling core crates.
- **Exit Gate (`v0.1` Proof Gate):**
  1. `gestalt-kernel` ≤ 2,000 LOC, ≤ 2 direct deps, `#![forbid(unsafe_code)]`.
  2. `ChatLoop`, `DecisionRouter`, decorators (`goal_check`, `compaction`), and presets (`swe`, `general`, `minimal_shell`) run unmodified and switch mid-session with 100% history preservation.
  3. External Rust extension AND external out-of-process `--ext` script add custom loops, decorators, tools, and `UISurface` renderers without modifying core crates.
  4. 100% pass rate on fail-closed `PolicyGate` + OS sandbox adversarial suite.
  5. ≥ 10 golden multi-Phase `SessionTree` JSONL fixtures replay offline with 0 network calls and 100% `Effect` parity; ACP v1 conformance tests pass; cold startup ≤ 25 ms, stripped binary ≤ 15 MB, idle RSS ≤ 30 MB.

---

### Phase 2: `v0.2` Ecosystem, Triggers & Container Sandboxes (Target: Weeks 9–12)

#### Milestone 2.1 — Container & Micro-VM Sandboxes (Weeks 9–10)
- [ ] Implement `crates/gestalt-runtime/src/sandbox/docker.rs` and `microvm.rs` (`Firecracker` on Linux / Apple `Virtualization.framework` on macOS) implementing `SandboxDriver`.
- **Exit Gate:** 100% pass rate on the adversarial security & sandbox conformance suite across Docker and Micro-VM drivers.

#### Milestone 2.2 — Event & Schedule `Trigger` Runtime (Week 11)
- [ ] Define `Trigger` trait and registration slot in `gestalt-runtime`; implement local interval (`/loop`), filesystem watcher, webhook listener, and cloud cron (`/schedule`) triggers in `gestalt-std`.
- **Exit Gate:** Triggers enqueue Turns cleanly (queuing as `FollowUp` if a Turn is already active) and pass deterministic replay verification.

#### Milestone 2.3 — MCP Code Mode, Expanded Model Paradigms & `curator` Replay Gating (Week 12)
- [ ] Implement optional sandboxed MCP Code Mode adapter (`search` / `execute` meta-tools) collapsing large MCP server tool schemas.
- [ ] Implement `Embedding`, `WorldModel`, and `ComputerUse` provider adapters, plus automated multi-trace offline replay regression checks inside the `curator` decorator prior to `SaveMode::Promote`.
- **Exit Gate:** MCP Code Mode reduces tool-schema prompt tokens by ≥ 80% on 50+ tool MCP servers; `curator` blocks staged promotions that break any golden replay fixture.

### Phase 3: `v1.0` Production Stability & Multi-Surface Platform (Target: Weeks 13–16)

#### Milestone 3.1 — Schema v1 Freeze, SemVer CI & `gestalt gc` (Week 13)
- [ ] Freeze `gestalt-kernel` 1.0 traits and `SessionTree` v1 JSONL schema (`schema_version: 1`) with `cargo semver-checks` in CI, and implement `gestalt gc` blob/staging cleanup command.
- **Exit Gate:** `cargo semver-checks` and JSONL schema v1 compatibility suite pass in CI.

#### Milestone 3.2 — Reference Desktop/Web GUI Host & Proactive Workflows (Weeks 14–16)
- [ ] Ship reference Desktop/Web GUI host consuming `EventStream` and `UISurface` (`renderers`, `panels`, `dialogs`, `actions`) over WebSocket / ACP.
- [ ] Ship production multi-agent workflow templates combining `Trigger` + `goal_check` + parallel `Effect::Spawn` (`WorkspaceMode::Worktree`) + adversarial evaluator decorators.
- **Exit Gate:** End-to-end GUI conformance suite and multi-agent worktree merge suite pass.

---

## 5. Automated CI Verification Matrix

Every commit to `v2` runs the following CI gates:

| Gate | Command / Check | Failure Condition |
| :--- | :--- | :--- |
| **Formatting & Lints** | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` | Any warning or formatting diff |
| **Kernel LOC Budget** | `tokei crates/gestalt-kernel/src -o json` | Code LOC > 2,000 |
| **Kernel Dep Ceiling** | `cargo tree -p gestalt-kernel --depth 1` | Any direct dep outside `{serde, serde_json}` |
| **Kernel Purity Check** | `rg -i "(anthropic\|openai\|jev\|skill\.md\|memory\.md\|tokio\|reqwest)" crates/gestalt-kernel/src` | Any match in `gestalt-kernel` |
| **Unit & Replay Suite** | `cargo test --workspace` (network-disabled sandbox) | Any test failure or golden replay divergence |
| **OS Sandbox Suite** | Integration tests on `ubuntu-latest` (`bwrap`) & `macos-latest` (`sandbox-exec`) | Any policy bypass or `protected_paths` write |
| **Binary & Startup Budget** | Release build (`--no-default-features` & `--features tui`) size + hyperfine cold start | Stripped binary > 15 MB or cold start > 25 ms |
