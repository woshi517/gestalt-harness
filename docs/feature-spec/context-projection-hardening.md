# Feature Specification: Context Projection Architecture Hardening

**Status:** Proposed
**Target:** `gestalt-harness`
**Primary crates:** `gestalt-core`, `gestalt-context`, `gestalt-runtime`, `gestalt-trace`
**Feature class:** Architectural hardening
**Priority:** P0
**Breaking change:** Internal architecture only; public configuration should remain backward-compatible

---

## 1. Summary

Gestalt currently has a capable context-management system consisting of:

* deterministic prompt assembly;
* stable-prefix snapshots;
* tool-result clearing;
* structured context compaction;
* compaction validation;
* projection manifests;
* session checkpoints;
* steering-message injection;
* context composition hooks.

The main architectural weakness is not missing functionality. It is ambiguous ownership between:

1. canonical session history;
2. context-reduction state;
3. provider-visible messages.

Context management may currently replace or rewrite portions of `Session.history` after compaction. In addition, both `MinimalContextPipeline` and `RuntimeContextPipeline` participate in budget enforcement and message selection, creating overlapping context-policy responsibilities.

This feature hardens the architecture around one central invariant:

> Canonical session history is append-only durable truth. Clearing, compaction, truncation, and omission affect only the provider-visible projection.

The implementation should preserve Gestalt’s existing compaction and clearing behavior while changing where their results are stored and how provider context is constructed.

---

## 2. Goals

### 2.1 Primary goals

1. Preserve the complete canonical session history throughout a session.
2. Ensure context clearing and compaction modify only provider-visible projections.
3. Establish one authoritative context-planning pipeline.
4. Make old tool results removable from prompts but recoverable from artifacts.
5. Replace tool-name-based clearing rules with typed tool retention metadata.
6. Enforce stable prompt-prefix behavior and deterministic ordering.
7. Improve context-pressure handling and compaction recovery.
8. Make context decisions explainable through projection manifests.
9. Add deterministic tests for context lifecycle invariants.

### 2.2 Engineering goals

* Keep context policy outside the core agent loop.
* Avoid introducing a new orchestration framework.
* Preserve provider neutrality.
* Preserve existing public behavior unless current behavior depends on destructive history mutation.
* Reuse existing context accounting, compaction, checkpoint validation, and trace infrastructure.
* Keep the normal context-build path inexpensive.
* Make state transitions explicit and testable.

---

## 3. Non-goals

This feature does not introduce:

* vector-based conversation memory;
* semantic retrieval over all session history;
* embedding-backed relevance scoring;
* hierarchical summary trees;
* multiple compactor agents;
* summary voting;
* a protected-fact knowledge graph;
* a general context middleware DSL;
* a new database;
* a full event-sourcing framework;
* provider-specific context managers;
* multi-agent context handoff;
* automatic retrieval of every omitted artifact;
* a task-management subsystem;
* changes to the extension JSON-RPC protocol unless required for retention metadata.

Provider-native compaction may be represented as an optional capability, but implementing every provider-specific compaction API is not required by this feature.

---

## 4. Problem Statement

### 4.1 Canonical history and provider context are conflated

`Session.history` currently acts as:

* the durable record of user, assistant, and tool messages;
* the input to context management;
* the source reconstructed during resume;
* potentially the reduced history after compaction.

If compaction replaces an old history range with a checkpoint message, the original transcript no longer exists as canonical in-memory session state.

This causes problems for:

* resume;
* branch;
* rollback;
* audit;
* future re-compaction;
* summary drift recovery;
* debugging;
* trace equivalence.

### 4.2 Context-policy ownership is split

`MinimalContextPipeline::build()` currently performs:

* system-prefix construction;
* workspace and memory insertion;
* token estimation;
* newest-first history selection;
* budget truncation;
* exhaustion notice insertion.

`RuntimeContextPipeline::prepare_context()` performs:

* usable-budget calculation;
* projected-growth accounting;
* tool-result clearing;
* compaction;
* patch composition;
* projection persistence;
* exhaustion handling.

Both layers can independently decide which content reaches the provider. This creates the possibility of:

* double trimming;
* inconsistent token estimates;
* hidden omissions;
* unclear ownership;
* divergent dynamic and snapshot behavior.

### 4.3 Tool-result clearing is tied to names

Clearing currently recognizes specific tool names such as:

* `read_file`;
* `grep_search`;
* `list_dir`;
* `search_web`;
* `read_url_content`.

This does not scale cleanly to:

* aliases;
* MCP tools;
* extension tools;
* alternative implementations;
* namespaced tools.

### 4.4 Cleared output is not directly recoverable

A tombstone containing an output hash can prove which result was removed, but it cannot restore the result.

The harness should distinguish between:

* removing content from the prompt;
* deleting information from the session.

### 4.5 Context limits are handled too close to overflow

Compaction must itself fit within a model request. Waiting until the request already exceeds the context limit reduces the chance that compaction can succeed.

The runtime needs explicit pressure zones and predictable fallback behavior.

---

## 5. Architectural Principles

### P1 — Canonical history is append-only

Messages accepted into a session must remain available in canonical session state.

Context management must not destructively replace historical messages.

### P2 — Provider context is a projection

The provider receives a bounded, validated projection of canonical history and supporting context.

The projection may include:

* full original messages;
* tombstones;
* compaction checkpoints;
* stable context;
* dynamic context patches;
* exhaustion notices.

The projection is not canonical history.

### P3 — One component owns context policy

`RuntimeContextPipeline` owns:

* context budgeting;
* message selection;
* tool clearing;
* compaction selection;
* checkpoint application;
* omission decisions;
* projection validation.

The lower-level assembler must not independently apply history policy.

### P4 — Reduction must remain reversible where practical

Cleared tool results should be recoverable through artifact references.

Compaction checkpoints should retain source ranges and hashes so their origin remains auditable.

### P5 — Stable prompt content is deterministic

Identical stable inputs must produce byte-identical stable provider prefixes.

