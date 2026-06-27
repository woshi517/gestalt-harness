# Feature/Fix Specification: Provider & Model Schema Normalization

## 1. Summary

Gestalt currently has a working provider/model configuration path, but the schema shape is not explicit enough for the real provider formats Gestalt needs to support.

The main issue is that provider behavior is currently inferred from a broad `kind` value such as `openai`, `openai-compatible`, or `anthropic`. This is too coarse because modern providers are not just “OpenAI-like” or “Anthropic-like.” They differ by API format, endpoint shape, request serialization, streaming protocol, tool schema format, reasoning controls, prompt caching behavior, and model metadata.

This feature fixes the provider/model schema so Gestalt can cleanly support:

* Anthropic Messages API: `/v1/messages`
* OpenAI Chat Completions API: `/chat/completions`
* OpenAI Responses API: `/responses`
* OpenAI-compatible providers such as OpenRouter, Groq, Together, Ollama, etc.
* Provider-scoped model definitions with explicit context window sizes
* Model variants such as `low`, `medium`, `high`, `xhigh`
* Runtime model switching without losing canonical context
* Provider switching between runs inside the same session
* Resuming a session with a different provider/model while preserving session history

The core architectural direction is:

> Session context must be provider-neutral. Provider/model selection belongs to a run, not to the canonical session history.

---

## 2. Current Implementation Snapshot

This spec is anchored on the current Gestalt architecture.

### 2.1 Current config layer

Current config is primarily handled in:

```text
crates/gestalt-cli/src/config.rs
crates/gestalt-cli/src/provider_catalog.rs
crates/gestalt-cli/src/providers.rs
```

The current provider config shape already supports several useful fields:

```rust
ProviderConfig {
    id,
    display_name,
    protocol,
    base_url,
    default_model,
    api_key_env,
    auth_ref,
    kind,
    models_endpoint,
    headers,
    request,
    capabilities,
    models,
}
```

Built-in providers currently include examples such as:

```text
openai
anthropic
openrouter
ollama
groq
together
```

The current system can resolve a provider from config/profile/defaults and construct a concrete provider implementation.

### 2.2 Current provider implementations

Current provider implementations are roughly split across:

```text
crates/gestalt-models/src/openai.rs
crates/gestalt-models/src/anthropic.rs
```

Current behavior:

| Provider style                   | Current support        |
| -------------------------------- | ---------------------- |
| Anthropic Messages API           | Supported              |
| OpenAI Chat Completions          | Supported              |
| OpenAI-compatible chat endpoints | Supported              |
| OpenAI Responses API             | Not properly supported |

The OpenAI adapter currently assumes a Chat Completions style endpoint:

```text
{base_url}/chat/completions
```

This is not sufficient for the OpenAI Responses API, which has a different request and response shape.

### 2.3 Current model metadata

Model metadata exists in:

```text
crates/gestalt-core/src/model.rs
crates/gestalt-models/src/catalog.rs
```

The core model metadata already has the right conceptual shape:

```rust
ModelInfo {
    id,
    provider,
    display_name,
    max_context_tokens,
    max_output_tokens,
    supports_tools,
    supports_streaming,
    supports_vision,
    supports_json_mode,
    input_cost_per_million,
    output_cost_per_million,
    source,
    last_updated,
}
```

However, the user-facing provider-scoped model config does not currently expose enough of this metadata.

Current configured model definitions support options and variants, but they do not properly expose per-model context window size:

```rust
ModelDefinitionConfig {
    display_name,
    options,
    variants,
}
```

This means runtime context management cannot reliably derive its token budget from the selected model.

### 2.4 Current context pipeline

Context construction is already heading in the correct direction.

The existing architecture separates:

```text
Canonical session history
        ↓
Context pipeline
        ↓
Provider-visible projection
```

This is the right model.

Gestalt should continue to treat canonical session history as provider-neutral and durable. Only the provider-visible projection should change when switching provider/model.

### 2.5 Current cache behavior

Gestalt already has a concept of stable prompt prefix and provider cache planning.

Current behavior:

* Anthropic adapter can use explicit cache control.
* OpenAI adapter mostly relies on automatic provider-side prompt caching.
* The context pipeline can create stable prompt prefix snapshots.
* Tool schema arrays are part of the provider-visible prefix and should remain stable where possible.

This direction is correct and should be preserved.

---

## 3. Problems To Fix

### Problem 1 — `kind` is too broad

Current provider `kind` mixes multiple concerns:

```json
{
  "kind": "openai"
}
```

But “OpenAI” can mean different API formats:

```text
OpenAI Chat Completions
OpenAI Responses
OpenAI-compatible Chat Completions
```

These require different request serialization.

The schema needs to distinguish provider identity from API wire format.

