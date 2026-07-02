---
title: "fix: Address model-schema-normalization review findings"
status: completed
created: 2026-06-28
depth: standard
origin: Code review of ref/model-schema-normalization branch
---

# fix: Address Model Schema Normalization Review Findings

## Problem Frame

The `ref/model-schema-normalization` branch received a thorough code review with one **blocker** and four **important non-blockers**. The blocker must be fixed before merge; the non-blockers represent correctness and robustness improvements that should ship alongside.

## Scope Boundaries

**In scope:**
- Blocker: Fix OpenAI Responses `call_id` vs `item.id` tool-call handling
- Non-blocker 1: Document `run --session` vs `continue` semantics
- Non-blocker 2: Cap default `reserved_output` to avoid over-reserving context
- Non-blocker 3: Make `probe_provider` use `models_endpoint` from resolved config
- Non-blocker 4: Remove the `OpenAiProvider` → `OpenAiChatCompletionsProvider` alias
- Non-blocker 5: Add additional config validations for reasoning/thinking capabilities

### Deferred to Follow-Up Work

- Full Responses API streaming fidelity (e.g., `response.output_item.done`, partial streaming for non-tool items)
- `run --session` rerouting through continuation logic (if we later decide that's the desired UX)

---

## Key Technical Decisions

1. **`call_id` is the canonical Gestalt tool-call ID for Responses** — The OpenAI Responses API uses `item.id` (`fc_...`) as the item identifier and `call_id` (`call_...`) for correlating `function_call_output` back to the call. The `call_id` must be the one emitted in `ToolCallStreamed` events and later used in `ToolResult` serialization.

2. **Cap reserved output at 8192 by default** — When no explicit `reserved_output_tokens` is configured, default to `min(model.max_output_tokens, 8192)` rather than the model's full `max_output_tokens`. This prevents a 32k-output model from needlessly eating 32k of input context budget every turn.

3. **Remove `OpenAiProvider` alias entirely** — The alias `OpenAiChatCompletionsProvider as OpenAiProvider` is a legacy indirection. All new code uses `get_by_api_format_with_resolver` which routes correctly. The old `get()/get_with_resolver()` paths should also explicitly reference `OpenAiChatCompletionsProvider` to prevent confusion.

4. **`run --session` stays as label-only for now** — The current implementation is correct: `run --session` labels a new run under an existing session; `continue` preserves context. We document this clearly rather than changing behavior.

---

## Implementation Units

### U1. Fix OpenAI Responses tool-call ID handling (Blocker)

**Goal:** Emit `call_id` (not `item.id`) as the Gestalt tool-call ID from Responses streaming, ensuring `function_call_output` correlation works against the real OpenAI API.

**Requirements:** Responses tool calls must use `call_id` for round-tripping tool results.

**Dependencies:** None

**Files:**
- `crates/gestalt-models/src/openai/responses.rs` (lines 34-37, 433-471)
- `crates/gestalt-cli/tests/model_normalization_acceptance_tests.rs` (lines 6-60)

**Approach:**

1. Expand `ToolCallState` to store `call_id`:
   ```
   struct ToolCallState {
       call_id: String,
       name: String,
   }
   ```

2. In the `response.output_item.added` handler (line 433–449):
   - Read `item.id` as the item ID (used as state map key)
   - Read `item.call_id` as the OpenAI tool-call correlation ID — with fallback to `item.id` when `call_id` is absent (for compatible providers)
   - Read `item.name` as the function name
   - Store `ToolCallState { call_id, name }` keyed by `item.id`

3. In the `response.function_call_arguments.delta` handler (line 452–471):
   - Use `item_id` for state lookup (this is `item.id`, the map key)
   - Emit `AgentEvent::ToolCallStreamed { id: state.call_id, ... }` — the `call_id`, not the item ID

4. Update the acceptance test fixture to use realistic, distinct IDs:
   - `"id": "fc_123"` for the item ID
   - `"call_id": "call_123"` for the tool-call correlation ID
   - Assert the emitted Gestalt tool-call ID is `"call_123"`

**Patterns to follow:** The Chat Completions adapter in `chat_completions.rs` already stores `id` in its `ToolCallState` and emits it correctly. The Responses adapter needs to follow the same semantic — the ID used for `function_call_output.call_id` must match what was emitted in `ToolCallStreamed.id`.

**Test scenarios:**
- Happy path: SSE stream with `output_item.added` containing both `id: "fc_xxx"` and `call_id: "call_xxx"`, followed by `function_call_arguments.delta` referencing `item_id: "fc_xxx"` → emitted `ToolCallStreamed.id` equals `"call_xxx"`
- Multiple tool calls: Two concurrent function calls with different `fc_` and `call_` IDs → each emitted `ToolCallStreamed` carries the correct `call_id` for its respective item
- Fallback: `call_id` field missing from item → graceful fallback to `item.id` to support compatible providers
- Round-trip verification: The `call_id` emitted in streaming matches what `convert_responses_messages` sends as `function_call_output.call_id`

**Verification:** Tests pass. The emitted tool-call ID is `call_123`, not `fc_123`. The `function_call_output` serialization in `convert_responses_messages` already uses `call_id: tool_use_id`, so once the streaming parser emits `call_id`, the round-trip is correct.

---

### U2. Cap default reserved output tokens

**Goal:** Prevent over-reservation of context window by capping the default `reserved_output` to `min(model.max_output_tokens, 8192)`.

**Requirements:** Sensible default token budgeting that doesn't waste input context.

**Dependencies:** None

**Files:**
- `crates/gestalt-runtime/src/runtime.rs` (lines 209-219)
- `crates/gestalt-cli/src/sessions.rs` (lines 526-531)
- `crates/gestalt-cli/src/context.rs` (lines 53-58)
- `crates/gestalt-cli/tests/model_normalization_acceptance_tests.rs` (line 222)

**Approach:**

Apply the cap in the three places where `reserved_output` defaults to `max_output_tokens`:

1. **`crates/gestalt-runtime/src/runtime.rs`** (lines 209-219): Add `.min(8192)` to the model-derived fallback:
   ```
   .map(|m| m.max_output_tokens.min(8192))
   ```

2. **`crates/gestalt-cli/src/sessions.rs`** (lines 526-531): Same pattern in `calculate_continuation_state`:
   ```
   .or(Some(current_model.max_output_tokens.min(8192)))
   ```

3. **`crates/gestalt-cli/src/context.rs`** (lines 53-58): Same pattern in context assembly.

4. Update the acceptance test assertion on line 222 from `assert_eq!(token_budget.reserved_output, 16_384)` to `assert_eq!(token_budget.reserved_output, 8192)`.

**Patterns to follow:** The existing fallback chain structure stays the same — just add `.min(8192)` to the model-derived default.

**Test scenarios:**
- Model with max_output=32,768, no explicit config → reserved_output = 8192
- Model with max_output=4096, no explicit config → reserved_output = 4096 (below cap, unchanged)
- Explicit `reserved_output_tokens = 16384` in config → reserved_output = 16384 (explicit override, cap not applied)
- Continuation state with model switch → reserved_output uses capped value from new model

**Verification:** Token budget tests pass. A model with 32k max output no longer reserves 32k.

---

### U3. Make probe_provider use models_endpoint

**Goal:** `probe_provider` should use `resolved.models_endpoint` when present, instead of hardcoding `/v1/models` or `{base_url}/models`.

**Requirements:** Provider probe should work correctly for OpenRouter, Groq, Together, and any provider that defines a custom `models_endpoint`.

**Dependencies:** None

**Files:**
- `crates/gestalt-cli/src/providers.rs` (lines 40-85)

**Approach:**

In `probe_provider` (line 64–85), restructure the URL construction to check `resolved.models_endpoint` first:

```
let probe_url = if let Some(ref ep) = resolved.models_endpoint {
    ep.clone()
} else if resolved.api_format() == ApiFormat::AnthropicMessages {
    let base = ...;
    format!("{base}/v1/models")
} else {
    let base = ...;
    format!("{base}/models")
};
```

This mirrors the pattern already used in `models.rs` lines 155-171.

**Patterns to follow:** `crates/gestalt-cli/src/models.rs` lines 155-171 already implements this exact pattern for model listing. `probe_provider` should follow the same logic.

**Test scenarios:**
- Provider with explicit `models_endpoint` → probe uses that URL
- Anthropic provider without `models_endpoint` → falls back to `{base}/v1/models`
- OpenAI-compatible without `models_endpoint` → falls back to `{base}/models`

**Verification:** `gestalt doctor` probes the correct endpoint for providers that define `models_endpoint` in the catalog (e.g., OpenRouter, Groq, Together).

---

### U4. Remove legacy OpenAiProvider alias

**Goal:** Eliminate the `OpenAiProvider` alias to `OpenAiChatCompletionsProvider` so there's no ambiguity about which provider implementation is used.

**Requirements:** No code path should silently get Chat Completions when OpenAI Responses is the intended default.

**Dependencies:** None

**Files:**
- `crates/gestalt-models/src/openai.rs` (line 6)
- `crates/gestalt-models/src/lib.rs` (line 19)
- `crates/gestalt-models/src/registry.rs` (lines 9, 40, 52, 158, 172)
- `crates/gestalt-models/tests/auth_tests.rs` (line 6, 49)
- `crates/gestalt-models/tests/provider_stream_tests.rs` (line 4, 13)

**Approach:**

1. **Remove the alias** in `openai.rs` line 6: Delete `pub use chat_completions::OpenAiChatCompletionsProvider as OpenAiProvider;`

2. **Update `lib.rs`** line 19: Remove `OpenAiProvider` from the re-export list.

3. **Update `registry.rs`** line 9: Import `OpenAiChatCompletionsProvider` instead of `OpenAiProvider`.

4. **Update all `OpenAiProvider` references in `registry.rs`** (lines 40, 52, 158, 172) to `OpenAiChatCompletionsProvider`.

5. **Update test files**: Replace `OpenAiProvider` with `OpenAiChatCompletionsProvider` in `auth_tests.rs` and `provider_stream_tests.rs`.

**Patterns to follow:** `get_by_api_format_with_resolver` already explicitly references `OpenAiChatCompletionsProvider` and `OpenAiResponsesProvider` by their full names.

**Test scenarios:**
- All existing tests compile and pass after alias removal
- `registry::get("openai", ...)` still creates a Chat Completions provider (now explicit)
- `registry::get_by_api_format_with_resolver` still routes correctly for all three API formats
- No remaining references to `OpenAiProvider` in the codebase

**Verification:** `cargo build` and `cargo test` pass. `grep -r "OpenAiProvider" crates/` returns no matches.

---

### U5. Expand config validation for reasoning and thinking capabilities

**Goal:** Add validations that catch invalid option/capability combinations early, before they cause confusing provider API errors.

**Requirements:** Better user-facing error messages for invalid configurations.

**Dependencies:** None

**Files:**
- `crates/gestalt-cli/src/config.rs` (lines 2433-2455)

**Approach:**

Expand `validate_model_options` (line 2433):

1. Remove the `_` prefix from `_capabilities` parameter (line 2435) since we'll now use it.

2. **Reject `reasoning_effort` when model capability says reasoning is false:**
   ```
   if !capabilities.reasoning && options.reasoning_effort.is_some() {
       return Err(InvalidValue {
           field: "reasoning_effort",
           reason: "model does not support reasoning; remove reasoning_effort or use a reasoning-capable model"
       })
   }
   ```

3. **Reject `thinking` when model reasoning capability is false on Anthropic:**
   ```
   if api_format == ApiFormat::AnthropicMessages
       && !capabilities.reasoning
       && options.thinking.is_some()
   {
       return Err(InvalidValue {
           field: "thinking",
           reason: "model does not support extended thinking; remove thinking or use a reasoning-capable model"
       })
   }
   ```

**Patterns to follow:** The existing `text_verbosity` and `thinking` format validations at lines 2438-2453 show the exact error pattern.

**Test scenarios:**
- `reasoning_effort: high` on a model with `reasoning: false` → error
- `thinking: { budget_tokens: 1024 }` on Anthropic model with `reasoning: false` → error
- `thinking: { budget_tokens: 1024 }` on Anthropic model with `reasoning: true` → ok
- `reasoning_effort: high` on a model with `reasoning: true` → ok
- Existing validations (text_verbosity on non-Responses, thinking on Responses) still work

**Verification:** Config validation tests pass. Invalid combinations are caught with clear error messages.

---

### U6. Document `run --session` vs `continue` semantics

**Goal:** Make the distinction between `run --session <id>` and `continue <id>` explicit in doc comments and help text.

**Requirements:** Users and future developers understand that `run --session` labels only; `continue` preserves context.

**Dependencies:** None

**Files:**
- `crates/gestalt-cli/src/run.rs` (line 14 — add doc comment)
- `crates/gestalt-cli/src/sessions.rs` (line 537 — add doc comment)

**Approach:**

1. On `run_prompt` (line 14): Add doc comment explaining that `session_id_override` is for labeling/grouping runs under a session ID, not for context preservation. A fresh `Session` is created with only the current prompt.

2. On `run_session_action` (line 537): Add doc comment explaining that this is the context-preserving entry point — it reconstructs history from the prior run's checkpoints, recalculates token budget, and seeds the new session.

3. If CLI help text for `--session` flag exists, ensure it clarifies: "Group this run under an existing session ID (does not restore prior context; use `continue` for that)."

**Test expectation:** None — documentation-only change.

**Verification:** Doc comments are accurate and helpful. No behavioral changes.

---

## System-Wide Impact

- **Token budgeting change (U2)** affects all providers. The cap at 8192 is conservative — models that need more can set `reserved_output_tokens` explicitly. Three callsites need updating (runtime, sessions, context).
- **Alias removal (U4)** is a breaking change for any downstream code importing `OpenAiProvider` from `gestalt-models`. All internal references are updated in this plan; external consumers (if any) need to update imports to `OpenAiChatCompletionsProvider`.
- **Validation expansion (U5)** may reject configs that previously passed silently and failed at the provider API level. This is intentional — fail-fast with a clear message is better.

---

## Sequencing

```
U1 (Blocker: call_id fix)  ──→  merge-ready
U2 (reserved_output cap)  ─┐
U3 (probe models_endpoint) ├──→ ship together or independently
U4 (remove alias)          ─┤
U5 (validation expansion)  ─┤
U6 (documentation)         ─┘
```

U1 is the only merge-blocker and should be completed first. U2–U6 are independent of each other and can be done in parallel or any order.
