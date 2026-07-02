---
title: "feat: Provider and model schema normalization"
type: feature
status: proposed
date: 2026-06-27
origin: docs/feature-spec/model-schema-normalization.md
related:
  - docs/gestalt-harness-architecture.md
  - docs/feature-spec/context-projection-hardening.md
  - docs/adrs/ADR-025-unified-gestalt-json-config.md
  - docs/adrs/ADR-026-cache-aware-prompt-assembly.md

---

# Provider and Model Schema Normalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make provider wire format, credential source, and model limits explicit; add a dedicated OpenAI Responses adapter; and allow every run in an existing session to resolve and trace its own provider/model without changing canonical history.

**Architecture:** Keep the current crate boundaries and session-lineage implementation. `gestalt-cli` parses, merges, validates, and resolves provider/model configuration; `gestalt-core` carries small provider-neutral selection and resolved-model snapshot types; `gestalt-runtime` derives each run's token budget and projects canonical history; `gestalt-models` selects a thin wire adapter by `ApiFormat`; `gestalt-trace` persists the resolved snapshot with the existing run manifest. A continued run reconstructs canonical history as it does today, but it never reuses the parent run's model limit.

**Tech Stack:** Rust 1.75 workspace, serde/schemars, reqwest, eventsource-stream, Tokio, existing `Provider`, `ContextPipeline`, `RunManifest`, and CLI config systems.

---

## 1. Current Architecture Anchor

This plan is anchored to repository state:

```text
commit: 9d54aa0 (ref/extension protocol)
date:   2026-06-27
```

The input specification is currently an untracked user-owned file:

```text
docs/feature-spec/model-schema-normalization.md
```

Do not modify or stage that file while implementing this plan unless the user explicitly asks.

### 1.1 What already works

- `crates/gestalt-cli/src/config.rs` resolves CLI overrides, profiles, defaults, provider config, model options, and variants into `ResolvedProvider`.
- `crates/gestalt-cli/src/runtime.rs` creates one concrete provider and one `RuntimeConfig` for a run.
- `crates/gestalt-cli/src/sessions.rs` reconstructs canonical `SessionMessage` history from a parent run and builds a new runtime from the current effective config. This already permits provider/model changes between continued runs.
- `crates/gestalt-core/src/session.rs` stores provider-neutral canonical messages separately from `ContextProjectionState`.
- `crates/gestalt-runtime/src/context.rs` builds provider-visible projections without rewriting canonical history.
- `crates/gestalt-models/src/anthropic.rs` renders explicit Anthropic cache control from a provider-neutral `PromptCachePlan`.
- `crates/gestalt-models/src/openai.rs` intentionally ignores Anthropic cache metadata, preserving automatic OpenAI-side caching.
- `crates/gestalt-cli/src/connect.rs` and TUI onboarding already store pasted keys in the OS keychain and persist only a credential reference.
- `crates/gestalt-cli/src/auth.rs` already resolves ephemeral CLI keys, environment variables, keychain entries, and interactive prompts through a resolver chain.
- `crates/gestalt-trace/src/run_manifest.rs` already models session/run lineage.

### 1.2 Concrete gaps in the current implementation

1. `ProviderKind` controls both provider identity and wire serialization.
2. `OpenAiProvider` always posts to `{base_url}/chat/completions`, although its request body currently includes Responses-only fields such as `reasoning` and `text`.
3. Provider-scoped `ModelDefinitionConfig` has no context/output limits or typed capabilities.
4. `WorkspaceConfig::merge` replaces complete provider and profile entries through `HashMap::extend`.
5. `RuntimeConfig` falls back to a 120,000-token model limit.
6. `run_session_action` reconstructs the parent checkpoint's `TokenBudget` and passes it unchanged into a run that may use another model.
7. `RunManifest` records lineage and compatibility, but not the resolved provider/model snapshot.
8. Model built-ins are split between `gestalt-models::ModelCatalog` and an additional hard-coded list in `gestalt-cli/src/models.rs`.
9. Provider option compatibility is not validated before an HTTP request.
10. Manual inline API keys are rejected, and `api_key_env`/`auth_ref` expose storage mechanics instead of accepting the desired string shorthand.

### 1.3 Existing boundaries to preserve

```text
gestalt-cli
  config parsing + merge + validation + selection
            |
            v
gestalt-runtime
  run orchestration + token budget + context projection
            |
            v
gestalt-core
  canonical messages + provider-neutral contracts/events
            |
            v
gestalt-models
  endpoint + request serialization + SSE normalization
```

Do not:

- add provider wire payload structs to `gestalt-runtime`;
- add session persistence to provider adapters;
- create a second run store beside `RunManifest`;
- move config file I/O into `gestalt-core`;
- let a provider adapter compact or mutate canonical history.

## 2. Scope Decisions and Gap Resolution

### Gap Report: `model-schema-normalization.md`

**Overall status:** ⚠️ Minor gaps resolved by this plan

#### Resolved blocking ambiguities

**[GAP-001] `kind` compatibility is undecided**

- **Type:** Migration ambiguity
- **Where:** AD-001 and §16
- **Problem:** The spec permits either a clean break or a temporary mapping.
- **Decision:** Make `api_format` authoritative and reject `kind` in version-1 config because all config structs use `deny_unknown_fields` and the project is greenfield. Update checked-in fixtures, generated schema, examples, and migration documentation in the same change. Keep the Rust name `ProviderKind` removed rather than aliased.

**[GAP-002] Model-limit fallback order references an absent provider field**

- **Type:** Field mismatch
- **Where:** AD-007 versus §8.2
- **Problem:** AD-007 mentions a provider-default context limit, but `ProviderConfig` does not define one.
- **Decision:** Use this exact order: CLI run override → profile override → workspace context override → configured provider model → built-in/cached catalog → conservative 32,000-token fallback. Do not add an undocumented provider-wide model limit.

**[GAP-003] “Fresh projection” conflicts with persisted projection-reduction state**

- **Type:** Unspecified behavior
- **Where:** AD-005 and §17.4
- **Problem:** Reusing `ContextProjectionState` after switching models can preserve an old compaction checkpoint and prevent a larger model from seeing more canonical history.
- **Decision:** Reuse reduction state only when the resolved provider/model/api-format snapshot is unchanged. On a selection change, preserve canonical history but start with a default `ContextProjectionState` and rebuild prompt/cache planning.

**[GAP-004] Run trace storage location is unspecified**

- **Type:** Missing contract
- **Where:** §15
- **Problem:** The spec proposes a `RunStarted` event but the existing durable run index is `RunManifest`.
- **Decision:** Store `ResolvedModelSnapshot` in `RunManifest` and emit it through a new `AgentEvent::RunStarted`. Older manifests deserialize with `None`; all newly created manifests write `Some`.

#### Non-blocking scope decisions

**[GAP-005] Persisted session defaults are optional and have no storage contract**

- **Type:** Scope creep risk
- **Where:** §14.3
- **Decision:** Do not add `session set-default-model` in this implementation. Global/workspace/profile defaults and per-command overrides already define run selection. A persisted session-default feature needs a separate specification for mutation, branching, and precedence.

**[GAP-006] Existing replay is offline trace replay, not provider re-execution**

- **Type:** Assumption without basis
- **Where:** §15.3
- **Decision:** Persist the snapshot needed by future live replay and keep existing offline replay deterministic. Do not add provider re-execution or remapped live replay in this change.

