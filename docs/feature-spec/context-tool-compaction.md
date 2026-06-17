---
title: "feat: Context Compaction and Tool-Result Clearing"
status: proposed
type: feature
depth: deep
owners:
  - gestalt-context
  - gestalt-runtime
  - gestalt-core

---

# feat: Context Compaction and Tool-Result Clearing

## Summary

Add a provider-neutral context-management layer that prevents long-running Gestalt sessions from exceeding a model's context window while preserving session auditability, deterministic replay, prompt-cache stability, and the simplicity of the core agent loop.

The feature introduces two complementary mechanisms:

1. **Tool-result clearing** — removes old, bulky, re-fetchable tool payloads from the model-visible context while preserving the fact that the tool call occurred and enough provenance to retrieve the result again.
2. **Context compaction** — replaces an old, completed segment of the model-visible conversation with a structured, high-fidelity checkpoint summary.

Both mechanisms operate on the **context projection sent to the provider**, not by destructively rewriting the canonical session history.

---

# 1. Architectural Decision

> ## Decision: Canonical history remains append-only; context is a compiled projection
>
> Gestalt MUST preserve the complete `Session.history` as immutable canonical evidence.
>
> Tool-result clearing and compaction MUST operate only while compiling the next model-visible context. They MUST NOT delete, replace, or rewrite canonical session messages.
>
> A compaction is stored as a first-class checkpoint that references a covered history range. The next provider request contains the checkpoint plus uncompacted recent history, while trace and replay retain access to every original message and tool result.

The system therefore maintains three distinct layers:

```text
Canonical session history
    Complete, append-only, replayable
              │
              ▼
Context-management pipeline
    Clear → compact → allocate → render
              │
              ▼
Provider-visible context
    Bounded, optimized, intentionally lossy
```

Persistent memory is a separate fourth concern:

```text
Persistent workspace memory
    Curated, durable, cross-session, user-approved
```

### Why this decision

Destructively rewriting `Session.history` would undermine:

- Exact historical replay.
- Trace auditability.
- Debugging and incident analysis.
- Fine-tuning or trajectory export.
- Tool-call/result lineage.
- Comparison of compaction policies.
- Rebuilding a context projection with a newer policy.
- User trust in Gestalt's files-first, inspectable-state model.

Treating context as a compiled projection preserves the existing append-only session model while allowing the provider request to remain bounded.

### Rejected alternatives

#### Destructively replace old messages with a summary

Rejected because it destroys evidence and makes exact replay impossible.

#### Use a sliding window only

Rejected because it silently loses old requirements, architectural decisions, policy outcomes, and unresolved work without producing a recovery checkpoint.

#### Depend exclusively on provider-native compaction

Rejected because it makes Gestalt behavior provider-specific and may prevent deterministic tracing, local testing, and support for OpenAI-compatible or local providers.

#### Treat persistent memory as the compaction mechanism

Rejected because session-operational state and durable cross-session knowledge have different lifecycles, trust requirements, and approval semantics.

---

# 2. Problem Statement

Long-running agent sessions continuously accumulate:

- User and operator messages.
- Assistant text and reasoning blocks.
- Tool calls.
- Large tool results.
- Context contributions from skills and extensions.
- Policy and approval outcomes.
- Source excerpts and document reads.
- Repeated repair attempts.

Eventually one of two failures occurs:

1. The provider rejects the request because it exceeds the model context limit.
2. The request remains technically valid, but performance and cost degrade because the context contains too much low-signal or stale material.

Gestalt currently values linear, append-only history and deterministic context construction. The solution must preserve those properties rather than replacing them with an opaque mutable conversation state.

---

# 3. Goals

This feature MUST:

- Keep model requests below the effective context limit.
- Intervene before provider rejection.
- Preserve canonical session history unchanged.
- Clear re-fetchable tool output before compacting higher-value dialogue.
- Preserve complete recent turns verbatim.
- Compact only safe, completed history ranges.
- Retain tool-call/result protocol validity.
- Preserve user requirements, corrections, decisions, unresolved work, failures, approvals, and source provenance.
- Maintain prompt-cache stability for the session-static prefix.
- Emit traceable context-management events.
- Support exact replay using stored context projections and checkpoints.
- Remain provider-neutral.
- Keep compaction logic outside the sacred `AgentLoop`.
- Allow provider-native compaction as an optional adapter.
- Fail explicitly when critical context cannot fit.

---

# 4. Non-Goals

This feature does not:

- Replace `.gestalt/memory.md`.
- Automatically promote compacted information into persistent memory.
- Introduce vector retrieval as a requirement.
- Summarize every turn.
- Mutate original trace records.
- Provide semantic truth guarantees for generated summaries.
- Implement multi-agent delegation as a context-management mechanism.
- Require a particular model provider.
- Move policy or approval checks into the context layer.
- Make untrusted tool or document content trusted.
- Guarantee byte-identical regeneration of an LLM-produced summary without storing it.