### P6 — Context failure must be explicit

The runtime must never silently discard content outside a recorded projection decision.

---

## 6. Target Context Model

Gestalt should use three explicit layers.

```text
┌───────────────────────────────────────────────────────┐
│ Canonical Session History                             │
│                                                       │
│ Complete append-only user, assistant, and tool record │
└───────────────────────────┬───────────────────────────┘
                            │
                            ▼
┌───────────────────────────────────────────────────────┐
│ Context Planning State                                │
│                                                       │
│ Compaction artifacts                                  │
│ Cleared tool-result references                        │
│ Stable prompt snapshot                                │
│ Context activation epoch                              │
└───────────────────────────┬───────────────────────────┘
                            │
                            ▼
┌───────────────────────────────────────────────────────┐
│ Provider-visible Context Projection                   │
│                                                       │
│ Stable prefix                                         │
│ Checkpoint projection                                 │
│ Recent canonical history                              │
│ Tombstones and dynamic context                        │
└───────────────────────────────────────────────────────┘
```

---

## 7. Core Invariants

The implementation must enforce the following invariants.

### I1 — Context management never removes canonical messages

After clearing or compaction:

```rust
session.history == original_history
```

except for messages legitimately appended during the turn.

### I2 — Projection output is chronological

The planner may scan newest-to-oldest when selecting messages, but provider-visible conversation messages must be emitted oldest-to-newest.

### I3 — Tool exchanges remain complete

An assistant tool call and its corresponding tool result must be:

* both represented;
* both excluded;
* or represented through an explicitly valid tombstone arrangement.

The projection must not contain orphan tool results or unresolved tool calls.

### I4 — Every omission is recorded

Each omitted, tombstoned, compacted, or replaced message must appear in the `ProjectionManifest`.

### I5 — Stable-prefix content is deterministic

Stable prefix rendering must not depend on:

* hash-map iteration order;
* nondeterministic extension discovery;
* process timing;
* tool registration race order.

### I6 — Provider request fits the usable limit

A successful `prepare_context()` call must guarantee:

```text
estimated provider input <= usable input limit
```

### I7 — Canonical resume state is independent of the latest projection

Resuming a session must reconstruct canonical history even if the last request used a compacted projection.

### I8 — Context state references stable message identities

Projection and compaction metadata must use stable IDs rather than only vector indices.

---

## 8. Proposed Data Model

## 8.1 Stable message identity

Canonical session history should use core-owned message envelopes.

```rust
pub struct Session {
    pub id: SessionId,
    pub history: Vec<SessionMessage>,
    pub context_state: ContextProjectionState,
    pub token_budget: TokenBudget,
    // existing fields...
}
```

`Message` remains the provider-neutral conversational content type.

`SessionMessage` represents canonical durable session history.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub id: MessageId,
    pub message: Message,
    pub metadata: Option<MessageMetadata>,
}
```

```rust
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
)]
pub struct MessageId {
    pub origin_session_id: SessionId,
    pub sequence: u64,
}
```

`MessageId.sequence` should be monotonically allocated within its origin session.

Existing provider-neutral `Message` types remain unchanged where possible.

### Requirements

* IDs must survive checkpoint serialization.
* IDs must survive resume.
* Branches retain inherited message IDs and allocate new IDs using the branch session ID.
* Vector indices may still be stored as diagnostic metadata but must not be the primary identity.
* Provider adapters continue to consume projected `Message` values rather than `SessionMessage`.

---

## 8.2 Context state

Add durable context-projection control state to the session model.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextProjectionState {
    pub active_checkpoint: Option<CompactionCheckpointRef>,
    pub cleared_tool_results: BTreeMap<ToolUseId, ClearedToolResultRef>,
    pub prompt_snapshot: Option<PromptSnapshotRef>,
    pub context_epoch: ContextEpoch,
    pub policy_fingerprint: Option<String>,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCheckpointRef {
    pub checkpoint_id: String,
    pub artifact: ArtifactRef,
    pub source_start: MessageId,
    pub source_end_exclusive: MessageId,
    pub source_hash: String,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearedToolResultRef {
    pub tool_use_id: String,
    pub tool_id: CanonicalToolId,
    pub message_id: MessageId,
    pub output_hash: String,
    pub artifact: Option<ArtifactRef>,
    pub original_tokens: usize,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSnapshotRef {
    pub snapshot_hash: String,
    pub prefix_hash: String,
    pub artifact: ArtifactRef,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: String,
    pub content_hash: String,
}
```

```rust
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub struct ContextEpoch(pub u64);
```

### Ownership

`ContextProjectionState` represents durable, resumable projection-control state.

It must not contain provider-formatted messages, runtime handles, filesystem paths, or synchronization primitives.

Persist what is required to reproduce the next projection and rebuild ordinary planning calculations at runtime.

---

## 8.3 Context plan

Introduce an internal planning result.

```rust
#[derive(Debug, Clone)]
pub struct ContextPlan {
    pub stable_prefix: Vec<PlannedMessage>,
    pub historical_projection: Vec<PlannedMessage>,
    pub recent_history: Vec<PlannedMessage>,
    pub dynamic_context: Vec<PlannedMessage>,
    pub omissions: Vec<ContextOmission>,
    pub clear_actions: Vec<ClearAction>,
    pub checkpoint: Option<CompactionCheckpointRef>,
    pub pressure: ContextPressure,
}
```

```rust
#[derive(Debug, Clone)]
pub enum PlannedMessage {
    Canonical {
        message_id: MessageId,
    },
    Synthetic {
        message: Message,
        source: SyntheticContextSource,
    },
    Tombstone {
        tool_result: ClearedToolResultRef,
    },
    Checkpoint {
        checkpoint: CompactionCheckpointRef,
    },
}
```

The context plan should remain internal to `gestalt-runtime` or `gestalt-context` unless extension consumers need it.

---

## 8.4 Pressure model

```rust
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub enum ContextPressure {
    Normal,
    Soft,
    Hard,
    Overflow,
}
```