**[GAP-007] Optional request endpoint paths lack a field name**

- **Type:** Field mismatch
- **Where:** §4.1 and §8.2
- **Decision:** Add `request_path: Option<String>`. If absent, adapters use `/v1/messages`, `/chat/completions`, or `/responses` according to `api_format`. Validation requires a relative absolute-path string beginning with `/`.

**[GAP-008] Credential syntax and onboarding persistence are underspecified**

- **Type:** Missing behavior
- **Where:** §4.1, §7, §8.2, and existing onboarding behavior
- **Problem:** The feature spec lists API-key source fields but does not define inline literals, `$ENV_VAR` shorthand, precedence, redaction, or whether onboarding may write plaintext.
- **Decision:** Keep the schema string-based and support exactly one configured auth form per provider layer: `auth_ref: "keychain:gestalt/<provider>"`, `api_key_env: "PROVIDER_API_KEY"`, `api_key: "$PROVIDER_API_KEY"`, or `api_key: "literal-key"`. CLI/TUI onboarding always writes `auth_ref` for pasted keys and never writes `api_key`. Inline literals are manual power-user behavior and produce a warning that never includes the secret.

#### Confirmed good

- Canonical `SessionMessage` history is provider-neutral and append-only.
- Provider adapters already normalize streams into `AgentEvent`.
- Context projection and prompt cache planning are already outside provider adapters.
- CLI provider/model overrides are global flags and already apply to `sessions continue`, `resume`, and `branch`.
- Tool schemas are deterministically ordered before provider adaptation.
- CLI/TUI onboarding already has the correct secure default: paste once, store in the OS keychain, persist a reference.

## 3. Target File Structure

### Create

```text
crates/gestalt-models/src/openai/mod.rs
crates/gestalt-models/src/openai/common.rs
crates/gestalt-models/src/openai/chat_completions.rs
crates/gestalt-models/src/openai/responses.rs
tests/fixtures/provider-streams/openai-responses-text.sse
tests/fixtures/provider-streams/openai-responses-tool.sse
docs/migrations/provider-kind-to-api-format.md
```

### Modify

```text
crates/gestalt-core/src/model.rs
crates/gestalt-core/src/provider.rs
crates/gestalt-core/src/session.rs
crates/gestalt-core/src/event.rs
crates/gestalt-core/src/lib.rs
crates/gestalt-models/src/lib.rs
crates/gestalt-models/src/registry.rs
crates/gestalt-models/src/catalog.rs
crates/gestalt-models/src/auth.rs
crates/gestalt-models/tests/auth_tests.rs
crates/gestalt-models/tests/no_secret_tests.rs
crates/gestalt-models/tests/provider_stream_tests.rs
crates/gestalt-cli/src/config.rs
crates/gestalt-cli/src/auth.rs
crates/gestalt-cli/src/connect.rs
crates/gestalt-cli/src/provider_catalog.rs
crates/gestalt-cli/src/providers.rs
crates/gestalt-cli/src/models.rs
crates/gestalt-cli/src/runtime.rs
crates/gestalt-cli/src/run.rs
crates/gestalt-cli/src/sessions.rs
crates/gestalt-cli/src/main.rs
crates/gestalt-cli/src/output.rs
crates/gestalt-runtime/src/config.rs
crates/gestalt-runtime/src/runtime.rs
crates/gestalt-trace/src/run_manifest.rs
crates/gestalt-trace/src/resume.rs
crates/gestalt-cli/tests/config_tests.rs
crates/gestalt-cli/tests/config_schema_tests.rs
crates/gestalt-cli/tests/connect_cli_tests.rs
crates/gestalt-cli/tests/provider_model_cli_tests.rs
crates/gestalt-cli/tests/sessions_cli_tests.rs
crates/gestalt-cli/tests/run_smoke_tests.rs
crates/gestalt-runtime/tests/runtime_run_tests.rs
crates/gestalt-trace/tests/golden_trace_tests.rs
crates/gestalt-cli/src/tui/app.rs
crates/gestalt-cli/src/tui/update.rs
crates/gestalt-cli/tests/tui_smoke_tests.rs
tests/fixtures/config/v1/full_valid.json
tests/fixtures/workspaces/minimal/gestalt.json
tests/fixtures/workspaces/profiled/gestalt.json
docs/schemas/gestalt.schema.json
README.md
```

The OpenAI module split is targeted: it separates two wire protocols while preserving `gestalt_models::OpenAiProvider` as a compatibility re-export for the Chat Completions adapter.

## 4. Implementation Tasks

### Task 1: Add provider-format and resolved-model contracts

**Files:**

- Modify: `crates/gestalt-core/src/provider.rs`
- Modify: `crates/gestalt-core/src/model.rs`
- Modify: `crates/gestalt-core/src/session.rs`
- Modify: `crates/gestalt-core/src/event.rs`
- Modify: `crates/gestalt-core/src/lib.rs`
- Test: `crates/gestalt-core/tests/core.rs`

- [ ] **Step 1: Write serde round-trip tests for the shared contracts**

Add tests covering these exact serialized values:

```rust
#[test]
fn api_format_uses_snake_case_wire_names() {
    assert_eq!(
        serde_json::to_value(ApiFormat::OpenAiResponses).unwrap(),
        serde_json::json!("openai_responses")
    );
}

#[test]
fn resolved_model_snapshot_round_trips() {
    let snapshot = ResolvedModelSnapshot {
        selection: ModelSelection {
            provider_id: "openai".into(),
            model_id: "gpt-5.1".into(),
            variant: Some("high".into()),
        },
        api_format: ApiFormat::OpenAiResponses,
        display_name: Some("GPT-5.1".into()),
        max_context_tokens: 400_000,
        max_output_tokens: 32_768,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            reasoning: true,
            prompt_cache: PromptCacheMode::Automatic,
        },
    };

    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(
        serde_json::from_value::<ResolvedModelSnapshot>(value).unwrap(),
        snapshot
    );
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test -p gestalt-core --test core api_format_uses_snake_case_wire_names
cargo test -p gestalt-core --test core resolved_model_snapshot_round_trips
```

Expected: compilation fails because the new types do not exist.

- [ ] **Step 3: Add the shared types**

Define in `gestalt-core/src/provider.rs`:

```rust
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    None,
    Automatic,
    Explicit,
    #[default]
    ProviderDependent,
}
```

Define in `gestalt-core/src/model.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
    pub vision: bool,
    pub json_mode: bool,
    pub reasoning: bool,
    pub prompt_cache: PromptCacheMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedModelSnapshot {
    pub selection: ModelSelection,
    pub api_format: ApiFormat,
    pub display_name: Option<String>,
    pub max_context_tokens: usize,
    pub max_output_tokens: usize,
    pub capabilities: ModelCapabilities,
}
```

Add `resolved_model: Option<ResolvedModelSnapshot>` to `SessionConfig`, with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Update all existing `SessionConfig` literals with `resolved_model: None`.

Add:

```rust
AgentEvent::RunStarted {
    resolved_model: ResolvedModelSnapshot,
}
```

Re-export all new types from `gestalt-core/src/lib.rs`.

- [ ] **Step 4: Run focused and crate tests**

Run:

```bash
cargo test -p gestalt-core --test core
cargo test -p gestalt-core
```