---

# 5. Terminology

## Canonical history

The complete append-only sequence of session messages and tool results stored by `Session`.

## Context projection

The bounded message sequence compiled for one provider request.

## Tool-result clearing

Replacing an eligible tool-result payload in the context projection with a compact structured tombstone.

## Tool-result compression

Replacing a large tool-result payload with a smaller semantic digest while retaining important content.

## Compaction checkpoint

A structured summary of a contiguous, completed history range, stored with provenance and token statistics.

## Active tail

The recent uncompacted messages retained verbatim after the latest checkpoint.

## Critical context

Content that must survive context management, including trusted system instructions, current user requirements, active approvals, unresolved tool protocol pairs, and configured protected messages.

## Effective input budget

The maximum tokens available to the provider request after reserving output capacity and a safety margin.

---

# 6. Context Budget Model

Gestalt MUST calculate an effective input budget before every provider request:

```text
effective_input_budget =
    model_context_limit
  - reserved_output_tokens
  - provider_safety_margin_tokens
```

The token estimate MUST account for:

- System messages.
- Tool schemas.
- Workspace and memory context.
- Skill instructions.
- Extension context.
- Conversation messages.
- Tool calls and results.
- Provider formatting overhead when known.
- Cache-read tokens as context occupancy, even when they are discounted financially.

The runtime SHOULD maintain both:

```rust
pub struct ContextBudget {
    pub model_context_limit: usize,
    pub reserved_output_tokens: usize,
    pub safety_margin_tokens: usize,
    pub effective_input_limit: usize,
}
```

and:

```rust
pub struct ContextUsage {
    pub stable_prefix_tokens: usize,
    pub dynamic_history_tokens: usize,
    pub tool_result_tokens: usize,
    pub schema_tokens: usize,
    pub ephemeral_tokens: usize,
    pub total_tokens: usize,
}
```

---

# 7. Pressure Zones and Trigger Policy

Gestalt MUST act before the provider rejects a request.

Recommended default zones:

| Zone       |               Context ratio | Action                               |
| ---------- | --------------------------: | ------------------------------------ |
| Healthy    |                    `< 0.70` | No context reduction                 |
| Clearing   |                 `0.70–0.82` | Clear or compress stale tool results |
| Compaction |                 `0.82–0.92` | Compact an old history segment       |
| Emergency  |                    `> 0.92` | Aggressive clearing and compaction   |
| Exhausted  | Cannot fit critical context | Stop with typed error                |

Ratios MUST be configurable.

Gestalt SHOULD also use projected growth:

```text
projected_next_turn_tokens =
    current_request_tokens
  + rolling_average_tool_growth
  + expected_turn_overhead
```

Context management SHOULD run when either current usage or projected usage crosses a configured threshold.

This avoids a session moving from a safe request to an oversized request after one parallel batch of large tool calls.

---

# 8. Context-Management Order

The context manager MUST apply reductions in the following order:

```text
1. Preserve the stable prompt snapshot
2. Remove expired ephemeral context
3. Clear stale re-fetchable tool results
4. Compress oversized summarizable tool results
5. Compact an old completed history segment
6. Rebuild and recount the provider request
7. Apply emergency deterministic trimming
8. Stop explicitly if critical context still cannot fit
```

This order reflects relative information risk:

- Expired ephemeral context is safest to remove.
- Re-fetchable tool payloads are usually safer to remove than dialogue.
- Tool compression loses less conversational state than whole-history compaction.
- Emergency trimming is a last resort.

---

# 9. Tool-Result Retention Model