---

### Problem 2 — OpenAI Responses API is not first-class

Current OpenAI support assumes `/chat/completions`.

Gestalt needs first-class support for:

```text
POST /responses
```

The Responses API should not be bolted onto the Chat Completions adapter with conditional hacks.

It should have its own adapter or format-specific serializer.

---

### Problem 3 — Model context window is not configurable per model

The context pipeline needs to know the selected model’s context window.

Today, runtime budgeting can fall back to a global default such as:

```text
120_000 tokens
```

This is unsafe because different models have very different context limits.

Model metadata should be part of resolved provider/model configuration.

---

### Problem 4 — Provider/model selection is too session-sticky

Gestalt should allow a session to continue even if the next run uses a different provider/model.

A session should not be tied permanently to one provider.

Correct model:

```text
Session = canonical context and history
Run = one execution attempt using a selected provider/model
```

Therefore:

```text
Same session, run 1: Anthropic Claude
Same session, run 2: OpenAI GPT via Responses API
Same session, run 3: OpenRouter model
```

All runs should use the same canonical session history unless explicitly forked or reset.

---

### Problem 5 — Prompt cache behavior should be provider-aware, not session-breaking

Switching provider/model may invalidate provider-side prompt cache, but it must not invalidate Gestalt’s session context.

Provider cache is an optimization.

Canonical history is the source of truth.

---

## 4. Goals

### 4.1 Schema goals

The new schema must support:

* Provider base URL
* API key source
* API wire format
* Optional endpoint paths
* Provider capabilities
* Provider-scoped model definitions
* Per-model context window
* Per-model output limit
* Per-model capabilities
* Model variants
* Provider/model profile defaults
* Run-level provider/model override

### 4.2 Runtime goals

The runtime must support:

* Switching model between runs in the same session
* Switching provider between runs in the same session
* Rebuilding provider-visible context projection for the selected model
* Re-budgeting context based on selected model context window
* Keeping canonical session history unchanged
* Preserving context across resumed sessions
* Keeping prompt cache strategy provider-aware

### 4.3 Architecture goals

The change should keep Gestalt simple:

* No provider-native wire types in `gestalt-runtime`
* No tool execution inside provider adapters
* No provider-specific context logic in session history
* No permanent coupling between session and provider
* Provider adapter owns wire serialization only
* Context pipeline owns projection and budgeting
* Runtime owns run orchestration
* Config resolver owns provider/model selection

---

## 5. Non-Goals

This feature does not need to solve:

* Dynamic provider model discovery as a mandatory runtime dependency
* Full pricing catalog refresh
* Automatic benchmark-based model routing
* Multi-provider parallel execution
* Multi-agent orchestration
* Provider failover
* Cross-provider semantic memory translation
* Vendor-side prompt cache portability

These can be added later.

---

## 6. Architectural Decisions

## AD-001 — Replace broad provider `kind` with explicit `api_format`

Current schema:

```json
{
  "kind": "openai"
}
```

Proposed schema:

```json
{
  "api_format": "openai_responses"
}
```

Supported values:

```text
anthropic_messages
openai_chat_completions
openai_responses
```

Optional future values:

```text
mistral_chat
google_generate_content
ollama_chat
```

### Backward compatibility

Since this is still greenfield, we can make a breaking schema improvement.

However, for convenience, the resolver may support temporary mapping:

| Old `kind`          | New `api_format`                    |
| ------------------- | ----------------------------------- |
| `anthropic`         | `anthropic_messages`                |
| `openai`            | `openai_chat_completions` initially |
| `openai-compatible` | `openai_chat_completions`           |

Recommended final direction:

```text
Deprecate kind for request serialization.
Use api_format as the source of truth.
```

If a provider family is still useful for display or built-in discovery, introduce a separate field:

```json
{
  "provider_family": "openai"
}
```

But provider behavior must come from:

```json
{
  "api_format": "openai_responses"
}
```

---

## AD-002 — Provider identity and API format are separate

A provider has an identity:

```text
openai
anthropic
openrouter
groq
ollama
together
```

A provider also has an API format:

```text
anthropic_messages
openai_chat_completions
openai_responses
```

Examples:

```json
{
  "providers": {
    "openai": {
      "api_format": "openai_responses",
      "base_url": "https://api.openai.com/v1"
    },
    "openrouter": {
      "api_format": "openai_chat_completions",
      "base_url": "https://openrouter.ai/api/v1"
    },
    "anthropic": {
      "api_format": "anthropic_messages",
      "base_url": "https://api.anthropic.com"
    }
  }
}
```

This makes it possible for multiple providers to share the same API format without pretending they are the same provider.

---

## AD-003 — Provider/model selection belongs to `RunConfig`, not `Session`