Expected: all `gestalt-core` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/gestalt-core
git commit -m "feat(core): add resolved model run contracts"
```

### Task 2: Normalize config types and recursive merge behavior

**Files:**

- Modify: `crates/gestalt-cli/src/config.rs`
- Test: `crates/gestalt-cli/tests/config_tests.rs`
- Test: `tests/fixtures/config/v1/full_valid.json`

- [ ] **Step 1: Add failing schema parsing tests**

Add focused tests that parse:

```json
{
  "version": 1,
  "providers": {
    "openai": {
      "api_format": "openai_responses",
      "base_url": "https://api.openai.com/v1",
      "api_key": "$OPENAI_API_KEY",
      "models": {
        "gpt-5.1": {
          "max_context_tokens": 400000,
          "max_output_tokens": 32768,
          "capabilities": {
            "streaming": true,
            "tools": true,
            "reasoning": true,
            "prompt_cache": "automatic"
          }
        }
      }
    }
  }
}
```

Also add a rejection test for:

```json
{
  "version": 1,
  "providers": {
    "legacy": {
      "kind": "openai"
    }
  }
}
```

The rejection must assert that the error names unknown field `kind`.

Add parsing tests proving all four supported auth strings retain a JSON string schema:

```json
{"auth_ref": "keychain:gestalt/openai"}
{"api_key_env": "OPENAI_API_KEY"}
{"api_key": "$OPENAI_API_KEY"}
{"api_key": "sk-manually-configured"}
```

- [ ] **Step 2: Add failing recursive merge tests**

Construct global and workspace `WorkspaceConfig` values where the workspace adds only:

```json
{
  "providers": {
    "openai": {
      "models": {
        "gpt-5.1": {
          "variants": {
            "cheap": {
              "options": {
                "reasoning_effort": "low"
              }
            }
          }
        }
      }
    }
  }
}
```

Assert the merge retains global `api_format`, `base_url`, `api_key_env`, model limits, and adds variant `cheap`. Add an equivalent profile test proving that overriding only `model` retains the profile's `provider`.

- [ ] **Step 3: Run the tests and verify they fail**

Run:

```bash
cargo test -p gestalt-harness --test config_tests provider_config_parses_api_format_and_model_metadata
cargo test -p gestalt-harness --test config_tests provider_config_rejects_legacy_kind
cargo test -p gestalt-harness --test config_tests workspace_provider_models_merge_recursively
cargo test -p gestalt-harness --test config_tests profiles_merge_field_by_field
```

Expected: parsing and merge assertions fail against the current types and `HashMap::extend`.

- [ ] **Step 4: Replace `ProviderKind` with typed config fields**

In `config.rs`:

- remove `ProviderKind`;
- use `gestalt_core::ApiFormat`;
- add typed `PromptCacheMode`;
- add `ModelCapabilitiesConfig` with optional fields;
- add `max_context_tokens`, `max_output_tokens`, and `capabilities` to `ModelDefinitionConfig`;
- add `api_format`, `request_path`, and typed provider capabilities to `ProviderConfig`;
- add `api_key: Option<SecretString>` to `ProviderConfig`, where `SecretString` is serde-transparent and emits `[REDACTED]` from `Debug`;
- retain `api_key_env` and `auth_ref` as supported explicit forms;
- add `context_window_override` to `ProfileConfig`;
- add `context_window_override` to `CliOverrides`.

Keep fields optional in mergeable config structs. Required-field checks happen after layered config has been merged.

- [ ] **Step 5: Implement field-wise recursive merge helpers**

Add private helpers:

```rust
fn merge_provider_config(base: ProviderConfig, overlay: ProviderConfig) -> ProviderConfig;
fn merge_model_definition(
    base: ModelDefinitionConfig,
    overlay: ModelDefinitionConfig,
) -> ModelDefinitionConfig;
fn merge_model_variant(
    base: ModelVariantConfig,
    overlay: ModelVariantConfig,
) -> ModelVariantConfig;
fn merge_profile_config(base: ProfileConfig, overlay: ProfileConfig) -> ProfileConfig;
fn merge_model_capabilities(
    base: ModelCapabilitiesConfig,
    overlay: ModelCapabilitiesConfig,
) -> ModelCapabilitiesConfig;
fn merge_provider_capabilities(
    base: ProviderCapabilitiesConfig,
    overlay: ProviderCapabilitiesConfig,
) -> ProviderCapabilitiesConfig;
```

Implement these semantics:

- scalar `Some` values replace lower-precedence values;
- `headers` and `adapter_options` merge by key;
- `models` merge by model ID;
- `variants` merge by variant ID;
- option structs merge field by field;
- arrays retain the existing replace behavior.

Treat `api_key`, `api_key_env`, and `auth_ref` as one mutually exclusive auth group during layered merge. If an overlay sets any member, clear the inherited members before applying it. Reject a single config layer that specifies more than one member, naming fields but never values in the error.

Replace `self.providers.extend(other.providers)` and `self.profiles.extend(other.profiles)` with keyed calls to these helpers.

When computing `EffectiveConfig` fingerprints, replace an inline literal with the stable marker `[INLINE_API_KEY]` before serialization. The fingerprint may include environment-variable or keychain account names, but it must never hash or serialize literal credential bytes.

- [ ] **Step 6: Remove optimistic context defaults**

Change config defaults so absent metadata does not silently become 120,000:

```rust
context_window_override: None,
max_context_window: None,
reserved_output_tokens: None,
safety_margin_tokens: Some(2048),
```

Keep `max_context_window` as a deserialization-only legacy alias for `context_window_override` during this release, but do not emit it in generated config.

- [ ] **Step 7: Run the config tests**

Run:

```bash
cargo test -p gestalt-harness --test config_tests
```

Expected: all config parsing, precedence, and recursive merge tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/gestalt-cli/src/config.rs crates/gestalt-cli/tests/config_tests.rs tests/fixtures/config/v1/full_valid.json
git commit -m "feat(config): normalize provider and model schema"
```

### Task 2A: Resolve string-based provider credentials safely

**Files:**

- Modify: `crates/gestalt-models/src/auth.rs`
- Modify: `crates/gestalt-models/src/lib.rs`
- Modify: `crates/gestalt-cli/src/auth.rs`
- Modify: `crates/gestalt-cli/src/connect.rs`
- Modify: `crates/gestalt-cli/src/output.rs`
- Modify: `crates/gestalt-cli/src/tui/app.rs`
- Modify: `crates/gestalt-cli/src/tui/update.rs`
- Test: `crates/gestalt-models/tests/auth_tests.rs`
- Test: `crates/gestalt-models/tests/no_secret_tests.rs`
- Test: `crates/gestalt-cli/tests/connect_cli_tests.rs`
- Test: `crates/gestalt-cli/tests/tui_smoke_tests.rs`

- [ ] **Step 1: Replace the inline-key rejection test with safe acceptance tests**

Test these mappings:

```rust
assert_eq!(
    provider_auth_config(
        &json!({"api_key": "$OPENAI_API_KEY"}),
        "openai",
        "OPENAI_API_KEY",
    )?
    .credential,
    ConfiguredCredential::Environment("OPENAI_API_KEY".into()),
);

assert!(matches!(
    provider_auth_config(
        &json!({"api_key": "sk-manually-configured"}),
        "openai",
        "OPENAI_API_KEY",
    )?
    .credential,
    ConfiguredCredential::Inline(_),
));

assert_eq!(
    provider_auth_config(
        &json!({"auth_ref": "keychain:gestalt/openai"}),
        "openai",
        "OPENAI_API_KEY",
    )?
    .credential,
    ConfiguredCredential::Keychain("gestalt/openai".into()),
);
```