Each tool result SHOULD carry retention metadata.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRetention {
    pub class: RetentionClass,
    pub source_ref: Option<ContentRef>,
    pub summary: Option<String>,
    pub content_hash: String,
    pub token_estimate: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RetentionClass {
    Required,
    Active,
    Refetchable,
    Summarizable,
    Disposable,
}
```

## Retention semantics

### `Required`

The result must remain verbatim until explicitly superseded or released.

Examples:

- User approval or denial.
- Security-sensitive policy result.
- Non-recoverable external response without stored provenance.
- Data explicitly pinned by a workflow.

### `Active`

Retain while the current operation remains unresolved.

Examples:

- Current test failure.
- Current patch conflict.
- Tool output used by the next repair step.

### `Refetchable`

The full payload may be removed when Gestalt preserves enough information to repeat the call.

Examples:

- File reads.
- Workspace search results.
- Document page extraction.
- Deterministic API reads with stable identifiers.

### `Summarizable`

The payload may be replaced with a semantic digest.

Examples:

- Long test logs.
- Build output.
- Web research results with retained URLs and hashes.
- Multi-file search results.

### `Disposable`

Only the execution record is normally needed.

Examples:

- A successful acknowledgement.
- A no-content write confirmation when the diff is stored separately.
- Temporary progress output.

---

# 10. Tool Metadata Ownership

Retention classification SHOULD be declared by the tool implementation.

```rust
pub trait Tool {
    fn retention_policy(&self) -> ToolRetentionPolicy {
        ToolRetentionPolicy::default()
    }
}
```

A tool may provide:

```rust
pub struct ToolRetentionPolicy {
    pub default_class: RetentionClass,
    pub result_is_refetchable: bool,
    pub produces_artifact: bool,
    pub preserve_errors: bool,
}
```

For extension tools, the extension manifest MAY declare retention hints:

```toml
[[tools]]
name = "workspace_search"
read_only = true
idempotent = true
retention = "refetchable"
```

Trust rule:

> An untrusted extension may request stricter retention, but it MUST NOT weaken host retention requirements.

Unknown tools SHOULD default to `Summarizable` or `Required`, not `Disposable`.

---

# 11. Tool Clearing Eligibility

A tool result is eligible for clearing only when all of the following are true:

- Its tool call and tool result have both completed.
- It is outside the current unresolved tool batch.
- Its retention class permits clearing.
- It is outside the configured recent-turn window.
- It is not pinned by a context contributor or workflow.
- It is not required for an active approval or policy decision.
- It is not explicitly referenced by a protected recent message.
- Clearing it preserves provider-valid tool-call/result structure.

Gestalt SHOULD combine age and token budgets rather than using only “keep the last N calls.”

Recommended defaults:

- Preserve all tool results from the current and previous assistant turn.
- Preserve at least the last three tool calls.
- Limit retained tool-result payloads to 20–30% of the effective input budget.
- Prefer clearing the oldest and largest eligible results.
- Preserve active errors longer than successful results.

---

# 12. Tool-Result Tombstones

Cleared results MUST remain represented in the context projection.

Example:

```text
[tool result cleared]
tool_id: builtin:read
tool_call_id: call_0182
source: crates/gestalt-core/src/agent.rs
content_hash: sha256:9e...
original_tokens: 12481
reason: refetchable_outside_retention_window
refetch: call builtin:read with the original path and range
```

A tombstone MUST preserve:

- Tool identifier.
- Tool call identifier.
- Original input or a recoverable input reference.
- Result content hash.
- Original token estimate.
- Clearing reason.
- Re-fetch instructions when available.
- Artifact references when applicable.

The full original result remains in canonical history and the trace store.

---

# 13. Tool-Result Compression

Before whole-history compaction, Gestalt MAY compress eligible tool output.

```rust
pub struct ToolResultDigest {
    pub tool_call_id: String,
    pub summary: String,
    pub key_facts: Vec<String>,
    pub errors: Vec<String>,
    pub source_refs: Vec<ContentRef>,
    pub artifact_refs: Vec<ArtifactRef>,
    pub original_hash: String,
}
```

Compression MUST preserve:

- Error messages relevant to active work.
- Exact paths, identifiers, line ranges, and artifact references.
- Exit status and structured failure kind.
- Source provenance.
- Any values marked as exact or protected.

Compression SHOULD be deterministic for structured tool outputs when possible. LLM-backed compression is optional and must be recorded as a derived artifact.

---

# 14. Compaction Range Planning

Compaction MUST operate on a contiguous range of old history.

A valid compaction boundary occurs only after:

```text
User message
Assistant turn
All associated tool results
--------------------------------
Safe completed-turn boundary
```

Compaction MUST NOT cross:

- A partial streamed assistant turn.
- A tool call without its result.
- A pending approval.
- An unresolved policy interaction.
- A steering message not yet observed by the model.
- An in-progress extension or MCP call.
- A currently active repair exchange when configured as protected.
- The preserved recent-turn boundary.

The planner SHOULD preserve a configurable number of recent completed turns verbatim.

Recommended default:

```text
preserve_recent_turns = 4
```

The planner SHOULD compact enough material to reduce usage to a target range, rather than barely returning below the trigger.

Recommended target:

```text
post_compaction_target_ratio = 0.50–0.60
```

---

# 15. Compaction Checkpoint Model

Compaction MUST produce a first-class checkpoint.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub id: CompactionId,
    pub session_id: String,
    pub covered_range: HistoryRange,
    pub summary: CompactionSummary,
    pub source_hash: String,
    pub policy_version: String,
    pub compactor: CompactorIdentity,
    pub prompt_hash: String,
    pub created_at_turn: usize,
    pub token_stats: CompactionTokenStats,
}
```

Supporting types:

```rust
pub struct HistoryRange {
    pub start_message_index: usize,
    pub end_message_index_inclusive: usize,
}

pub struct CompactorIdentity {
    pub backend: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

pub struct CompactionTokenStats {
    pub original_tokens: usize,
    pub summary_tokens: usize,
    pub tokens_recovered: usize,
}
```

The checkpoint MUST include a hash of the exact covered canonical history.

The active provider context becomes:

```text
Stable prompt snapshot
+ latest applicable compaction checkpoint
+ uncompacted recent history
+ current dynamic and ephemeral context
```

---

# 16. Structured Compaction Schema

Gestalt MUST NOT use only a free-form “summarize the conversation” instruction.

Recommended schema:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSummary {
    pub task_objective: String,
    pub user_constraints: Vec<ConstraintRecord>,
    pub current_state: String,
    pub completed_work: Vec<CompletedAction>,
    pub decisions: Vec<DecisionRecord>,
    pub unresolved_questions: Vec<String>,
    pub active_plan: Vec<PlanStep>,
    pub artifacts: Vec<ArtifactRef>,
    pub modified_files: Vec<FileChangeRef>,
    pub important_sources: Vec<SourceRef>,
    pub tool_failures: Vec<ToolFailureDigest>,
    pub approvals_and_denials: Vec<PolicyDigest>,
    pub exact_values: Vec<KeyValueFact>,
    pub uncertainties: Vec<String>,
    pub restore_instructions: Vec<String>,
}
```

## Universal preservation rules

The compactor MUST preserve:

- Exact user requirements.
- User corrections and steering messages.
- Current objective.
- Architectural and implementation decisions.
- Open questions.
- Remaining work.
- Policy approvals and denials.
- Relevant failures and failed approaches.
- Artifact identifiers and file paths.
- Source provenance.
- Exact values whose precision matters.
- Explicit uncertainty.
- Trust boundaries.

The compactor MUST NOT:

- Promote untrusted source content into trusted instructions.
- Convert assumptions into facts.
- Treat temporary state as persistent memory.
- Omit known unresolved conflicts.
- Claim that a tool succeeded when the canonical result shows failure.
- Rewrite a user denial into an approval.

---

# 17. Workload-Specific Compaction Profiles

Gestalt SHOULD support compaction profiles.

## Coding profile

Prioritize:

- Modified files.
- Exact paths and line ranges.
- Applied patches.
- Build and test commands.
- Current failures.
- Repository state.
- Architecture decisions.
- Pending implementation steps.
- User restrictions.
- Commands that must not be repeated.

## Knowledge-work profile

Prioritize:

- Claims and evidence.
- Document, page, line, and URL references.
- Numerical values.
- Definitions.
- Competing interpretations.
- Research gaps.
- Draft structure.
- Citation status.
- Uncertainty and provenance.

## Generic profile

Used when no domain profile is active. It preserves the universal schema without domain-specific additions.

---

# 18. Compaction Lifecycle Placement

Context management must run at a safe point before the next provider request.

Canonical turn boundary:

```text
Complete previous assistant turn
→ append all tool results atomically
→ drain accepted steering messages
→ evaluate context pressure
→ clear/compress/compact context projection
→ build ProviderRequest
→ invoke provider
```

### Architectural requirement

> Steering messages MUST be drained before compaction planning.

This ensures an accepted operator correction is included in canonical history and considered by the compactor before the next request.

No post-stop compaction is necessary unless explicitly requested for archival purposes. A terminal run should not create a new model-visible checkpoint that the model never used.

---

# 19. Component Architecture

Introduce an asynchronous context manager outside `gestalt-core`.

```rust
#[async_trait]
pub trait ContextManager: Send + Sync {
    async fn prepare(
        &self,
        input: ContextPreparationInput<'_>,
    ) -> Result<PreparedContext, ContextManagementError>;
}
```

```rust
pub struct ContextPreparationInput<'a> {
    pub session: &'a Session,
    pub model: &'a ModelContextSpec,
    pub tools: &'a ToolCatalogSnapshot,
    pub prompt_snapshot: &'a PromptSnapshot,
    pub policy: &'a ContextManagementPolicy,
}
```

```rust
pub struct PreparedContext {
    pub messages: Vec<Message>,
    pub token_estimate: usize,
    pub usage: ContextUsage,
    pub actions: Vec<ContextAction>,
    pub checkpoint: Option<CompactionCheckpoint>,
    pub projection_hash: String,
}
```

```rust
pub enum ContextAction {
    EphemeralContextExpired {
        item_ids: Vec<String>,
    },
    ToolResultCleared {
        call_id: String,
        original_tokens: usize,
        tombstone_tokens: usize,
    },
    ToolResultCompressed {
        call_id: String,
        original_tokens: usize,
        compressed_tokens: usize,
    },
    HistoryCompacted {
        checkpoint_id: CompactionId,
        covered_range: HistoryRange,
        original_tokens: usize,
        summary_tokens: usize,
    },
    EmergencyTrimmed {
        item_ids: Vec<String>,
    },
}
```

Recommended internal pipeline:

```text
ContextManager
├── ContextCollector
├── TokenAccountant
├── PressureEvaluator
├── ToolResultClearer
├── ToolResultCompressor
├── CompactionPlanner
├── CompactionEngine
├── CriticalContextValidator
├── BudgetAllocator
└── MessageRenderer
```

---

# 20. Crate Ownership

## `gestalt-core`

Owns provider-neutral contracts only:

- Context-management event types.
- Basic checkpoint identifiers and serializable shared types if required.
- Typed stop reason for context exhaustion.
- Trait boundaries needed by `AgentLoop`.

`gestalt-core` MUST NOT:

- Call a compactor model.
- Read or write checkpoint files.
- Know provider-native compaction formats.
- Implement retention heuristics.
- Perform token-heavy context analysis.

## `gestalt-context`

Owns deterministic context logic:

- Token accounting.
- Pressure evaluation.
- Clearing eligibility.
- Compaction range planning.
- Critical-context validation.
- Budget allocation.
- Tombstone rendering.
- Context projection hashing.

## `gestalt-runtime`

Owns I/O and composition:

- Invoking an LLM-backed compaction engine.
- Persisting checkpoints.
- Selecting provider-native or local backends.
- Connecting context actions to runtime events.
- Loading configuration.
- Coordinating extensions and context contributors.

## `gestalt-models`

Owns provider-specific support:

- Provider-native context editing.
- Provider-native compaction request fields.
- Parsing provider compaction blocks.
- Mapping native results into canonical Gestalt checkpoints.

## `gestalt-trace`

Owns:

- Serialized checkpoint artifacts.
- Context projection records.
- Exact replay inputs.
- Token statistics.
- Context-management event envelopes.

---

# 21. Agent Loop Integration

The sacred loop should change minimally.

Conceptual integration:

```rust
loop {
    drain_steering_messages(session)?;

    let prepared = context_manager
        .prepare(ContextPreparationInput {
            session,
            model: &effective_model,
            tools: &tool_snapshot,
            prompt_snapshot: &session.prompt_snapshot,
            policy: &context_policy,
        })
        .await?;

    emit_context_actions(&prepared.actions, emit);

    let request = provider_request_builder.build(prepared.messages);

    let outcome = run_turn(session, request, emit).await?;

    if terminal(outcome) {
        break;
    }
}
```

The loop does not decide:

- What to clear.
- When to compact.
- Which range to compact.
- How to summarize.
- How checkpoints are stored.
- Whether provider-native support is used.

It only requests a prepared context and emits resulting events.

---

# 22. Cache-Aware Prompt Placement

Compaction MUST preserve the stable prompt snapshot.

Placement:

```text
[SessionStatic / ActivationStatic stable prefix]
    system prompt
    workspace instructions
    stable memory snapshot
    stable tool schemas
    activated stable skills