A session should not permanently own a provider/model.

A session owns:

```text
Session ID
Canonical message history
Checkpoints
Memory references
Workspace references
Context artifacts
Run history
```

A run owns:

```text
Provider ID
Model ID
Model variant
Resolved provider config snapshot
Resolved model config snapshot
Context projection used for this run
Provider request metadata
Usage/cost metadata
```

Proposed conceptual split:

```rust
pub struct Session {
    pub id: SessionId,
    pub history: Vec<SessionMessage>,
    pub checkpoints: Vec<CheckpointRef>,
    pub default_selection: Option<ModelSelection>,
    pub runs: Vec<RunSummary>,
}

pub struct RunConfig {
    pub session_id: SessionId,
    pub selection: ModelSelection,
    pub max_turns: usize,
    pub approval_mode: ApprovalMode,
}

pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
}
```

The session may have defaults, but every run should resolve its own provider/model selection.

---

## AD-004 — Canonical history is provider-neutral

Canonical history must never store provider-specific wire payloads.

Do not store OpenAI-specific or Anthropic-specific request structures as the canonical source of truth.

Correct:

```rust
Message::System
Message::User
Message::Assistant
Message::ToolResult
```

Incorrect:

```rust
OpenAiChatMessage
AnthropicMessageBlock
OpenAiResponseInputItem
```

Provider-specific serialization must happen at the adapter boundary only.

---

## AD-005 — Context projection is rebuilt per run

Each run builds a fresh provider-visible projection from canonical history.

This is required because different models may have different context windows.

Example:

```text
Run 1:
  provider = anthropic
  model = claude-x
  context window = 200k

Run 2:
  provider = openai
  model = gpt-y
  context window = 128k
```

The second run should not reset the session.

Instead, it should:

1. Load canonical history.
2. Resolve selected model metadata.
3. Calculate token budget.
4. Build a provider-neutral context packet.
5. Compact or clear eligible old tool results if needed.
6. Serialize the projection using the selected provider adapter.

---

## AD-006 — Prompt cache is an optimization, not session state

Prompt cache should be treated as provider-specific and disposable.

Switching provider/model may lose vendor-side cache benefits, but must not lose conversation state.

Gestalt may keep internal cache metadata:

```rust
PromptPrefixHash
ToolSchemaHash
PromptSnapshotId
```

But provider-side cache identity should be scoped by:

```text
provider_id
api_format
model_id
tool_schema_hash
prompt_prefix_hash
```

Example:

```rust
pub struct ProviderCacheKey {
    pub provider_id: String,
    pub api_format: ApiFormat,
    pub model_id: String,
    pub prompt_prefix_hash: String,
    pub tool_schema_hash: String,
}
```

This avoids pretending that Anthropic and OpenAI caches are portable.

---

## AD-007 — Model context window drives context budgeting

Runtime should resolve context budget from the selected model.

Resolution order:

```text
1. Explicit run-level context override
2. Explicit profile-level context override
3. Provider-scoped model max_context_tokens
4. Built-in model catalog max_context_tokens
5. Provider default max_context_tokens
6. Conservative fallback
```

Recommended fallback:

```text
32_000 tokens
```

Avoid large optimistic defaults such as `120_000` unless the selected model metadata supports it.

---

## AD-008 — Keep provider adapters thin

Provider adapters should only handle:

* Endpoint selection
* Request serialization
* Streaming event parsing
* Provider-specific tool schema rendering
* Provider-specific cache metadata rendering
* Provider-specific error normalization

Provider adapters should not:

* Mutate session history
* Run tools
* Decide policy
* Compact history
* Manage canonical memory
* Own retry strategy beyond transport-level details

---

# 7. Proposed Config Schema

## 7.1 Full example