Suggested default thresholds:

| Pressure   |         Projected usage |
| ---------- | ----------------------: |
| `Normal`   | `< 70%` of usable input |
| `Soft`     |                `70–85%` |
| `Hard`     |               `85–100%` |
| `Overflow` |                `> 100%` |

Thresholds should be configurable through `ContextManagementPolicy`.

The initial implementation should not add adaptive or learned thresholds.

---

## 8.5 Tool retention metadata

Extend the canonical tool descriptor.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRetention {
    pub clearable: bool,
    pub reconstructible: bool,
    pub retain_errors: bool,
}
```

Suggested defaults:

```rust
impl Default for ToolRetention {
    fn default() -> Self {
        Self {
            clearable: false,
            reconstructible: false,
            retain_errors: true,
        }
    }
}
```

Example classifications:

| Tool category     | Clearable | Reconstructible | Retain errors |
| ----------------- | --------: | --------------: | ------------: |
| File read         |       Yes |             Yes |           Yes |
| Search            |       Yes |         Usually |           Yes |
| Directory listing |       Yes |             Yes |           Yes |
| Web fetch         |       Yes |       Sometimes |           Yes |
| File write        |        No |              No |           Yes |
| Patch application |        No |              No |           Yes |
| Shell mutation    |        No |              No |           Yes |
| Approval result   |        No |              No |           Yes |

Extension manifests may expose these values using their existing `read_only` and `idempotent` fields, but the host must derive the final trusted retention policy.

Untrusted extensions must not be able to claim that authoritative or mutating results are safely clearable without host validation.

---

## 9. Component Responsibilities

## 9.1 `RuntimeContextPipeline`

`RuntimeContextPipeline` becomes the sole context-policy authority.

It owns:

1. usable-budget calculation;
2. context-pressure classification;
3. stable-prefix resolution;
4. protected-window calculation;
5. tool-result clearing decisions;
6. compaction-range selection;
7. compaction invocation;
8. checkpoint validation;
9. context-plan creation;
10. projection assembly request;
11. final projection validation;
12. projection-manifest persistence;
13. state-delta production;
14. transactional commit coordination.

Proposed primary API:

```rust
pub struct ContextPreparationRequest<'a> {
    pub history: &'a [SessionMessage],
    pub context_state: &'a ContextProjectionState,
    pub token_budget: &'a TokenBudget,
    pub tool_retention: &'a ToolRetentionRegistrySnapshot,
    pub context_policy: &'a ContextManagementPolicy,
    // other request-scoped fields...
}
```

```rust
pub struct PreparedContext {
    pub packet: ContextPacket,
    pub manifest: ProjectionManifest,
    pub state_delta: ContextStateDelta,
}
```

```rust
pub struct ContextStateDelta {
    pub activate_checkpoint: Option<CompactionCheckpointRef>,
    pub cleared_tool_results: Vec<ClearedToolResultRef>,
    pub prompt_snapshot_update: Option<PromptSnapshotRef>,
    pub next_context_epoch: Option<ContextEpoch>,
    pub next_policy_fingerprint: Option<String>,
}
```

The pipeline must not mutate `Session` directly during planning.

It returns a validated `PreparedContext` plus `ContextStateDelta`, and runtime commits that delta only after required artifact persistence, packet validation, and manifest persistence succeed.

The final core contract should be request-based:

```rust
#[async_trait]
pub trait ContextPipeline: Send + Sync {
    async fn prepare_context(
        &self,
        request: ContextPreparationRequest<'_>,
    ) -> Result<PreparedContext, ContextError>;
}
```

---

## 9.2 `MinimalContextPipeline`

Rename or narrow `MinimalContextPipeline` to:

```text
MessageAssembler
```

or:

```text
ContextMessageAssembler
```

It owns only:

* resolving `PlannedMessage` references;
* rendering provider-neutral messages;
* wrapping untrusted content;
* composing stable prefix and dynamic tail;
* preserving chronological order;
* estimating final projected token usage;
* inserting an explicit exhaustion notice when instructed by the plan;
* producing `ContextPacket`.

It must not:

* select history messages;
* decide which messages to drop;
* clear tool outputs;
* invoke compaction;
* calculate protected windows;
* independently enforce a second history budget.

Proposed API:

```rust
pub fn assemble(
    &self,
    session: &Session,
    plan: ContextPlan,
) -> Result<ContextPacket, ContextError>;
```

---

## 9.3 `ContextAccountant`

`ContextAccountant` remains responsible for calculations, not policy.

It should return:

```rust
pub struct ContextEstimate {
    pub stable_prefix_tokens: usize,
    pub dynamic_history_tokens: usize,
    pub tool_schema_tokens: usize,
    pub source_tokens: usize,
    pub patch_tokens: usize,
    pub checkpoint_tokens: usize,
    pub total_estimated_tokens: usize,
    pub usable_limit: usize,
}
```

Actual provider usage must be recorded separately.

---

## 9.4 `ProjectionValidator`

Add or consolidate a final validator that runs before the provider request.

It must verify:

* message chronology;
* initial system-message rules;
* complete tool exchanges;
* valid checkpoint references;
* valid tombstone references;
* trust-boundary rendering;
* stable-prefix ordering;
* total estimated budget;
* every omission represented in the manifest.

Validation errors should be typed and must prevent the provider call.

---

## 10. Context Preparation Flow

## 10.1 Standard path

```text
1. Agent loop drains and appends accepted steering messages at the pre-request safe point
2. Run `before_context_build` hooks
3. Read canonical history and `ContextProjectionState`
4. Resolve stable context snapshot and tool-retention snapshot
5. Calculate projected request size
6. Classify context pressure
7. Build projection plan
8. Persist required checkpoint or tool-result artifacts
9. Assemble provider-visible messages
10. Validate projection and manifest
11. Return `PreparedContext`
12. Commit `ContextStateDelta` atomically to `Session.context_state`
```

---

## 10.2 Pressure behavior

### Normal pressure

```text
Canonical history
    ↓