[Dynamic tail]
    compaction checkpoint
    recent conversation
    tool calls and retained results
    steering messages
    ephemeral notices
```

The compaction checkpoint MUST NOT mutate the frozen session-static prefix.

This preserves provider prefix-cache eligibility across turns and across repeated compactions.

A toolset activation change may create a new activation-static snapshot but should not force rewriting unrelated context.

---

# 23. Provider-Native Support

Provider-native context management is an optimization, not the canonical architecture.

Extend provider capabilities:

```rust
pub struct ProviderCapabilities {
    pub native_compaction: bool,
    pub native_tool_result_clearing: bool,
    pub returns_compaction_block: bool,
}
```

Backend selection:

```rust
pub enum ContextManagementBackend {
    Local,
    ProviderNative,
    Auto,
}
```

## Local

Gestalt performs clearing and compaction itself.

This is the reference behavior because it is:

- Provider-neutral.
- Mock-testable.
- Compatible with local providers.
- Compatible with OpenAI-compatible endpoints.
- Traceable.
- Replayable.

## Provider native

Gestalt sends provider-specific context-management configuration.

The adapter MUST map the native result into the canonical:

```rust
CompactionCheckpoint
```

Provider-native behavior MUST still expose:

- Covered range.
- Summary content.
- Token counts before and after.
- Provider and model.
- Configuration or policy identifier.
- Checkpoint hash.
- Trace event.

## Auto

Use provider-native behavior only when the provider capability satisfies Gestalt's trace and replay contract. Otherwise use local behavior.

---

# 24. Persistent Memory Boundary

Compaction and persistent memory MUST remain separate.

| Property  | Compaction                    | Persistent memory                 |
| --------- | ----------------------------- | --------------------------------- |
| Scope     | Current session               | Cross-session                     |
| Trigger   | Context pressure              | Memory proposal workflow          |
| Content   | Broad operational state       | Narrow durable knowledge          |
| Lifetime  | Replaced by later checkpoints | Persistent                        |
| Lossiness | Broad and lossy               | Curated                           |
| Approval  | Usually automatic             | User-approved under current model |
| Placement | Dynamic context tail          | Stable session snapshot           |

A memory proposer MAY inspect checkpoints as evidence.

A checkpoint MUST NOT be written directly into `.gestalt/memory.md`.

---

# 25. Events and Observability

Add first-class events:

```rust
AgentEvent::ContextPressureDetected {
    estimated_tokens: usize,
    effective_limit: usize,
    pressure_ratio: f32,
    projected_next_turn_tokens: Option<usize>,
}
```

```rust
AgentEvent::ToolResultsCleared {
    call_ids: Vec<String>,
    tokens_before: usize,
    tokens_after: usize,
}
```

```rust
AgentEvent::ToolResultsCompressed {
    call_ids: Vec<String>,
    tokens_before: usize,
    tokens_after: usize,
}
```

```rust
AgentEvent::CompactionStarted {
    covered_range: HistoryRange,
    reason: String,
}
```

```rust
AgentEvent::CompactionCompleted {
    checkpoint_id: CompactionId,
    covered_range: HistoryRange,
    tokens_before: usize,
    tokens_after: usize,
    summary_hash: String,
}
```

```rust
AgentEvent::CompactionFailed {
    reason: String,
    recoverable: bool,
}
```

```rust
AgentEvent::ContextPrepared {
    projection_hash: String,
    token_estimate: usize,
    context_generation: u64,
}
```

Runtime events MAY wrap or supplement these with persistence and backend details.

Full summary bodies SHOULD be stored once as trace artifacts rather than duplicated in every event-bus history entry.

---

# 26. Trace and Replay

Gestalt MUST support two replay modes.

## Exact historical replay

Uses the exact stored context projection or checkpoint that was originally sent.

Purpose:

> Reproduce what the model actually saw.

No compaction or clearing policy is rerun.

## Rebuild replay

Rebuilds a context projection from canonical history using recorded configuration:

- Context policy version.
- Tool retention metadata.
- Tokenizer identity.
- Model context limit.
- Prompt snapshot hash.
- Compactor identity.
- Compaction prompt hash.
- Clearing policy.
- Protected-message rules.

Purpose:

> Re-evaluate whether a projection can be reproduced or compare policies.

An LLM-generated checkpoint MUST be reused for exact replay. Calling the compactor again creates a new derivation and is not exact replay.

---

# 27. Error Handling

Add typed errors:

```rust
pub enum ContextManagementError {
    TokenCountUnavailable,
    InvalidToolPairing,
    CompactionProviderFailed,
    InvalidCompactionOutput,
    CheckpointPersistenceFailed,
    CannotPreserveCriticalContext,
    CriticalContextExceedsWindow {
        required_tokens: usize,
        effective_limit: usize,
    },
}
```

Add an explicit stop reason:

```rust
StopReason::ContextExhausted {
    model_limit: usize,
    effective_input_limit: usize,
    required_tokens: usize,
    critical_tokens: usize,
}
```

Recovery order:

```text
1. Clear eligible tool results
2. Compress eligible tool results
3. Run configured compactor
4. Validate checkpoint
5. Rebuild and recount
6. Retry once with stricter compaction target
7. Apply deterministic emergency trimming
8. Stop with ContextExhausted
```

Gestalt MUST NOT silently rely on a provider HTTP error as the normal overflow mechanism.

---

# 28. Compaction Validation

A generated checkpoint MUST pass validation before use.

Validation SHOULD include:

- Schema validity.
- Non-empty objective and current state.
- Covered-range hash match.
- Presence of protected user constraints.
- Presence of unresolved approvals and denials.
- Presence of active artifact references.
- No unknown message indexes.
- No malformed source references.
- Summary size below the configured maximum.
- Critical-context preservation checks.
- Trust-boundary preservation.

Validation failure is recoverable until the configured retry limit is exhausted.

---

# 29. Configuration

Recommended `gestalt.json` shape:

```json
{
  "context": {
    "strategy": "snapshot",
    "reserved_output_tokens": 8192,
    "safety_margin_tokens": 4096,

    "management": {
      "enabled": true,
      "backend": "auto",

      "pressure": {
        "clear_tool_results_at": 0.72,
        "compact_at": 0.84,
        "emergency_at": 0.94,
        "use_projected_growth": true
      },

      "tool_results": {
        "enabled": true,
        "keep_recent_calls": 3,
        "keep_recent_turns": 2,
        "max_budget_ratio": 0.25,
        "preserve_errors_for_turns": 4,
        "replace_with_tombstone": true
      },

      "compaction": {
        "enabled": true,
        "profile": "auto",
        "target_budget_ratio": 0.55,
        "preserve_recent_turns": 4,
        "minimum_compactable_tokens": 16000,
        "max_summary_tokens": 12000,
        "max_checkpoints": 3,
        "mode": "structured"
      }
    }
  }
}
```

## Configuration rules

- Ratios must be between `0.0` and `1.0`.
- `clear_tool_results_at < compact_at < emergency_at`.
- `target_budget_ratio < compact_at`.
- `reserved_output_tokens + safety_margin_tokens < model_context_limit`.
- `preserve_recent_turns >= 1`.
- Invalid configuration must fail during startup validation.
- Model-specific overrides may refine global defaults.

---

# 30. Checkpoint Retention

The canonical trace may retain all checkpoints.

The active model projection SHOULD normally use only the latest applicable checkpoint.

When multiple checkpoints exist:

```text
Checkpoint A covers messages 0–40
Checkpoint B covers checkpoint A + messages 41–80
Active tail begins at message 81
```

Checkpoint B SHOULD summarize the prior checkpoint plus the next compacted range rather than requiring the provider to see all earlier checkpoints.

The trace MUST preserve checkpoint lineage:

```rust
pub struct CheckpointLineage {
    pub parent_checkpoint_id: Option<CompactionId>,
    pub covered_source_hashes: Vec<String>,
}
```

`max_checkpoints` controls active or locally retained convenience copies, not destruction of trace evidence.

---

# 31. Extension and Hook Interaction

Context contributors and extensions may add context with stability metadata.

Rules:

- `SessionStatic` and `ActivationStatic` context are protected from compaction.
- `TurnDynamic` context is eligible according to policy.
- `Ephemeral` context expires before clearing or compaction.
- Extension-added context must retain trust labels through summaries.
- Hooks may annotate retention or pin content, but may not bypass the hard model limit.
- Untrusted extensions may not mark arbitrary content as permanently `Required` without host limits.
- Context hooks that receive history should receive canonical history unless their contract explicitly requests the provider projection.

A future lifecycle hook MAY observe compaction:

```text
before_context_compaction
after_context_compaction
```

These hooks are not required for the first implementation and should be deferred unless a concrete use case appears.

---

# 32. Security and Trust Requirements

Compaction is a transformation boundary and must preserve trust metadata.

The compactor MUST distinguish:

- Trusted system instructions.
- User-authored instructions.
- Assistant-generated content.
- Tool output.
- External untrusted sources.
- Extension-provided content.
- Policy decisions.

A checkpoint MUST NOT flatten these into a single undifferentiated instruction block.

Recommended rendered structure:

```text
<session_checkpoint>
  <trusted_objective>...</trusted_objective>
  <user_constraints>...</user_constraints>
  <agent_state>...</agent_state>
  <untrusted_source_findings>...</untrusted_source_findings>
  <policy_decisions>...</policy_decisions>