```json
{
  "version": 1,

  "defaults": {
    "profile": "default"
  },

  "profiles": {
    "default": {
      "provider": "openai",
      "model": "gpt-5.1",
      "variant": "medium"
    },

    "research": {
      "provider": "anthropic",
      "model": "claude-sonnet-4.5",
      "variant": "high"
    },

    "cheap": {
      "provider": "openrouter",
      "model": "openrouter/free",
      "variant": "low"
    }
  },

  "providers": {
    "openai": {
      "display_name": "OpenAI",
      "api_format": "openai_responses",
      "base_url": "https://api.openai.com/v1",
      "api_key_env": "OPENAI_API_KEY",
      "default_model": "gpt-5.1",

      "request": {
        "timeout_ms": 120000,
        "stream_chunk_timeout_ms": 30000
      },

      "capabilities": {
        "streaming": true,
        "tools": true,
        "vision": true,
        "json_mode": true,
        "prompt_cache": "automatic"
      },

      "models": {
        "gpt-5.1": {
          "display_name": "GPT-5.1",
          "max_context_tokens": 400000,
          "max_output_tokens": 32768,

          "capabilities": {
            "streaming": true,
            "tools": true,
            "vision": true,
            "json_mode": true,
            "reasoning": true
          },

          "options": {
            "text_verbosity": "medium"
          },

          "variants": {
            "low": {
              "options": {
                "reasoning_effort": "low"
              }
            },
            "medium": {
              "options": {
                "reasoning_effort": "medium"
              }
            },
            "high": {
              "options": {
                "reasoning_effort": "high"
              }
            },
            "xhigh": {
              "options": {
                "reasoning_effort": "xhigh"
              }
            }
          }
        }
      }
    },

    "anthropic": {
      "display_name": "Anthropic",
      "api_format": "anthropic_messages",
      "base_url": "https://api.anthropic.com",
      "api_key_env": "ANTHROPIC_API_KEY",
      "default_model": "claude-sonnet-4.5",

      "request": {
        "timeout_ms": 120000,
        "stream_chunk_timeout_ms": 30000
      },

      "capabilities": {
        "streaming": true,
        "tools": true,
        "vision": true,
        "prompt_cache": "explicit"
      },

      "models": {
        "claude-sonnet-4.5": {
          "display_name": "Claude Sonnet 4.5",
          "max_context_tokens": 200000,
          "max_output_tokens": 8192,

          "capabilities": {
            "streaming": true,
            "tools": true,
            "vision": true,
            "reasoning": true
          },

          "options": {
            "thinking": {
              "enabled": true,
              "budget_tokens": 4096
            }
          },

          "variants": {
            "low": {
              "options": {
                "thinking": {
                  "enabled": true,
                  "budget_tokens": 1024
                }
              }
            },
            "medium": {
              "options": {
                "thinking": {
                  "enabled": true,
                  "budget_tokens": 4096
                }
              }
            },
            "high": {
              "options": {
                "thinking": {
                  "enabled": true,
                  "budget_tokens": 8192
                }
              }
            }
          }
        }
      }
    },

    "openrouter": {
      "display_name": "OpenRouter",
      "api_format": "openai_chat_completions",
      "base_url": "https://openrouter.ai/api/v1",
      "api_key_env": "OPENROUTER_API_KEY",
      "default_model": "openrouter/free",

      "headers": {
        "HTTP-Referer": "${env:GESTALT_OPENROUTER_REFERER}",
        "X-Title": "Gestalt"
      },

      "capabilities": {
        "streaming": true,
        "tools": true,
        "prompt_cache": "provider_dependent"
      },

      "models": {
        "openrouter/free": {
          "display_name": "OpenRouter Free Model",
          "max_context_tokens": 32768,
          "max_output_tokens": 4096,

          "variants": {
            "low": {
              "options": {
                "temperature": 0.2
              }
            }
          }
        }
      }
    }
  },

  "context": {
    "reserved_output_tokens": 8192,
    "safety_margin_tokens": 2048,
    "tool_result_retention": "auto",
    "compaction": {
      "enabled": true,
      "strategy": "checkpoint_oldest_completed_ranges"
    }
  }
}
```

---

# 8. Proposed Rust Types

## 8.1 API format enum

Add a new enum.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}
```

Current `ProviderKind` can either be removed or narrowed.

Recommended greenfield simplification:

```rust
// Replace ProviderKind with ApiFormat for request serialization.
pub type ProviderKind = ApiFormat;
```

Better long-term version:

```rust
pub enum ProviderFamily {
    OpenAi,
    Anthropic,
    OpenRouter,
    Groq,
    Together,
    Ollama,
    Custom,
}
```

But this should only be used for catalog/display/discovery, not request serialization.

---

## 8.2 Provider config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub display_name: Option<String>,

    pub api_format: ApiFormat,

    pub base_url: String,

    pub api_key_env: Option<String>,

    pub auth_ref: Option<String>,

    pub default_model: Option<String>,

    pub models_endpoint: Option<String>,

    pub headers: BTreeMap<String, String>,

    pub request: ProviderRequestConfig,

    pub capabilities: ProviderCapabilitiesConfig,

    pub models: BTreeMap<String, ModelDefinitionConfig>,
}
```

Remove or deprecate:

```rust
kind
protocol
```

Unless `protocol` is explicitly needed for transport-level differences. If kept, it should not control provider wire format.

---