Add a compatibility test proving legacy `auth_ref: "secret:provider/openai"` resolves as the existing keychain account. Add a config-resolution test for its deprecation warning. Add negative tests for `api_key: "$"`, invalid environment names, unsupported `auth_ref` prefixes, and multiple auth fields in one provider object. Error strings may name provider/field/source kind but must not contain the literal key.

- [ ] **Step 2: Add resolver-chain precedence tests**

Assert:

```text
CLI --api-key session override wins over configured auth
configured inline key resolves as inline_config
api_key="$NAME" resolves the same source as api_key_env="NAME"
keychain reference resolves the named account
missing configured source falls through to interactive prompt only when allowed
providers with no configured auth can make unauthenticated requests
```

There is no precedence among `auth_ref`, `api_key_env`, and `api_key` inside one resolved config: merge/validation makes them mutually exclusive. The only runtime precedence is ephemeral CLI/session override → configured source → interactive prompt.

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-models --test auth_tests configured_credential_
cargo test -p gestalt-models --test no_secret_tests inline_api_key_
cargo test -p gestalt-harness --test connect_cli_tests onboarding_
```

Expected: inline keys are rejected and keychain references still use the legacy `secret:` syntax.

- [ ] **Step 4: Introduce one typed internal credential source**

In `crates/gestalt-models/src/auth.rs`, use:

```rust
#[derive(Clone, PartialEq, Eq)]
pub enum ConfiguredCredential {
    None,
    Environment(String),
    Keychain(String),
    Inline(Arc<str>),
}

pub struct ProviderAuthConfig {
    pub provider_id: String,
    pub credential: ConfiguredCredential,
}
```

Implement custom `Debug` for `ConfiguredCredential` so `Inline` renders as `Inline("[REDACTED]")`. Never implement a plaintext `Display`.

`provider_auth_config` must parse:

```text
auth_ref: "keychain:gestalt/openai" -> Keychain("gestalt/openai")
auth_ref: "secret:provider/openai"   -> Keychain("provider/openai"); config validation warns
api_key_env: "OPENAI_API_KEY"       -> Environment("OPENAI_API_KEY")
api_key: "$OPENAI_API_KEY"          -> Environment("OPENAI_API_KEY")
api_key: "sk-..."                    -> Inline(secret)
no auth fields + default_env="none"  -> None
no auth fields + another default     -> Environment(default_env)
```

- [ ] **Step 5: Add configured-source resolvers**

Update environment and keychain resolvers to act only on their matching `ConfiguredCredential` variant. Add an inline resolver that clones the secret directly into `ResolvedCredential` with:

```rust
CredentialSource::InlineConfig
```

Keep the chain order:

```text
SessionCredentialResolver
InlineCredentialResolver
EnvironmentCredentialResolver
KeychainCredentialResolver
PromptCredentialResolver (interactive only)
```

The source resolvers are mutually exclusive for configured credentials, so their relative order does not create hidden config precedence.

- [ ] **Step 6: Keep CLI/TUI onboarding keychain-only**

When a user pastes a key into CLI/TUI onboarding:

1. store it under keychain account `gestalt/<provider_id>`;
2. write only `auth_ref: "keychain:gestalt/<provider_id>"`;
3. clear `api_key` and `api_key_env` in that provider layer;
4. never pass the literal to `write_workspace_config_file`.

When onboarding is explicitly given `--api-key-env NAME`, write only `api_key_env: "NAME"` and do not prompt for or store a key.

Keep `--api-key` on run/chat as an ephemeral override. It must not mutate config or keychain.

- [ ] **Step 7: Warn on manual inline credentials without leaking them**

Add a structured warning to resolved config:

```text
providers.openai.api_key contains an inline credential; restrict gestalt.json permissions and avoid committing it
providers.openai.auth_ref uses legacy secret: syntax; rewrite it as keychain:
```

Surface it once during config validation and in provider doctor output. Do not include key length, prefix, suffix, hash, or value.

Ensure config/provider inspection recursively renders:

```json
{"api_key": "[REDACTED]"}
```

Run manifests, `ResolvedModelSnapshot`, traces, runtime inspection, error messages, and config fingerprints must not contain the literal.

- [ ] **Step 8: Run auth and onboarding tests**

Run:

```bash
cargo test -p gestalt-models --test auth_tests
cargo test -p gestalt-models --test no_secret_tests
cargo test -p gestalt-harness --test connect_cli_tests
cargo test -p gestalt-harness --test tui_smoke_tests
```

Expected: all four manual auth forms resolve, onboarding writes only keychain references, inline usage warns, and no rendered output contains literal test secrets.

- [ ] **Step 9: Commit**

```bash
git add crates/gestalt-models/src/auth.rs crates/gestalt-models/src/lib.rs crates/gestalt-models/tests/auth_tests.rs crates/gestalt-models/tests/no_secret_tests.rs crates/gestalt-cli/src/auth.rs crates/gestalt-cli/src/connect.rs crates/gestalt-cli/src/output.rs crates/gestalt-cli/src/tui crates/gestalt-cli/tests/connect_cli_tests.rs crates/gestalt-cli/tests/tui_smoke_tests.rs
git commit -m "feat(auth): support string-based provider credentials"
```

### Task 3: Resolve and validate one complete provider/model snapshot

**Files:**

- Modify: `crates/gestalt-cli/src/config.rs`
- Modify: `crates/gestalt-models/src/catalog.rs`
- Modify: `crates/gestalt-cli/src/models.rs`
- Test: `crates/gestalt-cli/tests/config_tests.rs`
- Test: `crates/gestalt-models/tests/catalog_tests.rs`

- [ ] **Step 1: Write failing resolution-order tests**

Cover these independent cases:

1. configured model metadata overrides catalog metadata;
2. catalog metadata fills missing configured fields;
3. CLI context-window override beats profile and model values;
4. profile context-window override beats workspace and model values;
5. workspace override beats model metadata;
6. an unknown custom model with no limits resolves to the 32,000 fallback and emits a validation warning;
7. `reserved_output_tokens` defaults to `min(model.max_output_tokens, 8192)`.

Use `ResolvedProvider.resolved_model` for assertions.

- [ ] **Step 2: Write failing option-validation tests**

Assert these return `ConfigError::InvalidValue`:

```text
anthropic_messages + text_verbosity
openai_chat_completions + text_verbosity
openai_responses + thinking
model capabilities tools=true while provider capabilities tools=false
request_path without a leading slash
selected variant not present
default_model not present in provider config or catalog
```

Unknown `adapter_options` must remain accepted.

- [ ] **Step 3: Run focused tests and verify they fail**

Run:

```bash
cargo test -p gestalt-harness --test config_tests resolved_model_
cargo test -p gestalt-harness --test config_tests incompatible_
```

Expected: failures show missing model metadata and validation.

- [ ] **Step 4: Make `ResolvedProvider` carry the complete run snapshot**

Replace behavior-driving string fields with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigWarningCode {
    InlineCredential,
    ConservativeModelFallback,
    UnknownAdapterOption,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigWarning {
    pub code: ConfigWarningCode,
    pub field: String,
    pub message: String,
}

pub struct ResolvedProvider {
    pub profile_name: Option<String>,
    pub resolved_model: ResolvedModelSnapshot,
    pub base_url: String,
    pub request_path: Option<String>,
    pub auth: ProviderAuthConfig,
    pub models_endpoint: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub request: ProviderRequestConfig,
    pub resolved_options: ModelOptionsConfig,
    pub warnings: Vec<ConfigWarning>,
}
```

