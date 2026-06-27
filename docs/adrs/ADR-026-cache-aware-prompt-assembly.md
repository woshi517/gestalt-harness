# ADR-026: Cache-Aware Prompt Assembly

**Status:** Accepted

## Context

LLM providers (Anthropic, OpenAI) charge for prompt-cached tokens. Cache hit rates depend on the *prefix* of the request body matching a previous request exactly — any mutation at or before the breakpoint invalidates the cache and triggers a full recomputation, costing more in both latency and money.

The default (`Dynamic`) assembly strategy interleaves all context messages in insertion order without regard to stability. This means a `ContextContributor` that appends a single ephemeral notice (e.g. a budget-exhausted warning) could be placed before conversation history, breaking the cache line for the entire session history that sits behind it.

We needed a strategy that:

1. Separates *stable* session context (system prompt, tool definitions, MCP server output, workspace description) from *turn-specific* context (user messages, assistant responses, tool results, budget notices).
2. Preserves the stable prefix verbatim between turns so provider caches can recognize it.
3. Allows the dynamic tail to vary freely without affecting the stable prefix.
4. Remains provider-neutral in `gestalt-core` while allowing provider adapters to handle serialization rules (e.g. Anthropic requires all `system` content in a top-level field, not in the `messages` array).

## Decision

### Strategy Pattern

Introduce `PromptAssemblyStrategy` in `gestalt-core`:

```rust
pub enum PromptAssemblyStrategy {
    Dynamic,   // default — interleave all messages in insertion order
    Snapshot,  // cache-aware — stable prefix + dynamic tail
}
```

The strategy is selected via `prompt.assembly_strategy` in `gestalt.json` and defaults to `Snapshot` when building the CLI pipeline.

### Context Stability Classification

Introduce `ContextStability` in `gestalt-core` to tag each context patch with its volatility:

```rust
pub enum ContextStability {
    SessionStatic,     // system prompt, tool defs, workspace description
    ActivationStatic,  // MCP tool schemas, extension-registered tools
    TurnDynamic,       // user/assistant messages, tool results
    Ephemeral,         // budget exhaustion notices, one-shot annotations
}
```

Each `ContextPatch` carries a `ContextStability` tag. The `RuntimeContextPipeline::build_packet` method classifies patches on assembly: stable patches go into the prefix, dynamic/ephemeral patches follow.

### Snapshot Lifecycle

1. **Creation:** On turn 0, `RuntimeContextPipeline` builds the full `ContextPacket`, then computes a `PromptSnapshot` from the stable prefix messages. The snapshot (message list + SHA-256 content hash) is persisted alongside the run manifest.

2. **Reuse:** On session resume/continuation, the CLI loads the previous run's snapshot from disk. It constructs and compares `ProviderCacheKey`s (based on provider ID, API format, model ID, prompt prefix hash, and tool schema hash) between the parent run and the current run. If the keys match, it emits `PromptSnapshotLoaded` and passes the snapshot hash into `AgentRuntime::run_session()`. If they do not match (e.g. during a provider/model switch or tool schema change), the snapshot is discarded and not reused.

3. **Cache invalidation:** If a stable patch's content changes (e.g. a tool schema update), `rebuild_packet()` recomputes the snapshot hash. The provider sees a new prefix hash and issues a cache miss on the next request — this is expected and correct behavior.

### Provider-Neutral Serialization

`gestalt-core` defines `PromptSegment`, `PromptSnapshot`, and `PromptCachePlan` as provider-neutral types. Provider adapters apply their own serialization rules:

- **Anthropic:** `split_anthropic_messages_with_cache()` slices the message array at the `cache_breakpoint`. Tail `Message::System` entries are serialized as `role: "user"` (not `role: "system"`) because the Anthropic Messages API requires all system-level content in the top-level `system` field.

### Runtime Integration

- `RuntimeContextPipeline::build_packet()` → `compose_messages_with_prefix()` classifies patches by `ContextStability`, places stable patches before the dynamic boundary, and calls `rebuild_packet()` to recompute hashes and segments without clearing cache metadata.
- `RuntimeContextHookAdapter` accepts an `initial_prompt_snapshot_hash: Option<String>` field, seeded from resume analysis.
- `AgentEvent` gains four new variants for trace observability: `PromptSnapshotCreated`, `PromptSnapshotLoaded`, `PromptSnapshotReused`, `PromptCachePlanGenerated`.

### Config Surface

```json
{
  "prompt": {
    "assembly_strategy": "snapshot"
  }
}
```

Values: `"dynamic"` (default in core), `"snapshot"` (default in CLI pipeline).

## Consequences

### Positive

- **Provider cache hit rates preserved.** Stable session context is grouped at the front of every request, with a single `cache_control` breakpoint.
- **Provider-neutral core.** `gestalt-core` has no knowledge of Anthropic vs OpenAI caching APIs. All provider-specific logic lives in `gestalt-models`.
- **Observable in traces.** Snapshot creation, loading, and reuse are emitted as trace events, making cache behavior inspectable without depending on provider telemetry.
- **Backward-compatible.** The `Dynamic` strategy remains available and is the default in `gestalt-core`; only the CLI pipeline defaults to `Snapshot`.

### Neutral

- **New types in core.** `PromptAssemblyStrategy`, `ContextStability`, `PromptSegment`, `PromptSnapshot`, `PromptCachePlan`, and `ProviderCacheKey` are all serializable and live in `gestalt-core` because the trace format or cache validation references them. This is consistent with the existing pattern (`ContextPacket`, `TokenBudget`, etc.).
- **Schema desync risk.** The JSON schema (`docs/schemas/gestalt.schema.json`) must be regenerated when config types change. The current schema is missing `assembly_strategy` on `PromptConfig`.

### Negative

- **Snapshot persistence adds I/O.** Each new run writes a `prompt-snapshot.json` to the run artifact directory. Resume must read it back. This is negligible compared to the trace sink and tool execution.
- **Context classification is manual.** Each `ContextContributor` author must choose a `ContextStability` tag. Incorrect tags can silently break cache behavior. Future work could add trace-level warnings for suspicious classification patterns.
- **Anthropic serialization is fragile.** The `role: "user"` fallback for tail `System` messages depends on `split_anthropic_messages_with_cache` being called at the right point. A refactor that changes message ordering must update this rule.