Full eligible projection
    ↓
Provider request
```

No clearing or compaction occurs.

### Soft pressure

```text
Canonical history
    ↓
Clear old eligible tool results
    ↓
Provider projection
```

Compaction should not run unless clearing fails to satisfy the soft target.

### Hard pressure

```text
Canonical history
    ↓
Clear old eligible tool results
    ↓
Compact eligible historical range
    ↓
Provider projection
```

Compaction should run proactively before actual overflow.

### Overflow

```text
Canonical history
    ↓
Clear
    ↓
Compact
    ↓
Fallback ladder
    ↓
Valid projection or ContextExhaustion
```

---

## 11. Tool-Result Clearing

## 11.1 Eligibility

A tool result may be cleared only when:

* the tool descriptor marks it `clearable`;
* the result is outside the protected recent window;
* the complete tool exchange is resolved;
* the result is not an error when `retain_errors` is enabled;
* the output has a valid hash;
* the result is not already represented by an active checkpoint in a conflicting way;
* any required artifact persistence succeeds.

## 11.2 Artifact persistence

Before replacing a result with a tombstone:

1. calculate or verify the output hash;
2. persist the original content when `reconstructible` or artifact retention is enabled;
3. record the artifact reference;
4. add a `ClearedToolResultRef` to `ContextProjectionState`;
5. represent the result as a tombstone only in the projection.

Canonical history remains unchanged.

## 11.3 Tombstone rendering

Recommended provider-neutral rendering:

```xml
<tombstone
  tool_use_id="call_123"
  tool_name="read_file"
  output_hash="sha256:..."
  artifact_ref="tool-results/call_123.json"
  original_tokens="8420"