## 8.3 Model config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDefinitionConfig {
    pub display_name: Option<String>,

    pub max_context_tokens: Option<u32>,

    pub max_output_tokens: Option<u32>,

    pub capabilities: ModelCapabilitiesConfig,

    pub options: ModelOptionsConfig,

    pub variants: BTreeMap<String, ModelVariantConfig>,
}
```

---

## 8.4 Model capabilities

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilitiesConfig {
    pub streaming: Option<bool>,
    pub tools: Option<bool>,
    pub vision: Option<bool>,
    pub json_mode: Option<bool>,
    pub reasoning: Option<bool>,
    pub prompt_cache: Option<PromptCacheMode>,
}
```

---

## 8.5 Prompt cache mode

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheMode {
    None,
    Automatic,
    Explicit,
    ProviderDependent,
}
```

Examples:

```text
OpenAI: automatic
Anthropic: explicit
OpenRouter: provider_dependent
Local Ollama: none
```

---

## 8.6 Model options

Keep existing model options, but validate them against `api_format`.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelOptionsConfig {
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,

    pub reasoning_effort: Option<String>,
    pub text_verbosity: Option<String>,

    pub thinking: Option<ThinkingConfig>,

    pub adapter_options: BTreeMap<String, serde_json::Value>,
}
```

Validation examples:

| Option             |   Anthropic Messages |             OpenAI Chat |     OpenAI Responses |
| ------------------ | -------------------: | ----------------------: | -------------------: |
| `temperature`      |                  Yes |                     Yes |                  Yes |
| `top_p`            |                  Yes |                     Yes |                  Yes |
| `thinking`         |                  Yes |                      No |                   No |
| `reasoning_effort` |                   No | Limited/model-dependent |                  Yes |
| `text_verbosity`   |                   No |                      No |                  Yes |
| `adapter_options`  | Allowed with warning |    Allowed with warning | Allowed with warning |

---

## 8.7 Resolved provider/model

Add an explicit resolved shape.

```rust
pub struct ResolvedModelProvider {
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,

    pub api_format: ApiFormat,
    pub base_url: String,
    pub auth: ResolvedAuth,
    pub headers: BTreeMap<String, String>,

    pub model: ResolvedModel,

    pub request: ProviderRequestConfig,
    pub capabilities: ResolvedCapabilities,
}
```

```rust
pub struct ResolvedModel {
    pub id: String,
    pub display_name: Option<String>,
    pub max_context_tokens: u32,
    pub max_output_tokens: u32,
    pub capabilities: ModelCapabilitiesConfig,
    pub options: ModelOptionsConfig,
}
```

This should be the single object passed into runtime/provider construction.

---

# 9. Runtime Design: Provider/Model Switching

## 9.1 Required behavior

Gestalt must support this:

```bash
gestalt run --session abc --provider anthropic --model claude-sonnet-4.5
gestalt run --session abc --provider openai --model gpt-5.1
gestalt run --session abc --provider openrouter --model openrouter/free
```

The session should retain context across these runs.

Switching provider/model should not imply:

```text
new session
history reset
memory reset
checkpoint reset
tool trace reset
```

It should only imply:

```text
new run
new resolved provider/model snapshot
new context projection
new provider request serialization
```

---

## 9.2 Session structure

Recommended shape:

```rust
pub struct Session {
    pub id: SessionId,

    pub history: Vec<SessionMessage>,

    pub checkpoints: Vec<ContextCheckpoint>,

    pub prompt_snapshots: Vec<PromptSnapshotRef>,

    pub runs: Vec<RunSummary>,

    pub defaults: Option<ModelSelection>,
}
```

---

## 9.3 Run structure

```rust
pub struct Run {
    pub id: RunId,

    pub session_id: SessionId,

    pub selection: ModelSelection,

    pub resolved_provider_model: ResolvedModelProvider,

    pub context_projection: ContextProjectionRef,

    pub started_at: DateTime<Utc>,

    pub stopped_at: Option<DateTime<Utc>>,

    pub stop_reason: Option<StopReason>,
}
```

---

## 9.4 Model selection

```rust
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
}
```

Resolution order:

```text
1. CLI flags
2. Run request
3. Profile
4. Session default
5. Workspace default
6. Global default
7. Built-in default
```

Important:

```text
Session default is only a default.
It must not lock the session to a provider/model.
```

---

## 9.5 Context projection flow

Current conceptual flow should become:

```text
Load Session
  ↓
Resolve ModelSelection for this Run
  ↓
Resolve ProviderConfig + ModelDefinitionConfig
  ↓
Create TokenBudget from selected model
  ↓
ContextPipeline builds provider-neutral ContextPacket
  ↓
Provider adapter serializes ContextPacket to provider wire format
  ↓
Provider streams response
  ↓
Runtime accumulates assistant turn
  ↓
Tool calls are validated/policy-checked/executed
  ↓
Canonical session history is appended
  ↓
Run summary is stored
```

---

## 9.6 Token budget creation