Keep convenience accessors:

```rust
pub fn provider_name(&self) -> &str;
pub fn model(&self) -> &str;
pub const fn api_format(&self) -> ApiFormat;
```

`provider_json()` contains only identity, format, transport, request, and capability fields. Pass `ResolvedProvider.auth` separately into provider construction so the generic JSON value never carries credentials. Public inspection uses a redacted auth-source descriptor such as `keychain`, `environment`, or `inline`; it never exposes an account secret or literal value.

- [ ] **Step 5: Centralize model catalog layers**

Move the OpenRouter, Ollama, Groq, and Together built-ins currently declared in `gestalt-cli/src/models.rs` into `gestalt-models/src/catalog.rs`.

Convert configured CLI model definitions to `Vec<ModelInfo>` in `gestalt-cli/src/models.rs`, then pass that provider-neutral layer through the existing API:

```rust
pub fn with_layer(self, models: Vec<ModelInfo>) -> Self;
```

Do not add a `gestalt-models -> gestalt-cli` dependency.

- [ ] **Step 6: Implement validation and deterministic fallback**

Implement:

```rust
fn validate_resolved_provider(resolved: &ResolvedProvider) -> Result<(), HarnessError>;
fn validate_model_options(
    api_format: ApiFormat,
    capabilities: &ModelCapabilities,
    options: &ModelOptionsConfig,
) -> Result<(), HarnessError>;
```

Use 32,000 only after configured and catalog metadata have both failed to provide a context limit. Return structured warnings from resolution and surface them in doctor output; do not print from library-like resolver code.

- [ ] **Step 7: Run model and CLI tests**

Run:

```bash
cargo test -p gestalt-models --test catalog_tests
cargo test -p gestalt-harness --test config_tests
cargo test -p gestalt-harness --test provider_model_cli_tests
```

Expected: all tests pass and configured models appear in model listing/inspection with `ModelInfoSource::UserDefined`.

- [ ] **Step 8: Commit**

```bash
git add crates/gestalt-cli/src/config.rs crates/gestalt-cli/src/models.rs crates/gestalt-models/src/catalog.rs crates/gestalt-cli/tests/config_tests.rs crates/gestalt-models/tests/catalog_tests.rs crates/gestalt-cli/tests/provider_model_cli_tests.rs
git commit -m "feat(models): resolve validated model metadata"
```

### Task 4: Split OpenAI Chat Completions from common transport code

**Files:**

- Delete: `crates/gestalt-models/src/openai.rs`
- Create: `crates/gestalt-models/src/openai/mod.rs`
- Create: `crates/gestalt-models/src/openai/common.rs`
- Create: `crates/gestalt-models/src/openai/chat_completions.rs`
- Modify: `crates/gestalt-models/src/lib.rs`
- Test: `crates/gestalt-models/tests/provider_stream_tests.rs`

- [ ] **Step 1: Add a Chat Completions request-path regression test**

Expose a test-only request builder or use a local HTTP fixture. Assert:

```text
request path = /chat/completions
body contains messages
body does not contain instructions
body does not contain input
body does not contain text
```

Also assert `text_verbosity` cannot reach this adapter because resolution rejects it.

- [ ] **Step 2: Run the regression test**

Run:

```bash
cargo test -p gestalt-models openai_chat_uses_chat_completions_shape
```

Expected: the body assertion fails because the current adapter conditionally inserts Responses-only `text`.

- [ ] **Step 3: Move the current implementation without changing normalized behavior**

Create:

```rust
pub use chat_completions::OpenAiChatCompletionsProvider;
pub use chat_completions::OpenAiChatCompletionsProvider as OpenAiProvider;
pub use responses::OpenAiResponsesProvider;
```

Move shared auth headers, status error mapping, numeric conversion, and generic SSE helpers into `common.rs`. Keep Chat message/tool conversion and Chat delta normalization in `chat_completions.rs`.

Remove `reasoning` and `text` serialization from the Chat adapter. If supported Chat models need `reasoning_effort`, serialize only the Chat-specific documented field accepted by the selected compatible provider through `adapter_options`; do not reuse the Responses object shape.

Provider constructors receive `ProviderAuthConfig` as a separate typed argument; they must not reparse credentials from generic provider JSON.

- [ ] **Step 4: Run existing provider tests**

Run:

```bash
cargo test -p gestalt-models --test provider_stream_tests
cargo test -p gestalt-models --test provider_tool_schema_tests
cargo test -p gestalt-models
```

Expected: all existing Anthropic and Chat Completions behavior remains green.

- [ ] **Step 5: Commit**

```bash
git add crates/gestalt-models/src crates/gestalt-models/tests/provider_stream_tests.rs
git commit -m "refactor(models): isolate chat completions adapter"
```

### Task 5: Implement the OpenAI Responses adapter

**Files:**

- Create: `crates/gestalt-models/src/openai/responses.rs`
- Create: `tests/fixtures/provider-streams/openai-responses-text.sse`
- Create: `tests/fixtures/provider-streams/openai-responses-tool.sse`
- Modify: `crates/gestalt-models/tests/provider_stream_tests.rs`
- Test: `crates/gestalt-models/src/openai/responses.rs`

- [ ] **Step 1: Write failing request serialization tests**

Use one provider-neutral `ProviderRequest` containing system, user, assistant tool call, and tool-result messages. Assert the Responses body contains:

```json
{
  "model": "gpt-5.1",
  "instructions": "system text",
  "input": [
    {
      "role": "user",
      "content": [
        {"type": "input_text", "text": "question"}
      ]
    },
    {
      "type": "function_call",
      "call_id": "call_1",
      "name": "read",
      "arguments": "{\"path\":\"README.md\"}"
    },
    {
      "type": "function_call_output",
      "call_id": "call_1",
      "output": "contents"
    }
  ],
  "tools": [
    {
      "type": "function",
      "name": "read",
      "description": "Read a file",
      "parameters": {"type": "object"},
      "strict": true
    }
  ],
  "reasoning": {"effort": "high"},
  "text": {"verbosity": "medium"},
  "stream": true
}
```

Use structural JSON assertions; do not compare property ordering.

- [ ] **Step 2: Write failing Responses SSE normalization tests**

Fixtures must cover:

- `response.output_text.delta` → `AgentEvent::Text`;
- `response.output_item.added` for a function call → remembered call ID/name;
- `response.function_call_arguments.delta` → `AgentEvent::ToolCallStreamed`;
- `response.completed.response.usage` → `AgentEvent::Usage`;
- `response.completed` → `AgentEvent::Stop { EndTurn }`;
- `response.incomplete` with `max_output_tokens` → `StopReason::MaxOutput`;
- malformed JSON → sanitized provider error.

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-models openai_responses_
```

Expected: compilation fails because `OpenAiResponsesProvider` is not implemented.

- [ ] **Step 4: Implement the dedicated adapter**

Implement `Provider` for `OpenAiResponsesProvider` with:

```text
POST {base_url}{request_path_or_/responses}
```

Keep its state machine local to `responses.rs`. Correlate streamed function-call argument deltas by `output_index`/`item_id`, normalize them to the existing `ToolCallStreamed` contract, and never execute tools in the adapter.

Build the `reqwest::Client` with configured `timeout_ms`. Apply `stream_chunk_timeout_ms` to each next SSE item with `tokio::time::timeout`; convert expiry to `ProviderError::Timeout`.

Accept the already parsed `ProviderAuthConfig` separately from transport config, matching the Chat and Anthropic constructors.

- [ ] **Step 5: Run provider tests**

Run:

```bash
cargo test -p gestalt-models openai_responses_
cargo test -p gestalt-models
```

Expected: all Responses request/stream tests and all existing provider tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/gestalt-models tests/fixtures/provider-streams
git commit -m "feat(models): add openai responses adapter"
```

### Task 6: Route provider construction by `ApiFormat`

**Files:**

- Modify: `crates/gestalt-models/src/registry.rs`
- Modify: `crates/gestalt-cli/src/provider_catalog.rs`
- Modify: `crates/gestalt-cli/src/runtime.rs`
- Modify: `crates/gestalt-cli/src/providers.rs`
- Test: `crates/gestalt-cli/tests/provider_model_cli_tests.rs`
- Test: `crates/gestalt-cli/tests/run_smoke_tests.rs`

- [ ] **Step 1: Add failing routing tests**

Assert:

```text
anthropic + anthropic_messages -> AnthropicProvider
openrouter + openai_chat_completions -> OpenAiChatCompletionsProvider with id=openrouter
openai + openai_responses -> OpenAiResponsesProvider
custom provider identity + openai_chat_completions -> Chat adapter retaining custom identity
```

Do not identify an adapter by comparing a user-facing provider ID to `"openai-compatible"`.

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-harness --test provider_model_cli_tests provider_adapter_
```

Expected: OpenAI Responses routing fails because registry lookup still uses `kind`.

- [ ] **Step 3: Replace behavior-driving registry lookup**

Add:

```rust
pub fn get_by_api_format_with_resolver(
    provider_id: &str,
    api_format: ApiFormat,
    config: ProviderConfig,
    auth: ProviderAuthConfig,
    resolver: Arc<dyn CredentialResolver>,
) -> Result<Arc<dyn Provider>, HarnessError>;
```

Use an explicitly registered custom factory when `provider_id` names one. Otherwise match built-in HTTP adapter behavior only on `ApiFormat`. This preserves the existing extension point for test/custom providers without using provider identity to choose among Anthropic, Chat Completions, and Responses serializers.

Update built-ins:

```text
openai     -> openai_responses
anthropic  -> anthropic_messages
openrouter -> openai_chat_completions
ollama     -> openai_chat_completions
groq       -> openai_chat_completions
together   -> openai_chat_completions
```

Update provider probing to choose auth headers by `api_format`, while continuing to use `models_endpoint` when explicitly configured.

- [ ] **Step 4: Run routing and smoke tests**

Run:

```bash
cargo test -p gestalt-harness --test provider_model_cli_tests
cargo test -p gestalt-harness --test run_smoke_tests
```

Expected: all provider identities route through the configured format and custom mock provider tests remain supported through their registered factory path.

- [ ] **Step 5: Commit**

```bash
git add crates/gestalt-models/src/registry.rs crates/gestalt-cli/src/provider_catalog.rs crates/gestalt-cli/src/runtime.rs crates/gestalt-cli/src/providers.rs crates/gestalt-cli/tests/provider_model_cli_tests.rs crates/gestalt-cli/tests/run_smoke_tests.rs
git commit -m "feat(runtime): select provider adapters by api format"
```

### Task 7: Derive every run's token budget from the resolved model

**Files:**

- Modify: `crates/gestalt-runtime/src/config.rs`
- Modify: `crates/gestalt-runtime/src/runtime.rs`
- Modify: `crates/gestalt-cli/src/runtime.rs`
- Modify: `crates/gestalt-cli/src/main.rs`
- Test: `crates/gestalt-runtime/tests/runtime_run_tests.rs`
- Test: `crates/gestalt-cli/tests/config_tests.rs`

- [ ] **Step 1: Write failing budget tests**

Add unit tests for:

```rust
let budget = build_token_budget(
    &resolved_model,
    Some(64_000),
    None,
    Some(2_048),
);
assert_eq!(budget.model_limit, 64_000);
assert_eq!(budget.reserved_output, 8_192);
```

Also assert:

- without overrides, `model_limit == resolved_model.max_context_tokens`;
- reserved output never exceeds model output limit;
- reserved output plus safety margin cannot consume the whole model limit;
- invalid combinations return `ConfigError::InvalidValue`, not a panic.

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-runtime build_token_budget_
```

Expected: tests fail because runtime still defaults to 120,000/8,000.

- [ ] **Step 3: Put the resolved snapshot in `RuntimeConfig`**

Replace independent model/provider/limit fields with:

```rust
pub resolved_model: ResolvedModelSnapshot,
pub context_window_override: Option<usize>,
pub reserved_output_tokens: Option<usize>,
pub safety_margin_tokens: Option<usize>,
```

Keep `max_tokens` only if it represents the effective request output limit; derive it once from resolved model/options and validate it is no larger than `resolved_model.max_output_tokens`.

Update runtime inspection and variant fingerprinting to read the snapshot.

- [ ] **Step 4: Implement one token-budget constructor**

Add a pure helper in `gestalt-runtime/src/config.rs` and call it from both:

- `AgentRuntime::run_prompt`;
- `crates/gestalt-cli/src/sessions.rs` when constructing a continued `Session`.

Initialize used-token counters to zero for each new run. They describe the new projection, not accumulated historical usage.

- [ ] **Step 5: Expose a run-level CLI override**

Add global:

```text
--context-window <TOKENS>
```

Map it to `CliOverrides.context_window_override`. Validate values are non-zero and larger than reserved output plus safety margin.

- [ ] **Step 6: Run runtime and CLI tests**

Run:

```bash
cargo test -p gestalt-runtime --test runtime_run_tests
cargo test -p gestalt-harness --test config_tests
```

Expected: all budgets derive from selected-model metadata or explicit overrides; no 120,000 fallback remains.

- [ ] **Step 7: Commit**

```bash
git add crates/gestalt-runtime/src/config.rs crates/gestalt-runtime/src/runtime.rs crates/gestalt-runtime/tests/runtime_run_tests.rs crates/gestalt-cli/src/runtime.rs crates/gestalt-cli/src/main.rs crates/gestalt-cli/tests/config_tests.rs
git commit -m "fix(context): budget each run from selected model"
```

### Task 8: Persist run snapshots and reset projection state on model switches

**Files:**

- Modify: `crates/gestalt-trace/src/run_manifest.rs`
- Modify: `crates/gestalt-trace/src/resume.rs`
- Modify: `crates/gestalt-cli/src/run.rs`
- Modify: `crates/gestalt-cli/src/sessions.rs`
- Test: `crates/gestalt-cli/tests/sessions_cli_tests.rs`
- Test: `crates/gestalt-trace/tests/golden_trace_tests.rs`

- [ ] **Step 1: Add failing manifest compatibility tests**

Assert:

- a new manifest round-trips `resolved_model: Some(snapshot)`;
- an old version-1 manifest without the field loads with `None`;
- newly created run and continued-run manifests always persist `Some`.