/>
```

The `artifact_ref` may be omitted when no recoverable artifact exists, but this should be visible in the projection manifest.

## 11.4 Rehydration

The initial feature requires one of these mechanisms:

* an existing artifact-read tool;
* a new minimal `read_artifact` tool;
* or a standard tool-result retrieval path in the runtime.

Automatic rehydration is not required.

The model may explicitly retrieve the artifact when needed.

---

## 12. Context Compaction

## 12.1 Compaction target

Compaction operates over a contiguous range of canonical messages identified by stable `MessageId` values.

```rust
pub struct CompactionRange {
    pub start: MessageId,
    pub end_exclusive: MessageId,
}
```

The range must:

* end before the protected recent window;
* begin after the source range of the previous active checkpoint where appropriate;
* preserve complete tool exchanges;
* meet the minimum token-benefit threshold;
* fit within the compactor model’s input limit.

## 12.2 Compaction output

Retain the existing structured checkpoint shape:

```rust
pub struct CompactionCheckpoint {
    pub goal: String,
    pub constraints: Vec<String>,
    pub completed_work: Vec<String>,
    pub in_progress_work: Vec<String>,
    pub blocked_items: Vec<String>,
    pub key_decisions: Vec<String>,
    pub next_steps: Vec<String>,
    pub critical_context: Vec<String>,
    pub relevant_references: Vec<String>,
}
```

Existing field types may remain unchanged where already implemented.

## 12.3 Compaction anchors

Before compaction, extract deterministic anchors:

```rust
pub struct CompactionAnchors {
    pub user_constraints: Vec<String>,
    pub file_references: Vec<String>,
    pub identifiers: Vec<String>,
    pub unresolved_questions: Vec<String>,
}
```

Anchor extraction should remain conservative and deterministic.

Initial supported anchor categories:

* explicit user constraints;
* file paths;
* URLs;
* tool-use IDs where relevant;
* code identifiers;
* unresolved questions;
* named deliverables.

Do not introduce semantic embeddings for anchor extraction.

## 12.4 Validation

The existing validator should continue to verify:

* source range;
* source hash;
* non-empty goal;
* non-empty critical context;
* presence of constraints where user constraints existed;
* anchor preservation;
* reference preservation;
* reduced token size.

Add validation that:

* the checkpoint refers to stable message IDs;
* all required `CompactionAnchors` are represented;
* the checkpoint does not claim unfinished work is completed;
* the checkpoint artifact is persisted before it becomes active.

## 12.5 Applying a checkpoint

Applying a checkpoint means updating:

```rust
context_state.latest_checkpoint
```

It must not replace the source messages in `session.history`.

The next provider projection becomes:

```text
stable prefix
active checkpoint
canonical messages after checkpoint range
dynamic context
```

---

## 13. Compaction Fallback Ladder

When the initial compaction attempt fails, the runtime should use a bounded deterministic fallback sequence.

### F1 — Artifactize oversized tool output

Remove or externalize oversized clearable tool results from the compactor input.

### F2 — Reduce the compaction range

Select a smaller valid contiguous historical range.

### F3 — Compact an earlier completed range

Prefer a smaller earlier range that still yields useful token savings.

### F4 — Provider-native compaction

Use provider-native compaction only when:

* the active provider advertises the capability;
* the runtime configuration allows it;
* the result can be stored as a valid provider-specific compaction artifact;
* canonical history remains unchanged.

### F5 — Controlled exhaustion

Return:

```rust
ContextError::Exhausted {
    usable_limit,
    projected_tokens,
    attempted_actions,
    largest_components,
}
```

The error must explain what prevented a valid projection.

The fallback ladder must have a strict retry bound. No open-ended recursive compaction is allowed.

---

## 14. Provider-native Compaction Capability

Represent provider-native compaction as optional provider capability.

```rust
pub enum CompactionBackend {
    HarnessStructured,
    ProviderNative,
}
```

Possible provider interface:

```rust
pub trait ProviderCompaction: Send + Sync {
    async fn compact(
        &self,
        request: ProviderCompactionRequest,
    ) -> Result<ProviderCompactionArtifact, ProviderError>;
}
```

Requirements:

* the core loop must not know which backend is used;
* provider-native artifacts must not replace canonical history;
* provider-specific compaction data must remain outside provider-neutral `Message`;
* the context packet may reference provider-native opaque state through provider request metadata;
* harness-structured compaction remains the default portable backend.

Implementation of provider-native compaction may be deferred after the capability boundary is defined.

---

## 15. Prompt Snapshot and Cache Stability

## 15.1 Stable prefix

The stable prefix may contain:

* base system prompt;
* workspace instructions;
* frozen session memory snapshot;
* session-static context contributors;
* activation-static tool definitions;
* activation-static skills.

It must not contain:

* ordinary conversation history;
* current user messages;
* tool results;
* context-pressure notices;
* one-turn annotations;
* changing timestamps;
* unordered contributor output.

## 15.2 Prompt snapshot

```rust
pub struct PromptSnapshot {
    pub epoch: ContextEpoch,
    pub content_hash: String,
    pub messages: Vec<Message>,
    pub estimated_tokens: usize,
}
```

The snapshot should be built:

* at session initialization;
* when a context activation changes;
* when an explicitly stable configuration changes.

It should not be regenerated on every turn.

## 15.3 Activation epochs

Increment `ContextEpoch` when:

* the active tool catalog changes;
* a skill is activated or deactivated;
* stable workspace instructions change;
* the model requires a different stable prefix;
* a stable extension context contributor changes.

Ordinary user and assistant messages do not increment the epoch.

## 15.4 Cache invalidation event

Add an event such as:

```rust
AgentEvent::ContextCacheInvalidated {
    previous_hash: Option<String>,
    new_hash: String,
    reason: String,
}
```

Or define it as a runtime event if cache planning remains runtime-specific.

Supported reasons should include:

* `session_initialized`;
* `workspace_changed`;
* `memory_snapshot_changed`;
* `tool_catalog_changed`;
* `skill_activation_changed`;
* `model_changed`;
* `prompt_override_changed`;
* `extension_context_changed`.

---

## 16. Token Accounting

## 16.1 Estimated usage

Track internal categories separately:

```rust
pub struct EstimatedContextUsage {
    pub stable_prefix: usize,
    pub dynamic_history: usize,
    pub tool_schemas: usize,
    pub context_patches: usize,
    pub sources: usize,
    pub checkpoint: usize,
    pub protocol_buffer: usize,
    pub total: usize,
}
```

## 16.2 Actual usage

Track provider-reported usage without attempting to divide it into internal categories.

```rust
pub struct ActualProviderUsage {
    pub input_tokens: usize,
    pub cached_input_tokens: Option<usize>,
    pub output_tokens: usize,
    pub reasoning_tokens: Option<usize>,
}
```

## 16.3 Estimation delta

Record:

```text
actual input tokens - estimated input tokens
```

This may later be used to tune provider/model-specific safety buffers.

Do not calculate:

```text
used_history = actual_input - estimated_non_history
```

as an authoritative value.

---

## 17. Projection Manifest

Expand or normalize `ProjectionManifest` so it fully explains the provider-visible request.

```rust
pub struct ProjectionManifest {
    pub projection_id: String,
    pub context_epoch: ContextEpoch,
    pub pressure: ContextPressure,
    pub usable_limit: usize,
    pub estimated_usage: EstimatedContextUsage,
    pub selected_messages: Vec<SelectedMessageRecord>,
    pub omissions: Vec<ContextOmission>,
    pub clear_actions: Vec<ClearAction>,
    pub checkpoint: Option<CompactionCheckpointRef>,
    pub prefix_hash: String,
    pub created_at_turn: usize,
}
```

```rust
pub struct SelectedMessageRecord {
    pub message_id: Option<MessageId>,
    pub projection_role: ProjectionRole,
    pub estimated_tokens: usize,
}
```

```rust
pub enum ProjectionRole {
    StablePrefix,
    Checkpoint,
    CanonicalHistory,
    Tombstone,
    DynamicContext,
    ExhaustionNotice,
}
```

```rust
pub struct ContextOmission {
    pub message_id: MessageId,
    pub reason: OmissionReason,
    pub replacement: Option<String>,
}
```

Possible omission reasons:

* `covered_by_checkpoint`;
* `tool_result_tombstoned`;
* `outside_selected_window`;
* `superseded_context_patch`;
* `unsupported_provider_content`;
* `explicit_policy_exclusion`.

No omission should occur without a reason.

---

## 18. Context Explainability

Add an inspectable context report through an internal API and CLI command.

Suggested command:

```bash
gestalt context explain
```

Optional session targeting:

```bash
gestalt context explain --session <session-id>
gestalt context explain --projection <projection-id>
```

Example output:

```text
Context limit                  128,000
Reserved output                16,000
Safety buffer                   4,000
Usable input                  108,000

Pressure                         Hard
Stable prefix                  12,420
Tool schemas                    8,310
Recent history                 41,700
Compaction checkpoint           4,820
Tool outputs                   17,300
Estimated total                84,550