Replace global-default-first behavior with selected-model-first behavior.

```rust
pub fn build_token_budget(
    resolved: &ResolvedModelProvider,
    context_config: &ContextConfig,
    run_override: Option<ContextWindowOverride>,
) -> TokenBudget {
    let model_limit = run_override
        .or(context_config.max_context_window)
        .unwrap_or(resolved.model.max_context_tokens);

    TokenBudget {
        model_limit,
        reserved_output: context_config
            .reserved_output_tokens
            .unwrap_or(resolved.model.max_output_tokens.min(8192)),
        safety_margin: context_config
            .safety_margin_tokens
            .unwrap_or(2048),
    }
}
```

---

# 10. Provider Adapter Design

## 10.1 Adapter selection

Provider adapter should be selected by `api_format`.

```rust
pub fn build_provider(resolved: ResolvedModelProvider) -> Box<dyn Provider> {
    match resolved.api_format {
        ApiFormat::AnthropicMessages => Box::new(AnthropicMessagesProvider::new(resolved)),
        ApiFormat::OpenAiChatCompletions => Box::new(OpenAiChatCompletionsProvider::new(resolved)),
        ApiFormat::OpenAiResponses => Box::new(OpenAiResponsesProvider::new(resolved)),
    }
}
```

---

## 10.2 Anthropic Messages provider

Endpoint:

```text
POST {base_url}/v1/messages
```

Responsibilities:

* Serialize system blocks
* Serialize messages
* Serialize tools
* Serialize `thinking` if supported
* Serialize Anthropic cache control
* Parse streaming message deltas
* Normalize events into Gestalt provider events

---

## 10.3 OpenAI Chat Completions provider

Endpoint:

```text
POST {base_url}/chat/completions
```

Responsibilities:

* Serialize `messages`
* Serialize Chat Completions tool schema
* Serialize `stream`
* Serialize `stream_options`
* Parse streaming chat deltas
* Normalize events into Gestalt provider events

This adapter should be used for:

```text
OpenRouter
Groq
Together
Ollama OpenAI-compatible mode
legacy OpenAI chat-compatible models
```

---

## 10.4 OpenAI Responses provider

Endpoint:

```text
POST {base_url}/responses
```

Responsibilities:

* Serialize `instructions`
* Serialize `input`
* Serialize Responses API tools
* Serialize reasoning options
* Serialize text verbosity options
* Parse Responses streaming events
* Normalize output text, reasoning, and tool calls into Gestalt provider events

This should be separate from the Chat Completions adapter.

Do not overload `OpenAiProvider` with both Chat and Responses behavior unless the implementation remains cleanly split by serializer modules.

Acceptable structure:

```text
openai/
  mod.rs
  chat_completions.rs
  responses.rs
  common.rs
```

---

# 11. Context & Cache Strategy

## 11.1 Stable prompt prefix

Gestalt should continue building stable prefix segments:

```text
default system prompt
workspace.md
memory.md
selected skills
tool schema
```

These should be ordered deterministically.

---

## 11.2 Tool schemas as cache-sensitive prefix

Most providers include tool definitions as part of the prompt/cache prefix.

Therefore:

* Tool ordering must be deterministic.
* Tool schema rendering must be stable.
* Tool availability should not fluctuate unnecessarily between turns.
* Provider-specific schema rendering should happen after provider selection, but from a stable canonical tool catalog.

Recommended hashes:

```rust
PromptPrefixHash
CanonicalToolCatalogHash
ProviderToolSchemaHash
ProviderCacheKey
```

---

## 11.3 Anthropic cache behavior

For Anthropic:

```text
tools → system → messages
```

Cache control should be applied according to the adapter’s provider-specific rules.

The context pipeline should not hard-code Anthropic cache syntax.

It should emit a provider-neutral cache plan:

```rust
PromptCachePlan {
    stable_prefix_segments,
    suggested_breakpoints,
}
```

The Anthropic adapter renders that into Anthropic-specific `cache_control`.

---

## 11.4 OpenAI cache behavior

For OpenAI:

* Prompt caching is automatic.
* Exact prefix matching matters.
* Stable ordering matters.
* Tool schema consistency matters.

The OpenAI adapters should not emit Anthropic-style cache controls.

Optional OpenAI-specific config:

```json
{
  "adapter_options": {
    "prompt_cache_key": "workspace-default",
    "prompt_cache_retention": "24h"
  }
}
```

This should remain adapter-specific.

---

## 11.5 Switching provider/model and cache

When switching from one provider/model to another:

```text
Canonical session history remains.
Context projection is rebuilt.
Provider-side cache may be cold.
Gestalt internal prefix hash remains useful.
Provider cache key changes.
```

This is expected.

Do not block provider/model switching because cache cannot transfer.