- [ ] **Step 2: Add failing session-switch integration tests**

Build a completed parent run using an Anthropic snapshot and canonical history. Continue it with OpenAI Responses and assert:

```text
same session_id
new run_id
parent_run_id points to Anthropic run
canonical history message IDs/content are retained
new manifest snapshot identifies OpenAI Responses
new TokenBudget uses OpenAI model limit
ContextProjectionState starts clean
```

Add a second test continuing with the exact same snapshot and assert compatible projection state is reused.

Add smaller/larger context tests:

- switching to 8,192 causes projection omission/compaction without deleting canonical history;
- switching back to 200,000 starts a fresh projection and can include more canonical messages.

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-harness --test sessions_cli_tests same_session_can_switch_provider_and_rebudget
cargo test -p gestalt-harness --test sessions_cli_tests switching_to_larger_model_rebuilds_projection
cargo test -p gestalt-trace --test golden_trace_tests run_manifest_resolved_model_
```

Expected: the first continuation inherits the parent checkpoint budget and manifests lack snapshots.

- [ ] **Step 4: Add the optional manifest field**

In `RunManifest`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub resolved_model: Option<ResolvedModelSnapshot>,
```

Add a helper constructor or builder so `run.rs`, `sessions.rs`, and tests do not repeatedly assemble version/lifecycle boilerplate.

Add the same optional snapshot field to `ResumeAnalysis` and populate it directly from the parent `RunManifest`.

- [ ] **Step 5: Rebuild continuation state according to snapshot compatibility**

In `run_session_action`:

```rust
let selection_changed =
    analysis.resolved_model.as_ref() != Some(&resolved_provider.resolved_model);

let context_state = if selection_changed {
    ContextProjectionState::default()
} else {
    analysis.context_state.clone()
};
```

Always construct the new token budget from the current resolved model. Never copy `analysis.token_budget.model_limit` into a new run.

Only load/reuse a parent prompt snapshot when the resolved snapshot is unchanged. Canonical history is always reconstructed regardless of selection.

- [ ] **Step 6: Emit `RunStarted` once per run**

Emit the event immediately after the trace sink and manifest are initialized, before `WorkspaceSnapshotCaptured` and before context construction.

Ensure both new-run and continued-run paths use the same helper so event ordering cannot drift.

- [ ] **Step 7: Run session and trace tests**

Run:

```bash
cargo test -p gestalt-harness --test sessions_cli_tests
cargo test -p gestalt-trace --test golden_trace_tests
```

Expected: lineage, history preservation, model switching, old-manifest compatibility, and trace order all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/gestalt-trace crates/gestalt-cli/src/run.rs crates/gestalt-cli/src/sessions.rs crates/gestalt-cli/tests/sessions_cli_tests.rs
git commit -m "feat(trace): persist per-run provider model snapshots"
```

### Task 9: Make cache identity and projection traces provider-aware

**Files:**

- Modify: `crates/gestalt-core/src/model.rs`
- Modify: `crates/gestalt-core/src/event.rs`
- Modify: `crates/gestalt-core/src/agent.rs`
- Modify: `crates/gestalt-runtime/src/context.rs`
- Test: `crates/gestalt-core/tests/core.rs`
- Test: `crates/gestalt-runtime/tests/runtime_run_tests.rs`
- Test: `crates/gestalt-models/src/anthropic.rs`
- Test: `crates/gestalt-models/src/openai/chat_completions.rs`
- Test: `crates/gestalt-models/src/openai/responses.rs`

- [ ] **Step 1: Add failing cache-key tests**

Define expected inequality for changes to:

```text
provider_id
api_format
model_id
prompt_prefix_hash
provider_tool_schema_hash
```

Define equality for two identical inputs.

- [ ] **Step 2: Add failing adapter cache tests**

Assert:

- Anthropic renders explicit cache control from `PromptCachePlan`;
- Chat Completions does not emit Anthropic `cache_control`;
- Responses does not emit Anthropic `cache_control`;
- OpenAI adapter-specific `prompt_cache_key` and `prompt_cache_retention` are copied only from validated `adapter_options`.

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-core provider_cache_key_
cargo test -p gestalt-models cache_
```

Expected: cache-key types and Responses cache behavior are absent.

- [ ] **Step 4: Add a deterministic provider cache key**

Define:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCacheKey {
    pub provider_id: String,
    pub api_format: ApiFormat,
    pub model_id: String,
    pub prompt_prefix_hash: String,
    pub provider_tool_schema_hash: String,
}
```

Add a SHA-256 `fingerprint()` method that serializes fields in fixed order.

- [ ] **Step 5: Extend context trace data without breaking old fixtures**

Add optional/defaulted fields to `AgentEvent::ContextBuilt`:

```rust
model_limit: Option<usize>,
reserved_output_tokens: Option<usize>,
canonical_message_count: Option<usize>,
projected_message_count: Option<usize>,
compaction_applied: Option<bool>,
tool_results_cleared: Option<usize>,
prompt_prefix_hash: Option<String>,
provider_tool_schema_hash: Option<String>,
provider_cache_key: Option<String>,
```

Populate them from `PreparedContext.manifest`, `ContextPacket`, the current token budget, adapted tool schemas, and `SessionConfig.resolved_model`.

- [ ] **Step 6: Run core, runtime, and provider tests**

Run:

```bash
cargo test -p gestalt-core
cargo test -p gestalt-runtime --test runtime_run_tests
cargo test -p gestalt-models
```

Expected: cache keys are deterministic and adapter cache behavior remains format-specific.

- [ ] **Step 7: Commit**

```bash
git add crates/gestalt-core crates/gestalt-runtime/src/context.rs crates/gestalt-runtime/tests/runtime_run_tests.rs crates/gestalt-models
git commit -m "feat(context): trace provider-aware cache identity"
```

### Task 10: Extend provider doctor and session inspection

**Files:**

- Modify: `crates/gestalt-cli/src/providers.rs`
- Modify: `crates/gestalt-cli/src/sessions.rs`
- Modify: `crates/gestalt-cli/src/output.rs`
- Test: `crates/gestalt-cli/tests/provider_model_cli_tests.rs`
- Test: `crates/gestalt-cli/tests/sessions_cli_tests.rs`
- Test: `crates/gestalt-cli/tests/format_contract_tests.rs`

- [ ] **Step 1: Add failing doctor-output tests**

Assert provider doctor JSON/text contains:

```text
provider
api_format
default_model
max_context_tokens
max_output_tokens
prompt_cache_mode
auth_status
validation_warnings
```

Doctor without `--live` must not perform network I/O.

- [ ] **Step 2: Add failing session inspection tests**

For a two-run session, assert inspection shows per run:

```text
run_id
provider
model
variant
api_format
created_at
lifecycle_state
parent_run_id
```

For an older manifest, render provider/model as `unknown` rather than failing.

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
cargo test -p gestalt-harness --test provider_model_cli_tests doctor_reports_resolved_model_metadata
cargo test -p gestalt-harness --test sessions_cli_tests inspect_lists_per_run_model_snapshots
```

Expected: output structs lack resolved model fields.

- [ ] **Step 4: Implement report changes**

Keep provider inspection/doctor as CLI presentation over `ResolvedProvider`; do not duplicate validation rules in `providers.rs`.

Read session run metadata from `RunManifest.resolved_model`. Do not reconstruct it from current workspace config, because historical runs must report their original snapshots.