Canonical messages                132
Projected canonical messages       41
Cleared tool results                 7
Compacted source messages           34
Protected recent turns               4
Prefix cache changed                 no
```

The detailed view should list:

* each clear action;
* each compacted range;
* each omitted message;
* artifact references;
* checkpoint references;
* prefix invalidation reason;
* token estimates by component.

This command should consume `ProjectionManifest`. It should not recalculate decisions independently.

---

## 19. Session Checkpointing and Resume

## 19.1 Checkpoint contents

Session checkpoints must persist:

* complete canonical history or a recoverable reference to it;
* `ContextProjectionState`;
* token budget;
* current context epoch;
* latest projection ID;
* steering queue lifecycle where relevant.

Primary representation:

```rust
pub struct SessionCheckpointV2 {
    pub session_id: SessionId,
    pub history: Vec<SessionMessage>,
    pub token_budget: TokenBudget,
    pub context_state: ContextProjectionState,
    pub latest_projection_id: Option<String>,
    pub checkpoint_sequence: u64,
}
```

Trace persists this structure verbatim. It does not invent message identities or reconstruct projection state on its own.

## 19.2 Resume behavior

Resume must:

1. reconstruct canonical history;
2. restore context state;
3. validate active checkpoint artifacts;
4. validate cleared tool-result artifact references;
5. restore the current context epoch;
6. build a new projection from canonical state.

Resume must not reconstruct canonical history from the last provider projection.

## 19.3 Compatibility

No special migration strategy is required for this feature.

If a future compatibility layer is introduced for older traces, it must not silently claim canonical fidelity when the stored history was already compacted.

## 19.4 Branch behavior

When branching at message `M`:

* copy canonical history through `M`;
* inherited messages retain their original `MessageId` values;
* new appended messages use the branch session ID and branch-local sequence values;
* carry the active checkpoint only when its source range is fully included in the branch and its artifact and source hash remain valid;
* carry cleared tool-result references only when their source messages remain in the branch;
* carry the prompt snapshot only when workspace, memory, activation epoch, and prompt/model configuration remain valid for the branch.

Otherwise clear invalid projection state and rebuild from canonical branch history.

---

## 20. Hooks and Context Contributors

Context contributors should return typed context candidates rather than plain unclassified messages.

```rust
pub struct ContextCandidate {
    pub message: Message,
    pub trust: ContentTrust,
    pub stability: ContextStability,
    pub priority: ContextPriority,
    pub lifetime: ContextLifetime,
    pub provenance: ContextProvenance,
    pub max_tokens: Option<usize>,
    pub required: bool,
}
```

The first implementation may use a reduced version:

```rust
pub struct ContextCandidate {
    pub message: Message,
    pub trust: ContentTrust,
    pub stability: ContextStability,
    pub provenance: ContextProvenance,
}
```

Required behaviors:

* untrusted extension output must not become trusted system instruction;
* contributors must declare stability;
* stable candidates participate in prompt snapshot construction;
* turn-dynamic candidates participate in the current projection only;
* ephemeral candidates must not persist accidentally across turns.

Existing `ContextPatch` behavior may be adapted rather than replaced.

## 20.1 Ownership matrix

| Concern | Core | Runtime | Context crate | Trace |
| --- | --- | --- | --- | --- |
| `Message` content model | Owns | Uses | Uses | Serializes |
| `SessionMessage` and `MessageId` | Owns | Appends and uses | Reads | Serializes |
| `Session.history` | Owns | Mutates append-only | Reads | Persists |
| `ContextProjectionState` | Owns in `Session` | Applies transitions | Reads types | Persists |
| Projection policy | Defines contract | Owns behavior | Implements pure algorithms | Observes |
| Tool-retention snapshot | Defines contract | Builds | Consumes | Fingerprints |
| Tool clearing algorithm | - | Orchestrates | Implements | Records |
| Compaction planning | - | Orchestrates | Implements | Records |
| Compactor model call | - | Owns | - | Records |
| Artifact persistence | - | Owns | - | References |
| Message assembly | Defines contract | Calls | Implements | Records output metadata |
| Projection validation | Defines contract | Calls | Implements | Records failures |
| Provider rendering | Defines contract | Coordinates | - | Records request metadata |
| Checkpoint serialization | Defines payload types | Supplies state | - | Owns persistence |

---

## 21. Events

Add or normalize context lifecycle events.

Recommended events:

```rust
ContextPlanningStarted {
    projection_id: String,
}
```

```rust
ContextPressureEvaluated {
    pressure: ContextPressure,
    estimated_tokens: usize,
    usable_limit: usize,
}
```

```rust
ToolResultsCleared {
    count: usize,
    tokens_removed: usize,
}
```

```rust
CompactionStarted {
    range_start: MessageId,
    range_end: MessageId,
    original_tokens: usize,
}
```

```rust
CompactionCompleted {
    checkpoint_id: String,
    original_tokens: usize,
    checkpoint_tokens: usize,
}
```

```rust
CompactionFailed {
    attempt: usize,
    reason: String,
    recoverable: bool,
}
```

```rust
ContextProjectionBuilt {
    projection_id: String,
    estimated_tokens: usize,
    omission_count: usize,
}
```

Avoid emitting full message contents in high-frequency events.

The projection manifest remains the detailed audit artifact.

---

## 22. Configuration

Extend the existing context-management configuration without introducing a new configuration section unless necessary.

Example:

```json
{
  "context": {
    "management": {
      "soft_pressure_ratio": 0.70,
      "hard_pressure_ratio": 0.85,
      "tool_result_budget_ratio": 0.50,
      "keep_recent_turns": 4,
      "keep_recent_tokens": 20000,
      "min_tokens_to_compact": 8000,
      "compaction_backend": "harness",
      "max_compaction_attempts": 3,
      "persist_clearable_tool_results": true
    }
  }
}
```

Validation requirements:

* `0 < soft_pressure_ratio < hard_pressure_ratio < 1`;
* `0 < tool_result_budget_ratio <= 1`;
* `max_compaction_attempts` must have a small upper bound;
* provider-native backend requires provider capability;
* artifact persistence must be enabled when required by a reconstructible retention policy.

Existing defaults should remain behaviorally close to current defaults.

---

## 23. Error Model

Introduce typed context errors where not already present.

```rust
pub enum ContextError {
    AccountingFailed {
        reason: String,
    },
    InvalidProjection {
        reason: String,
    },
    CompactionFailed {
        attempts: usize,
        reason: String,
    },
    ArtifactPersistenceFailed {
        tool_use_id: String,
        reason: String,
    },
    CheckpointValidationFailed {
        reason: String,
    },
    Exhausted {
        usable_limit: usize,
        projected_tokens: usize,
        attempted_actions: Vec<String>,
        largest_components: Vec<ContextComponentUsage>,
    },
}
```

Errors must be:

* visible in the trace;
* actionable where possible;
* free of silent fallback to destructive mutation.

---

## 24. Security and Trust Requirements

1. Canonical history must preserve trust metadata.
2. Tombstones must not change the trust classification of their source result.
3. Rehydrated artifacts must be rendered using the original trust classification.
4. Extension-provided retention claims must be validated by the host.
5. Artifact paths must be protected against traversal.
6. Checkpoint artifacts must include source hashes.
7. A malformed or missing checkpoint artifact must fail closed for projection construction.
8. Context contributors must not be able to bypass budget accounting.
9. Required trusted context must not be silently omitted.
10. Untrusted content must continue to be wrapped with explicit trust boundaries.

---

## 25. Implementation Phases

## Phase 1 — Canonical identity and core session envelopes

### Deliverables

* add session-scoped `MessageId`;
* add `SessionMessage`;
* change `Session.history` to `Vec<SessionMessage>`;
* centralize append operations on canonical history;
* update hooks, traces, and tests to read canonical envelopes.

### Exit criteria

* canonical history uses stable IDs everywhere;
* canonical appends remain append-only;
* provider-visible projection still renders plain `Message` values.

---

## Phase 2 — Durable projection state and checkpoint persistence

### Deliverables

* add `ContextProjectionState` to `Session`;
* add `SessionCheckpointV2`;
* persist and restore projection state during resume;
* define branch filtering rules for checkpoint, tombstone, and snapshot state;
* remove private pipeline ownership of active checkpoint state.

### Exit criteria

* resume restores canonical history and projection state;
* branch operations cannot retain projection state that references out-of-branch messages;
* trace persists but does not invent canonical identity.

---

## Phase 3 — Transactional context preparation

### Deliverables

* add `ContextPreparationRequest`;
* add `PreparedContext`;
* add `ContextStateDelta`;
* update the core context contract to request-based preparation;
* commit projection state only after successful persistence and validation.

### Exit criteria

* failed preparation leaves `Session.context_state` unchanged;
* preparation returns an auditable manifest and a state delta;
* runtime remains the sole owner of planning and commit behavior.

---

## Phase 4 — Projection-only planning and assembly ownership

### Deliverables

* make `RuntimeContextPipeline` the sole policy owner;
* narrow or rename `MinimalContextPipeline` to a pure assembler;
* introduce `ContextPlan`;
* remove duplicate history-trimming logic;
* stop applying compaction through projected pseudo-history replacement;
* add final projection validation.

### Exit criteria

* only one component decides message inclusion;
* assembler performs no hidden truncation;
* checkpoints apply during projection only.

---

## Phase 5 — Retention registry, pressure zones, and compaction hardening

### Deliverables

* add `ToolRetention`;
* add `ToolRetentionRegistrySnapshot`;
* derive retention snapshots from the composed catalog using canonical tool IDs;
* persist clearable tool outputs via artifact references;
* render artifact-backed tombstones;
* add `ContextPressure`;
* run clearing under soft pressure;
* run proactive compaction under hard pressure;
* implement bounded fallback ladder;
* improve compaction anchors and validation;
* emit compaction lifecycle events.

### Exit criteria

* clearing does not depend on hard-coded tool names;
* missing policies default conservatively to non-clearable behavior;
* cleared output remains recoverable;
* compaction begins before overflow;
* failed compaction follows deterministic bounded recovery;
* exhaustion errors explain why no valid projection could be produced.

---

## Phase 6 — Cache invariants and explainability

### Deliverables

* freeze prompt snapshots;
* add activation epochs;
* enforce deterministic stable ordering;
* emit cache-invalidation reasons;
* track actual provider cache tokens where available;
* add `gestalt context explain`.

### Exit criteria

* unchanged stable inputs produce identical hashes;
* cache invalidation is observable;
* projection decisions can be inspected from persisted artifacts.

---

## 26. Testing Strategy

## 26.1 Unit tests

### Canonical history

* compaction does not mutate canonical history;
* tool-result clearing does not mutate canonical history;
* checkpoint application affects projection only;
* stable message IDs survive serialization.

### Ordering

* newest-first selection produces oldest-first output;
* stable prefix remains before dynamic history;
* dynamic patches appear in deterministic order.

### Tool exchanges

* assistant tool calls remain paired with tool results;
* unresolved exchanges cannot be compacted;
* orphan results fail validation;
* errors remain when retention policy requires them.

### Tool retention

* clearable built-in tools can be tombstoned;
* non-clearable tools remain;
* extension claims cannot bypass host retention rules;
* artifact persistence failure prevents destructive projection replacement.

### Pressure

* normal pressure performs no reduction;
* soft pressure clears eligible results;
* hard pressure clears then compacts;
* overflow executes the bounded fallback ladder.

### Compaction validation

* constraints survive;
* file paths survive exactly;
* identifiers survive;
* unresolved questions survive;
* checkpoint must be smaller than source;
* invalid source hashes are rejected.

### Prompt caching

* stable snapshots are byte-identical across ordinary turns;
* tool-catalog changes increment the epoch;
* dynamic messages do not invalidate the stable snapshot;
* stable contributor order is deterministic.

---

## 26.2 Integration tests

Create golden context lifecycle fixtures.

Recommended fixtures:

1. `full_history_fits`
2. `soft_pressure_clears_tool_results`
3. `hard_pressure_compacts_history`
4. `compaction_preserves_canonical_history`
5. `resume_after_compaction`
6. `branch_before_compaction`
7. `cleared_result_rehydrates_from_artifact`
8. `tool_exchange_never_split`
9. `stable_prefix_cache_remains_constant`
10. `activation_epoch_changes_tool_catalog`
11. `compaction_retry_smaller_range`
12. `context_exhaustion_is_actionable`
13. `steering_message_before_context_build`
14. `untrusted_context_remains_wrapped`
15. `projection_manifest_accounts_for_every_message`

Each fixture should assert:

* canonical history;
* provider-visible messages;
* projection manifest;
* context state;
* emitted events;
* token estimate;
* resume reconstruction.

---

## 26.3 Property tests

Where practical, add property tests for:

* provider-visible chronological ordering;
* no selected message appearing twice;
* every canonical message either selected or accounted for;
* no tool-result orphaning;
* successful projections staying under the usable limit;
* stable snapshot determinism.

---

## 27. Acceptance Criteria

The feature is complete when all of the following are true.

### Architecture

* [ ] `Session.history` is never destructively compacted.
* [ ] `Session.history` uses `SessionMessage` envelopes with stable `MessageId` values.
* [ ] Clearing and compaction are represented in `ContextProjectionState`.
* [ ] `RuntimeContextPipeline` is the only context-policy authority.
* [ ] The lower-level assembler performs no independent history selection.
* [ ] Context preparation is request-based and returns a transactional state delta.

### Context correctness

* [ ] Provider-visible history is chronological.
* [ ] Tool exchanges remain complete.
* [ ] Every omission is represented in `ProjectionManifest`.
* [ ] Successful projections remain under the usable token limit.
* [ ] Trust boundaries survive clearing, compaction, resume, and rehydration.

### Tool results

* [ ] Clearing uses typed retention metadata.
* [ ] Hard-coded clearing allowlists are removed.
* [ ] Cleared tool results can reference recoverable artifacts.
* [ ] Mutating or authoritative results are not cleared by default.

### Compaction

* [ ] Compaction starts under hard pressure before overflow.
* [ ] Compaction ranges use stable message IDs.
* [ ] Checkpoint artifacts preserve required anchors.
* [ ] The fallback ladder is bounded.
* [ ] Compaction failure never silently mutates canonical history.

### Resume and replay

* [ ] Resume reconstructs complete canonical history.
* [ ] Resume restores active context state.
* [ ] Branching from pre-compaction history remains possible.
* [ ] Branch filtering drops projection state that references messages beyond the branch point.
* [ ] Replay distinguishes canonical messages from provider projections.

### Cache behavior

* [ ] Stable prefixes are deterministic.
* [ ] Activation changes increment the context epoch.
* [ ] Cache invalidation reasons are observable.
* [ ] Provider cache usage is recorded when available.

### Testing

* [ ] Golden lifecycle fixtures pass.
* [ ] Projection integrity tests pass.
* [ ] Existing agent-loop and tool-call tests remain green.
* [ ] Branch, resume, and replay lifecycle fixtures remain green.

---

## 28. Compatibility Considerations

### Extensions

Extensions should not require immediate changes.

The host may derive initial retention policy from:

* `read_only`;
* `idempotent`;
* trust level;
* declared risk.

Explicit retention fields may be added to the manifest in a later protocol-compatible extension.

### Configuration

Existing context-management settings should continue to work.

New settings should have defaults matching current behavior as closely as possible.

---

## 29. Architectural Decisions

### AD1 — Canonical history uses stable core-owned message envelopes

Gestalt will not introduce a new session event database as part of this feature.

`gestalt-core::Session.history` becomes `Vec<SessionMessage>`.

`SessionMessage` contains a stable `MessageId`, provider-neutral `Message`, and canonical metadata.

Provider adapters continue to consume projected `Message` values and never receive `SessionMessage` directly.

### AD2 — Durable context-projection state belongs to the core session model

`Session` owns `ContextProjectionState` because active checkpoint references, cleared-result references, prompt-snapshot identity, and context epochs must survive checkpoint, resume, and branch operations.

Core defines only pure serializable value types.

Runtime owns all planning, artifact I/O, and state-transition logic.

### AD3 — Context preparation is request-based and transactional

The final `ContextPipeline` contract accepts `ContextPreparationRequest` containing canonical history, current projection state, token policy, and an immutable tool-retention snapshot.

It returns `PreparedContext` and `ContextStateDelta`.

Runtime commits the delta only after artifact persistence, packet validation, and projection-manifest persistence succeed.

### AD4 — Compaction is a projection artifact

Compaction checkpoints describe historical ranges but do not replace them.

### AD5 — Context planning remains provider-neutral

Provider-specific rendering and native compaction remain behind provider capabilities.

### AD6 — Tool retention is supplied as an immutable catalog snapshot

Runtime derives `ToolRetentionRegistrySnapshot` from the active composed tool catalog and passes it into context preparation.

Context algorithms do not query runtime registries directly.

Missing policies use conservative non-clearable defaults.

### AD7 — Artifact retrieval is explicit

Cleared output is recoverable through an artifact reference, but automatic retrieval is deferred.

### AD8 — Explainability comes from manifests

No second diagnostics engine will independently reconstruct context decisions.

### AD9 — Trace persists session state but is not a source of canonical identity

Message IDs and context-projection state are created and owned by the session model.

Trace checkpoints serialize that state verbatim.

### AD10 — Existing structured checkpoints remain

The checkpoint schema is retained and hardened rather than replaced.

### AD11 — No semantic ranking in the default pipeline

Gestalt continues using recency, protected windows, tool semantics, and structured compaction.

---

## 30. Deferred Work

The following items should be considered only after the new architecture has production evidence.

* content-addressed canonical session event storage;
* incremental checkpoint deltas instead of full-history checkpointing;
* periodic full rebase compaction;
* semantic checkpoint quality scoring;
* relevance-based retrieval from old canonical history;
* cross-session memory retrieval;
* automatic tombstone rehydration;
* provider-native compaction implementations;
* durable task-state objects;
* multi-session context handoff;
* context-quality benchmarks across different models;
* adaptive pressure thresholds based on observed provider usage.

---

## 31. Expected Outcome

After implementation, Gestalt will retain its current context-management capabilities while gaining a substantially clearer architecture.

The resulting lifecycle will be:

```text
Canonical session history
        ↓
Context pressure accounting
        ↓
Projection planning
        ├── stable prefix
        ├── tool-result tombstones
        ├── active compaction checkpoint
        ├── recent canonical history
        └── dynamic context
        ↓
Projection validation
        ↓
Projection manifest
        ↓
Provider rendering
```

The most important result is:

> Context management becomes lossless at the session layer and intentionally lossy only at the provider projection layer.

This improves resume, auditability, branching, cache behavior, compaction reliability, extension compatibility, and long-running-agent coherence without adding a new context framework.