---

# 12. Config Merge Semantics

Current config merge appears to replace provider/profile entries too aggressively.

Fix this while touching schema.

Required merge behavior:

```text
Primitive fields: override
Maps: recursive merge
Arrays: replace unless explicitly documented otherwise
Provider models: recursive merge by model ID
Model variants: recursive merge by variant ID
```

Example:

Global config:

```json
{
  "providers": {
    "openai": {
      "api_format": "openai_responses",
      "base_url": "https://api.openai.com/v1",
      "api_key_env": "OPENAI_API_KEY",
      "models": {
        "gpt-5.1": {
          "max_context_tokens": 400000
        }
      }
    }
  }
}
```

Workspace config:

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

Resolved config should preserve:

```text
base_url
api_key_env
api_format
model max_context_tokens
new cheap variant
```

It should not replace the entire `openai` provider.

---

# 13. Validation Rules

Add a validation pass after config resolution and before runtime starts.

## 13.1 Provider validation

Validate:

```text
provider exists
api_format is known
base_url is present
auth source exists unless provider allows no auth
default_model exists if referenced
request timeout is valid
headers interpolate successfully
```

---

## 13.2 Model validation

Validate:

```text
model exists under provider or built-in catalog
max_context_tokens is known
max_output_tokens is known
variant exists if selected
variant options are compatible with model/provider
```

---

## 13.3 API format validation

Validate option compatibility.

Examples:

```text
thinking is only valid for anthropic_messages unless adapter explicitly supports it
text_verbosity is only valid for openai_responses
reasoning_effort is only valid for supported OpenAI formats/models
json_mode requires model/provider support
tools require provider/model tool support
```

Unsupported options should produce:

```text
Error for strict mode
Warning for permissive mode
```

Recommended default:

```text
Error for known incompatible options.
Warning for unknown adapter_options.
```

---

# 14. CLI Behavior

## 14.1 Run with explicit provider/model

```bash
gestalt run --provider openai --model gpt-5.1
```

Creates a new session if no session is active.

---

## 14.2 Continue existing session with different model

```bash
gestalt run --session abc --provider anthropic --model claude-sonnet-4.5
gestalt run --session abc --provider openai --model gpt-5.1
```

Expected behavior:

```text
Same session history.
New run.
New provider/model projection.
No context reset.
```

---

## 14.3 Set session default model

Optional command:

```bash
gestalt session set-default-model abc --provider openai --model gpt-5.1
```

This changes future run defaults only.

It must not rewrite past runs.

---

## 14.4 Show session runs

```bash
gestalt session runs abc
```

Should show:

```text
Run ID
Provider
Model
Variant
Started
Stopped
Stop reason
Input tokens
Output tokens
Context projection ID
```

---

## 14.5 Doctor command

Extend doctor output:

```bash
gestalt doctor providers
```

Should validate:

```text
Provider config
Auth availability
API format support
Default model metadata
Context window
Prompt cache mode
Known incompatible options
```

---

# 15. Trace & Replay

## 15.1 Run trace must include resolved provider/model snapshot

Each run trace should include:

```json
{
  "event": "RunStarted",
  "session_id": "abc",
  "run_id": "run_123",
  "provider": "openai",
  "model": "gpt-5.1",
  "variant": "medium",
  "api_format": "openai_responses",
  "max_context_tokens": 400000,
  "max_output_tokens": 32768
}
```

---

## 15.2 Context projection trace

Each run should trace:

```json
{
  "event": "ContextProjected",
  "session_id": "abc",
  "run_id": "run_123",
  "model_limit": 400000,
  "reserved_output_tokens": 8192,
  "safety_margin_tokens": 2048,
  "canonical_message_count": 128,
  "projected_message_count": 91,
  "compaction_applied": true,
  "tool_results_cleared": 12,
  "prompt_prefix_hash": "...",
  "tool_schema_hash": "..."
}
```

---

## 15.3 Replay behavior

Replay should use the resolved provider/model snapshot from the original run unless explicitly overridden.

Default:

```bash
gestalt replay run_123
```

Uses original provider/model snapshot.

Override:

```bash
gestalt replay run_123 --provider anthropic --model claude-sonnet-4.5
```

Allowed, but should be marked as non-identical replay:

```text
Replay mode: remapped provider/model
Determinism: not guaranteed
```

---

# 16. Migration Plan

Since Gestalt is still greenfield, prefer clean schema over excessive backward compatibility.

## Step 1 — Add new schema types

Add:

```text
ApiFormat
PromptCacheMode
ModelCapabilitiesConfig
ResolvedModelProvider
ResolvedModel
ModelSelection
```

---

## Step 2 — Replace provider `kind` usage

Search for current `kind` usage.