</session_checkpoint>
```

The exact wire rendering remains provider-specific, but the canonical checkpoint retains structured trust metadata.

---

# 33. Testing Strategy

## Unit tests

### Token accounting

- Counts stable and dynamic segments separately.
- Includes tool schemas.
- Applies output reservation and safety margin.
- Handles tokenizer unavailability.

### Clearing eligibility

- Clears old re-fetchable results.
- Preserves recent results.
- Preserves required results.
- Preserves unresolved tool pairs.
- Preserves active errors.
- Produces deterministic ordering.

### Tombstone rendering

- Includes call ID.
- Includes content hash.
- Includes refetch metadata.
- Never includes the cleared full payload.

### Compaction planning

- Chooses only contiguous completed ranges.
- Preserves configured recent turns.
- Does not cross approvals.
- Does not cross unresolved tool calls.
- Targets the configured post-compaction ratio.

### Checkpoint validation

- Rejects missing protected constraints.
- Rejects range hash mismatches.
- Rejects malformed source references.
- Preserves trust labels.

## Integration tests

Required fixtures:

```text
large_file_reads_clear_before_compaction
parallel_tool_batch_preserves_pairing
coding_session_compacts_old_repairs
research_session_preserves_citations
steering_message_included_before_compaction
policy_denial_survives_compaction
approval_pending_blocks_compaction_boundary
resume_from_checkpoint
switch_to_smaller_context_model
provider_native_checkpoint_mapping
compactor_failure_falls_back
critical_context_exhaustion_stops_cleanly
exact_replay_uses_stored_projection
rebuild_replay_uses_recorded_policy
```

## Golden traces

Golden traces SHOULD assert:

- Event ordering.
- Covered history indexes.
- Projection hashes.
- Clearing decisions.
- Checkpoint lineage.
- Stop reason.
- Stable prompt snapshot hash remains unchanged.

---

# 34. Evaluation Metrics

Measure:

- Context overflow rate.
- Task completion rate.
- Critical-fact retention.
- User-constraint retention.
- Source and citation retention.
- Policy-decision retention.
- Repeated tool-call rate after clearing.
- Incorrect re-fetch attempts.
- Tokens per completed task.
- Compactions per session.
- Average tokens recovered per clearing event.
- Average tokens recovered per compaction.
- Summary contradiction rate.
- Exact replay coverage.
- Prompt-cache hit rate before and after adoption.

No rollout should be considered successful solely because requests stop overflowing. Quality retention is equally important.

---

# 35. Implementation Phases

## Phase 1 — Token accounting and deterministic tool clearing

Deliver:

- Effective context budget.
- Context pressure events.
- Tool retention metadata.
- Clearing eligibility.
- Tombstone rendering.
- Token-budget retention policy.
- Projection hashing.
- Golden trace coverage.

No compactor model is required.

### Exit criteria

- Large file-read sessions remain under budget through clearing.
- Tool-call/result pairing remains provider-valid.
- Canonical history is byte-for-byte unchanged.
- Exact replay can identify the original full results.

---

## Phase 2 — Local compaction checkpoints

Deliver:

- Compaction range planner.
- Structured compaction schema.
- LLM-backed compaction engine.
- Checkpoint persistence.
- Validation.
- Checkpoint lineage.
- Resume from checkpoint.
- Typed context-exhaustion errors.

### Exit criteria

- Long coding and research sessions continue beyond the original context limit.
- Protected requirements survive regression probes.
- Recent turns remain verbatim.
- Checkpoints are inspectable and replayable.

---

## Phase 3 — Provider-native adapters

Deliver:

- Capability negotiation.
- Native Anthropic or other provider adapters.
- Canonical checkpoint mapping.
- `Local`, `ProviderNative`, and `Auto` modes.
- Provider conformance tests.

### Exit criteria

- Native and local paths emit equivalent canonical trace concepts.
- Native behavior never bypasses replay and audit requirements.
- Unsupported providers transparently fall back to local behavior.

---

## Phase 4 — Policy tuning and evaluation harness

Deliver:

- Coding and knowledge-work profiles.
- Projected-growth trigger.
- Comparative policy benchmarks.
- CLI diagnostics.
- Context-inspection output.

Example CLI:

```bash
gestalt context inspect
gestalt context checkpoints
gestalt context explain --turn 18
gestalt replay --context exact
gestalt replay --context rebuild
```

---

# 36. CLI and User Experience

When context management occurs, the default CLI should show a concise status:

```text
Context pressure: 173,420 / 196,000 tokens
Cleared 6 stale tool results, recovered 71,204 tokens
```

or:

```text
Compacted turns 1–14 into checkpoint cmp_01J...
Context reduced from 181,902 to 104,331 tokens
```

Verbose inspection should show:

- Why the action triggered.
- Which results were cleared.
- What range was compacted.
- Tokens before and after.
- Checkpoint path.
- Compactor model.
- Projection hash.

The UI must not imply that original history was deleted.

---

# 37. Migration and Compatibility

Existing sessions without checkpoints remain valid.

On resume:

1. Load canonical history.
2. Load any recorded prompt snapshot.
3. Detect the effective model context limit.
4. Run the current context policy or use an exact stored projection when replaying.
5. Create a checkpoint only if required.

Serialized message formats SHOULD remain backward compatible.

New retention metadata should use serde defaults.

Example:

```rust
#[serde(default)]
pub retention: Option<ToolRetention>
```

Unknown historical tool results should be treated conservatively.

---

# 38. Open Questions

1. Should the first compactor use the active session model or a separately configured economical model?
2. Should a compactor model require equal or greater context capacity than the active model?
3. Which exact user messages should be automatically marked as protected?
4. Should tool references in assistant text create automatic retention pins?
5. Should local structured compression be attempted before any LLM-backed tool-result compression?
6. How should checkpoints be represented in provider message formats that have strict role alternation?
7. Should exact context projections be stored for every turn or only for turns involving context management?
8. How much provider formatting overhead should adapters reserve when token counting is approximate?
9. Should session branching inherit the latest checkpoint or rebuild from canonical branch history?
10. Should context policies be versioned with semantic versions or content hashes only?

These questions do not block Phase 1.

---

# 39. Acceptance Criteria

The feature is complete when:

- [ ] Canonical `Session.history` is never destructively compacted.
- [ ] Context management runs before provider request construction.
- [ ] Steering messages are drained before compaction planning.
- [ ] Tool clearing precedes whole-history compaction.
- [ ] Clearing preserves tool-call/result validity.
- [ ] Cleared results render structured tombstones.
- [ ] Compaction operates only on completed contiguous history ranges.
- [ ] Recent turns remain verbatim.
- [ ] Compaction produces structured checkpoints with source hashes.
- [ ] Stable prompt snapshots are unchanged by compaction.
- [ ] Compaction and memory remain separate workflows.
- [ ] Every clearing and compaction action emits trace events.
- [ ] Exact replay uses stored projections/checkpoints.
- [ ] Provider-native support maps into the canonical checkpoint model.
- [ ] Context exhaustion produces a typed stop reason.
- [ ] Unit, integration, and golden-trace tests cover the stated invariants.
- [ ] The `AgentLoop` remains small and provider-neutral.

---

# 40. Final Architectural Position

Gestalt should adopt the following invariant:

> **History is immutable evidence. Context is a compiled projection. Compaction is a checkpointed optimization.**

This gives Gestalt a context-management system that is:

- Safe for long-running sessions.
- Compatible with the existing sacred loop.
- Provider-neutral.
- Cache-aware.
- Replayable.
- Inspectable.
- Suitable for coding and knowledge work.
- Extensible without turning the harness into a stateful orchestration framework.