- [ ] **Step 5: Run CLI report tests**

Run:

```bash
cargo test -p gestalt-harness --test provider_model_cli_tests
cargo test -p gestalt-harness --test sessions_cli_tests
cargo test -p gestalt-harness --test format_contract_tests
```

Expected: all text and JSON output contracts pass.

- [ ] **Step 6: Commit**

```bash
git add crates/gestalt-cli/src/providers.rs crates/gestalt-cli/src/sessions.rs crates/gestalt-cli/src/output.rs crates/gestalt-cli/tests
git commit -m "feat(cli): report provider model run metadata"
```

### Task 11: Regenerate schema, migrate examples, and document the breaking config change

**Files:**

- Create: `docs/migrations/provider-kind-to-api-format.md`
- Modify: `docs/schemas/gestalt.schema.json`
- Modify: `README.md`
- Modify: `tests/fixtures/config/v1/full_valid.json`
- Modify: `tests/fixtures/workspaces/minimal/gestalt.json`
- Modify: `tests/fixtures/workspaces/profiled/gestalt.json`
- Modify: `crates/gestalt-cli/tests/config_schema_tests.rs`

- [ ] **Step 1: Update all active config examples**

Replace:

```json
{"kind": "openai-compatible"}
```

with:

```json
{"api_format": "openai_chat_completions"}
```

Use `openai_responses` for the built-in OpenAI example and `anthropic_messages` for Anthropic.

Use keychain references in onboarding-generated examples:

```json
{"auth_ref": "keychain:gestalt/openai"}
```

Document manual alternatives next to, not instead of, the recommended form:

```json
{"api_key_env": "OPENAI_API_KEY"}
{"api_key": "$OPENAI_API_KEY"}
{"api_key": "sk-manually-configured"}
```

Every literal example must use a conspicuously fake value and include a warning against committing inline credentials.

- [ ] **Step 2: Write the migration guide**

Document this exact mapping:

```text
anthropic         -> anthropic_messages
openai            -> openai_responses for built-in OpenAI
openai-compatible -> openai_chat_completions
```

State that custom OpenAI deployments that only support Chat Completions must choose `openai_chat_completions`, even when provider identity is `openai`.

Also document that CLI/TUI onboarding always stores pasted keys in the OS keychain, while inline `api_key` is accepted only as manual power-user configuration.

Document `secret:<account>` as a read-only legacy alias for existing connections. New config and onboarding must emit `keychain:<account>`.

- [ ] **Step 3: Regenerate the checked-in JSON schema**

Run:

```bash
UPDATE_SCHEMA=1 cargo test -p gestalt-harness --test config_schema_tests test_schema_drift
```

Expected: test passes and `docs/schemas/gestalt.schema.json` contains a closed `ApiFormat` enum and model metadata fields, with no provider `kind`.

- [ ] **Step 4: Verify schema drift and fixtures**

Run:

```bash
cargo test -p gestalt-harness --test config_schema_tests
cargo test -p gestalt-harness --test fixture_smoke
```

Expected: checked-in schema matches generated output and all version-1 fixtures parse.

- [ ] **Step 5: Commit**

```bash
git add docs/migrations/provider-kind-to-api-format.md docs/schemas/gestalt.schema.json README.md tests/fixtures crates/gestalt-cli/tests/config_schema_tests.rs
git commit -m "docs(config): migrate provider kind to api format"
```

### Task 12: Run end-to-end acceptance and workspace verification

**Files:**

- Modify only files required by failures directly caused by Tasks 1–11

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --all -- --check
```

Expected: exit 0. If it fails, run `cargo fmt --all`, inspect the diff, and rerun the check.

- [ ] **Step 2: Run focused acceptance suites**

Run:

```bash
cargo test -p gestalt-core
cargo test -p gestalt-models
cargo test -p gestalt-runtime --test runtime_run_tests
cargo test -p gestalt-runtime --test context_management_tests
cargo test -p gestalt-trace
cargo test -p gestalt-harness --test config_tests
cargo test -p gestalt-harness --test config_schema_tests
cargo test -p gestalt-harness --test connect_cli_tests
cargo test -p gestalt-harness --test provider_model_cli_tests
cargo test -p gestalt-harness --test sessions_cli_tests
cargo test -p gestalt-harness --test run_smoke_tests
```

Expected: all commands exit 0.

- [ ] **Step 3: Run workspace tests**

Run:

```bash
cargo test --workspace --all-targets --all-features --locked
```

Expected: all workspace tests pass.

- [ ] **Step 4: Run strict linting**

Run:

```bash
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Expected: exit 0 with no warnings.

- [ ] **Step 5: Verify acceptance criteria manually from artifacts**

Inspect one new run and one continued run:

```bash
cargo run -p gestalt-harness -- --workspace tests/fixtures/workspaces/minimal --format json providers inspect openai
cargo run -p gestalt-harness -- --workspace tests/fixtures/workspaces/minimal --format json models inspect openai/gpt-4o-mini
```

Expected:

- provider output includes `api_format`;
- model output includes context and output limits;
- new run manifests include `resolved_model`;
- a continued run can select another provider/model without changing the session ID;
- checkpoint canonical history remains intact;
- continued-run budget matches the newly selected model;
- Chat and Responses requests route to distinct paths and serializers.

- [ ] **Step 6: Review the final diff for forbidden architecture drift**

Run:

```bash
git diff --stat
git diff -- crates/gestalt-core crates/gestalt-runtime crates/gestalt-models crates/gestalt-cli crates/gestalt-trace docs tests
```

Confirm:

- no provider wire payload entered `gestalt-runtime`;
- no provider-specific message entered canonical `Session.history`;
- no second run/session store was introduced;
- no secret was persisted in a run snapshot;
- no unrelated extension/context refactor was included.

- [ ] **Step 7: Commit final verification fixes**

```bash
git add crates docs tests README.md
git commit -m "test: verify provider model normalization"
```

Skip this commit if verification required no file changes.

## 5. Acceptance-Criteria Traceability

| Feature-spec criterion | Plan coverage |
|---|---|
| `api_format` drives serialization | Tasks 2, 3, 6 |
| Keychain-default onboarding with string auth forms | Tasks 2, 2A, 3, 6, 10, 11 |
| Separate Chat Completions and Responses formats | Tasks 4, 5 |
| Anthropic Messages remains supported | Tasks 4, 6, 9 |
| Provider-scoped context windows | Tasks 2, 3 |
| Selected model drives context budget | Task 7 |
| Same session can switch provider/model | Task 8 |
| Canonical history is not reset | Task 8 |
| Resumed sessions may use another selection | Task 8 |
| Prompt cache remains provider-aware | Task 9 |
| Adapter selection is based on format | Task 6 |
| Incompatible options fail validation | Task 3 |
| Run traces include resolved snapshots | Tasks 8, 9 |
| Schema, routing, switching, and budget tests | Tasks 2–9, 12 |

## 6. Explicitly Deferred Follow-ups

These are not hidden implementation work:

- persisted per-session default provider/model;
- a dedicated `session set-default-model` command;
- provider re-execution during replay;
- remapped cross-provider live replay;
- mandatory live model discovery;
- pricing refresh;
- provider failover;
- model routing;
- cache portability across providers.

The resolved run snapshot and manifest compatibility added here are prerequisites for replay/provider remapping, but do not implement it.