Replace behavior-driving use with:

```text
api_format
```

Temporary compatibility mapping is acceptable.

---

## Step 3 — Extend model definitions

Add:

```text
max_context_tokens
max_output_tokens
capabilities
```

to provider-scoped model config.

---

## Step 4 — Refactor provider construction

Change provider construction from:

```text
match kind
```

to:

```text
match api_format
```

Add dedicated support for:

```text
OpenAiChatCompletionsProvider
OpenAiResponsesProvider
AnthropicMessagesProvider
```

---

## Step 5 — Update runtime selection

Move provider/model selection into run config.

Ensure session can be reused with different selected models.

---

## Step 6 — Update context budget resolution

Make selected model metadata the primary source for context window.

---

## Step 7 — Update prompt cache keying

Make provider cache key include:

```text
provider_id
api_format
model_id
prompt_prefix_hash
tool_schema_hash
```

---

## Step 8 — Add validation

Add config validation before runtime starts.

---

## Step 9 — Update tests

Add schema, runtime, adapter, and context projection tests.

---

# 17. Test Plan

## 17.1 Schema parsing tests

Test:

```text
parse_provider_with_openai_responses
parse_provider_with_openai_chat_completions
parse_provider_with_anthropic_messages
parse_provider_model_context_window
parse_model_variants
reject_unknown_api_format
reject_missing_base_url
reject_missing_model_context_window_when_no_catalog_fallback
```

---

## 17.2 Config merge tests

Test:

```text
workspace_provider_models_merge_without_replacing_global_provider
workspace_variant_merge_preserves_base_url_and_auth
profile_override_changes_model_only
provider_override_changes_api_format
```

---

## 17.3 Provider adapter tests

Test:

```text
openai_chat_uses_chat_completions_endpoint
openai_responses_uses_responses_endpoint
anthropic_uses_messages_endpoint
openrouter_uses_openai_chat_completions_format
responses_serializes_reasoning_effort
responses_serializes_text_verbosity
anthropic_serializes_thinking
chat_rejects_text_verbosity
```

---

## 17.4 Runtime switching tests

Test:

```text
same_session_can_run_with_anthropic_then_openai
same_session_can_switch_model_without_history_reset
switching_to_smaller_context_triggers_projection_compaction
switching_to_larger_context_keeps_more_history
session_default_model_can_change_without_rewriting_past_runs
run_trace_records_resolved_provider_model_snapshot
```

---

## 17.5 Context cache tests

Test:

```text
prompt_prefix_hash_stable_across_runs_same_workspace
provider_cache_key_changes_when_provider_changes
provider_cache_key_changes_when_api_format_changes
provider_cache_key_changes_when_tool_schema_changes
openai_adapter_does_not_emit_anthropic_cache_control
anthropic_adapter_emits_cache_control_from_cache_plan
```

---

# 18. Acceptance Criteria

This feature is complete when:

* Provider config uses `api_format` as the source of truth for request serialization.
* OpenAI Chat Completions and OpenAI Responses are separate API formats.
* Anthropic Messages remains supported.
* Provider-scoped model config supports `max_context_tokens`.
* Runtime context budget resolves from selected model metadata.
* A session can run with one provider/model, then continue with another provider/model.
* Canonical session history is not reset when switching provider/model.
* Resumed sessions can use a different provider/model from earlier runs.
* Prompt cache strategy remains provider-aware.
* Provider adapter selection is simple and based on `api_format`.
* Config validation catches incompatible model/provider options.
* Run traces include resolved provider/model snapshots.
* Tests cover provider/model switching, schema parsing, adapter routing, and context budgeting.

---

# 19. Example End State

A user can configure:

```json
{
  "profiles": {
    "default": {
      "provider": "anthropic",
      "model": "claude-sonnet-4.5"
    },
    "openai-heavy": {
      "provider": "openai",
      "model": "gpt-5.1",
      "variant": "high"
    }
  }
}
```

Then run:

```bash
gestalt run --profile default --session project-alpha
gestalt run --profile openai-heavy --session project-alpha
```

Expected behavior:

```text
The second run continues the same session.
The provider changes.
The model changes.
The context projection is rebuilt.
The token budget changes.
The canonical history remains intact.
The provider cache may be cold.
The session is not reset.
```

---

# 20. Final Architectural Position

The net-positive simplification is:

```text
Provider config defines how to talk to an API.
Model config defines what the selected model can handle.
Run config chooses provider/model for this execution.
Session stores provider-neutral canonical context.
Context pipeline projects canonical context into the selected model budget.
Provider adapter serializes the projection into the selected API format.
```

This keeps Gestalt clean, provider-neutral, and ready for future provider growth without turning the runtime into provider-specific glue code.
