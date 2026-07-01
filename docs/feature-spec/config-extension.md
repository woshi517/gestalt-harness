---
title: "feat: Gestalt Configuration and Extension Protocol Refinement"
date: 2026-06-13
status: superseded-in-part
type: feat
depth: deep
target: v0.1
owners:
  - gestalt-app
  - gestalt-runtime
  - gestalt-cli
---

# feat: Gestalt Configuration and Extension Protocol Refinement

> [!IMPORTANT]
> [ADR-031](../adrs/ADR-031-v0-1-greenfield-compatibility-cutoff.md)
> supersedes this document's pre-hardening compatibility and migration
> requirements. Legacy harness TOML, deprecated config aliases, extension
> manifest/protocol V1, and deprecated Rust compatibility APIs are removed
> before stable v0.1. Remaining configuration and protocol hardening proposals
> apply only where they are consistent with ADR-031.

## Summary

Refine Gestalt's existing `gestalt.json` configuration system and process-backed extension protocol into a stable, versioned, ergonomic v0.1 contract.

This feature does **not** replace the current architecture. The existing top-level configuration domains already cover the essential responsibilities of an agent harness:

- provider and model selection;
- reusable execution profiles;
- prompt and context construction;
- tool execution limits;
- filesystem, shell, and network policy;
- MCP servers;
- skills;
- process-backed extensions;
- observability;
- TUI-specific preferences.

The work focuses on four improvements:

1. tighten the existing schema so invalid or ambiguous configuration fails early;
2. define deterministic layering, merging, interpolation, and resolution semantics;
3. add provider-neutral model variants for reasoning and model-specific request options;
4. harden and version the existing JSON-RPC extension protocol without bloating `AgentLoop` or replacing the runtime composition layer.

The intended result is a configuration surface that is easy for ordinary users to start with, expressive enough for power users and product builders, and stable enough for third-party extension authors.

---

## Background

Gestalt currently loads a global and/or workspace-level `gestalt.json` into `WorkspaceConfig`. The schema includes:

defaults  
profiles  
providers  
prompt  
context  
tools  
policies  
mcp  
skills  
extensions  
observe  
tui

Extensions are external processes described by `gestalt.extension.toml`. They communicate with `gestalt-runtime` through JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout and can contribute:

- tools;
- composition hooks;
- context injectors.

The architecture is directionally correct:

- `gestalt-core` remains unaware of configuration files and extension processes;
- provider-native formats remain inside provider adapters;
- process-backed capabilities register through `gestalt-runtime`;
- policy and approval still gate real tool execution;
- runtime and extension lifecycle events remain observable;
- extension failures do not require extension-specific branches in `AgentLoop`.

The problem is therefore not missing architecture. The problem is that the current public contracts are still underspecified in several areas:

- too many closed values are represented as unrestricted strings;
- config merge and `null` semantics are unclear;
- provider, profile, and model resolution overlaps;
- model reasoning variants are not represented explicitly;
- generic and provider-specific request options lack a stable boundary;
- several tool and RPC resource limits are absent;
- the extension handshake does not perform meaningful protocol negotiation;
- malformed RPC output can be silently discarded;
- timeout, cancellation, concurrency, and large-message behavior are incomplete;
- extension hook conflict and failure semantics are not fully contractual;
- trust is based too heavily on discovery location or extension ID.

---

## Reference Architecture: Lessons from OpenCode

OpenCode's configuration system is a useful ergonomic reference, but Gestalt should adopt only the parts that reinforce its harness boundaries.

### Lessons to adopt

#### 1. One human-editable JSON configuration contract

Gestalt should keep `gestalt.json` as its canonical configuration file. A machine-readable schema should continue to drive editor autocomplete and validation.

A normal configuration must remain sparse:

```json
{  
  "$schema": "https://gestalt.noentic.com/schema/gestalt.json",  
  "version": 1,  
  "defaults": {  
    "profile": "default"  
  },  
  "profiles": {  
    "default": {  
      "provider": "openrouter",  
      "model": "openrouter/free"  
    }  
  }  
}
```

The full generated schema may be broad, but `gestalt init` must not emit every optional field.

#### 2. Layered configuration is merged, not wholesale replaced

OpenCode merges non-conflicting keys across configuration locations and lets later sources override conflicting keys. Gestalt should formalize the same general behavior, with additional security-aware rules for policies, credentials, extensions, and permissions.

#### 3. Provider-local model definitions and model variants

OpenCode allows a provider entry to define per-model options and named variants. This is a good fit for Gestalt because providers expose different controls for reasoning effort, thinking mode, verbosity, caching, and other request features.

Gestalt should add a provider-neutral variant abstraction while preserving provider-specific serialization inside adapters.

#### 4. Explicit enablement and timeout controls for external capabilities

OpenCode's MCP configuration exposes `enabled` and timeout fields directly on each server. Gestalt should make equivalent lifecycle controls explicit for MCP servers and process extensions.

#### 5. Variable substitution

Environment references are useful for non-secret settings and auth references. File interpolation is useful for large instructions and certificates, but unrestricted file interpolation into arbitrary config fields complicates policy and secret handling.

Gestalt should support a narrow interpolation contract rather than arbitrary textual templating.

#### 6. An `experimental` namespace

Unstable options should not be mixed into the stable root schema. Features whose behavior may change before v0.2 should live under:

```json
{  
  "experimental": {}  
}
```

The runtime must warn that these keys are not covered by normal compatibility guarantees.

### Lessons not to copy directly

Gestalt should not copy OpenCode's entire product configuration surface. In particular:

- agents, commands, themes, keybindings, formatters, and LSP configuration are product-specific rather than universal harness concerns;
- arbitrary provider option bags must not leak into `gestalt-core`;
- project configuration must not be able to silently weaken global or managed security policy;
- credentials must remain separated from normal configuration;
- extension processes require stronger protocol and trust guarantees than ordinary in-process plugins.

---

## Goals

1. Keep `gestalt.json` as the canonical user-facing configuration format.
2. Preserve the current top-level schema unless a change resolves a clear responsibility problem.
3. Make configuration deterministic, inspectable, and safe to merge across scopes.
4. Reject unknown fields, invalid enum values, unresolved references, and conflicting declarations before runtime execution.
5. Support named model variants such as `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, and user-defined variants.
6. Keep model variants provider-neutral at the runtime boundary and provider-specific at adapter serialization.
7. Preserve the current process-backed extension architecture.
8. Add extension protocol version negotiation and typed lifecycle behavior.
9. Prevent malformed or oversized extension messages from becoming silent hangs or memory hazards.
10. Make extension capabilities observable and diagnosable through the existing runtime event system.
11. Maintain backward compatibility for valid v0.1-era configuration wherever possible.
12. Keep `gestalt-core` I/O-free and keep extension/configuration concerns outside `AgentLoop`.

---

## Non-Goals

This feature will not:

- turn `gestalt.json` into a workflow or multi-agent topology language;
- configure domain-specific agent behavior;
- move extension execution into `gestalt-core`;
- allow extensions to bypass `ToolCatalog`, policy, approval, trace, or context trust boundaries;
- add a public extension marketplace;
- implement signed extension packages;
- provide an OS-level sandbox;
- support remote extension transports in v0.1;
- expose every OpenAI, Anthropic, Google, or OpenAI-compatible request field in the stable root schema;
- make arbitrary JSON values flow unvalidated through the core loop;
- redesign MCP itself;
- replace skills with extensions or merge both concepts into a generic plugin system.

---

## Architectural Decisions

### AD-1: Retain the current top-level configuration domains

The stable v0.1 root remains:

```json
{  
  "$schema": "...",  
  "version": 1,  
  "defaults": {},  
  "profiles": {},  
  "providers": {},  
  "prompt": {},  
  "context": {},  
  "tools": {},  
  "policies": {},  
  "mcp": {},  
  "skills": {},  
  "extensions": {},  
  "observe": {},  
  "tui": {},  
  "experimental": {}  
}
```

No new root-level `agent`, `runtime`, `execution`, `security`, `memory`, `routing`, or `orchestration` objects are introduced in v0.1.

### AD-2: Configuration is sparse by default

All sections except `version` remain optional.

`gestalt init` generates only:

- `$schema`;
- `version`;
- a default profile;
- a provider connection when one has been configured;
- conservative execution defaults where useful.

A full example is available through documentation or:

gestalt config template --full

### AD-3: `version` is mandatory and exact

For the v0.1 schema:

```json
"version": {  
  "type": "integer",  
  "const": 1  
}
```

The root requires `version`.

Config versioning is independent from:

- extension manifest schema version;
- extension RPC protocol version;
- trace format version;
- model catalog version.

### AD-4: Stable fields are typed; unstable behavior is quarantined

Closed value sets use enums. Arbitrary strings are not accepted for:

- execution mode;
- policy action;
- prompt assembly strategy;
- MCP lifecycle mode;
- provider protocol where applicable;
- sandbox type;
- log format;
- trust classification where user-configurable.

Unstable options live under `experimental`.

### AD-5: Security configuration is monotonic across scopes

A higher-precedence workspace config may override ordinary behavior but must not silently widen authority granted by a lower or managed layer.

Examples:

- workspace config may narrow allowed write paths; 
- workspace config may add denied paths;
- workspace config may not remove a managed deny rule;
- workspace config may request an extension;
- workspace config may not automatically trust that extension;
- workspace config may reference credentials;
- workspace config may not define raw managed secrets.

---

## Configuration Sources and Precedence

Gestalt resolves configuration in the following order, from lowest to highest behavioral precedence:

1. built-in defaults;
2. global config;
3. optional explicit config path;
4. workspace config;
5. environment overrides;
6. CLI flags;
7. active session overrides;
8. managed policy overlays.

Recommended paths:

```
Global:     ~/.config/gestalt/gestalt.json  
Workspace:  <workspace-root>/gestalt.json  
Explicit:   GESTALT_CONFIG=/path/to/gestalt.json  
Managed:    platform-specific administrator-controlled location
```

Managed policy overlays are a future-compatible boundary. The v0.1 implementation may define the type and merge rules without shipping enterprise management discovery.

### Merge semantics

#### Scalars

- missing field: inherit;
- present non-null value: replace;
- explicit `null`: allowed only on fields whose schema documents reset semantics.

#### Objects

- merge recursively by key;
- later scalar conflicts replace earlier values;
- unknown fields are errors.

#### Maps

Examples include `providers`, `profiles`, and `mcp.servers`.

- merge by map key;
- nested entries merge recursively;
- an explicit `enabled: false` disables an inherited entry;
- deletion by `null` is not supported in v0.1 unless a field explicitly defines it.

#### Arrays

Arrays replace earlier arrays by default.

They do not concatenate implicitly because concatenation is dangerous for:

- policy allow/deny sets;
- active skills;
- trusted extensions;
- header-like ordered values.

Future append/remove operators may be added under a new config version, not through hidden special cases.

#### Security overlays

Policy merge behavior is domain-specific:

deny rules: union  
allow rules: may be narrowed by higher layers  
managed deny: cannot be overridden  
managed extension/provider restrictions: cannot be overridden

### Effective configuration diagnostics

The following commands are required:

```bash
gestalt config validate  
gestalt config show  
gestalt config explain <json-path>  
gestalt config paths
```


`config show` prints the effective configuration with secrets and sensitive resolved values redacted.

`config explain` includes provenance:

```
defaults.profile = "default"  
source: workspace:/repo/gestalt.json  
​  
profiles.default.model = "openrouter/openai/gpt-5.5"  
source: global:~/.config/gestalt/gestalt.json  
​  
policies.paths.deny_read += ".env"  
source: built-in security defaults
```


---

## Interpolation and Secret References

### Environment references

Gestalt may support narrow string interpolation:

```json
{  
  "providers": {  
    "gateway": {  
      "base_url": "{env:COMPANY_LLM_BASE_URL}",  
      "api_key_env": "COMPANY_LLM_API_KEY"  
    }  
  }  
}
```


Rules:

- unresolved required environment references are validation errors;
- interpolation is performed before typed deserialization of the affected field;
- interpolation does not recursively evaluate produced strings;
- interpolated values are redacted in diagnostics when the field is classified as sensitive.

### File references

Do not support unrestricted `{file:...}` substitution across every string field in v0.1.

Use explicit file fields instead:

```json
{  
  "prompt": {  
    "override_file": ".gestalt/system-prompt.md"  
  }  
}
```

Future fields such as certificates or provider metadata files should have typed `*_file` properties.

### Credentials

Credentials remain outside ordinary config.

Allowed references:

```json
{  
  "auth_ref": "secret:provider/openrouter",  
  "api_key_env": "OPENROUTER_API_KEY"  
}
```

Raw API keys in `gestalt.json` are rejected or produce a high-severity validation warning.

---

## Schema Refinements

## DefaultsConfig

### Current responsibility

Defines the default session selection and execution envelope.

### Proposed shape

```json
{  
  "defaults": {  
    "profile": "default",  
    "provider": null,  
    "model": null,  
    "variant": null,  
    "mode": "confirm",  
    "max_turns": 50,  
    "max_output_tokens": 8192,  
    "temperature": null,  
    "top_p": null  
  }  
}
```

### Rules

- `mode` is an enum: `confirm`, `yolo`, `human`, `dry_run`, `replay`.
- `max_turns` minimum is `1`.
- `max_output_tokens` minimum is `1`.
- `temperature` and `top_p` remain optional.
- unsupported sampling controls are omitted by provider adapters rather than sent blindly.
- `variant` selects a named variant for the resolved model.
- direct `provider`, `model`, or `variant` values override values inherited from `profile`.

### Resolution order

built-in defaults  
→ selected profile  
→ direct defaults override  
→ CLI override  
→ session override

---

## ProfileConfig

Profiles become **session selection overlays**, not complete nested configurations.

### Proposed shape

```json
{  
  "profiles": {  
    "default": {  
      "provider": "openrouter",  
      "model": "openai/gpt-5.5",  
      "variant": "high",  
      "mode": "confirm",  
      "max_turns": 50,  
      "max_output_tokens": 8192  
    },  
    "fast": {  
      "provider": "openrouter",  
      "model": "anthropic/claude-haiku-4.5",  
      "variant": "low",  
      "max_turns": 20  
    }  
  }  
}
```

Profiles may configure:

- provider;
- model;
- variant;
- mode;
- max turns;
- max output tokens;
- optional generic sampling controls.

Profiles may not contain:

- policies;
- extension trust;
- provider credentials;
- arbitrary hooks;
- full context pipelines;
- nested profiles.

This prevents profiles from becoming alternate root configs.

---

## ProviderConfig

### Responsibility

Defines a connection and adapter configuration, not a user identity and not a full model catalog.

### Proposed shape

```json
{  
  "providers": {  
    "openrouter": {  
    "kind": "openai-compatible",  
    "display_name": "OpenRouter",  
    "base_url": "https://openrouter.ai/api/v1",  
    "auth_ref": "secret:provider/openrouter",  
    "api_key_env": "OPENROUTER_API_KEY",  
    "default_model": "openrouter/free",  
    "models_endpoint": null,  
    "headers": {  
      "HTTP-Referer": "https://example.com",  
      "X-Title": "Gestalt"  
    },  
    "request": {  
      "timeout_ms": 300000,  
      "stream_chunk_timeout_ms": 30000  
    },  
    "models": {}
    }  
  }  
}
```

### Changes

- remove nested `id`; the map key is the provider connection ID;
- type `kind` as a closed enum for built-ins plus `openai-compatible`;
- remove `protocol` when it merely duplicates `kind`;
- retain a protocol field only if a provider connection can truly choose among multiple adapters;
- validate `base_url` and `models_endpoint` as URLs;
- validate `api_key_env` as an environment variable identifier;
- classify sensitive headers and redact them;
- add request-level timeouts;
- add an optional per-provider `models` map.

### Provider-specific options

Provider-specific options must not be placed directly into `ProviderRequest` as an untyped object originating from user config.

Use a two-stage boundary:

gestalt.json model/provider options  
→ validated ProviderOptionSet  
→ provider adapter serializer

Each built-in adapter owns a typed option schema.

OpenAI-compatible custom providers may support a constrained pass-through map under:

```json
{  
  "adapter_options": {}  
}
```

This is experimental in v0.1 and must be:

- size-limited;
- traceable;
- excluded from security-sensitive fields;
- consumed only by the selected adapter;
- absent from `AgentLoop`.

---

## Model Definitions and Variants

## Problem

Models increasingly expose multiple operating variants without changing the base model ID.

Examples include:

- reasoning effort: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`;
- Anthropic thinking budget or adaptive effort presets;
- verbosity controls;
- reasoning summary controls;
- provider routing preferences;
- cache behavior;
- model-specific output limits.

Representing each combination as a duplicate model entry causes drift and makes selection difficult.

## Decision

Add provider-scoped model configuration with named variants.

### Proposed schema

```json
{  
  "providers": {  
    "openai": {  
      "kind": "openai",  
      "models": {  
        "gpt-5.5": {  
          "display_name": "GPT-5.5",  
          "options": {  
            "text_verbosity": "low"  
          },  
          "variants": {  
            "none": {  
              "options": {  
                "reasoning_effort": "none"  
              }  
            },  
            "low": {  
              "options": {  
                "reasoning_effort": "low"  
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
      "kind": "anthropic",  
      "models": {  
        "claude-sonnet-4-6": {  
          "variants": {  
            "fast": {  
              "options": {  
                "thinking": {  
                  "type": "adaptive",  
                  "effort": "low"  
                }  
              }  
            },  
            "high": {  
              "options": {  
                "thinking": {  
                  "type": "adaptive",  
                  "effort": "high"  
                }  
              }  
            },  
            "legacy-16k": {  
              "options": {  
                "thinking": {  
                  "type": "enabled",  
                  "budget_tokens": 16000  
                }  
              }  
            }  
          }  
        }  
      }  
    }  
  }  
}
```


### Canonical model reference

A resolved model selection consists of:

```rust
pub struct ModelSelection {  
    pub provider_id: String,  
    pub model_id: String,  
    pub variant: Option<String>,  
}
```

User-facing references use separate fields in config:

```json
{  
  "provider": "openai",  
  "model": "gpt-5.5",  
  "variant": "high"  
}

```

CLI shorthand may support:

openai/gpt-5.5@high

The `@variant` suffix is CLI syntax only. Internally, model ID and variant remain separate to avoid ambiguity with provider model IDs that contain punctuation.

### Option merge order

provider defaults  
→ model options  
→ selected variant options  
→ profile request overrides  
→ CLI/session request overrides

### Variant inheritance

A custom variant may optionally extend another variant:

```json
{  
  "variants": {  
    "high": {  
      "options": {  
        "reasoning_effort": "high"  
      }  
    },  
    "high-verbose": {  
      "extends": "high",  
      "options": {  
        "text_verbosity": "high"  
      }  
    }  
  }  
}
```


Rules:

- only one parent;
- inheritance is restricted to the same model;
- cycles are validation errors;
- final options are materialized during config resolution;
- provider adapters receive only the resolved variant;
- trace records the variant name and resolved option fingerprint.

### Built-in variants

Gestalt may ship adapter-owned presets for known models.

Built-in variants are catalog metadata, not hard-coded into `AgentLoop`.

User configuration may:

- override a built-in variant;
- disable a built-in variant;
- define additional variants.

```json
{  
  "variants": {  
    "fast": {  
      "disabled": true  
    },  
    "company-high": {  
      "extends": "high",  
      "options": {  
        "text_verbosity": "low"  
      }  
    }  
  }  
}
```

### Validation

The selected variant must be validated against:

- the resolved provider adapter;
- model capability metadata;
- the adapter's option schema;
- model-specific supported values where known.

A configuration may be syntactically valid but capability-invalid:

model does not support reasoning_effort=xhigh

That must fail in `gestalt config validate` or provider/model resolution before the request is sent.

### Provider-neutral versus provider-native options

Gestalt should normalize only options with stable cross-provider meaning:

```rust
pub struct GenericGenerationOptions {  
    pub max_output_tokens: Option<u32>,  
    pub temperature: Option<f32>,  
    pub top_p: Option<f32>,  
    pub reasoning_effort: Option<ReasoningEffort>,  
    pub text_verbosity: Option<TextVerbosity>,  
}
```


Provider-native capabilities remain in typed adapter options.

For example:

Generic reasoning_effort=high  
  OpenAI adapter    → reasoning.effort = "high"  
  Anthropic adapter → thinking.type = "adaptive", effort = "high"

This mapping is allowed only when semantics are sufficiently compatible. Otherwise, the variant must use provider-native typed options.

No adapter may silently approximate an unsupported option without emitting a resolution warning.

---

## PromptConfig

### Proposed shape

```json
{  
  "prompt": {  
    "assembly_strategy": "snapshot",  
    "override": null,  
    "override_file": ".gestalt/system.md"  
  }  
}
```



Rules:

- `assembly_strategy` is `dynamic` or `snapshot`;
- `override` and `override_file` are mutually exclusive;
- `override` means complete system prompt replacement;
- supplemental instructions belong in workspace or skill context, not another ambiguous prompt field.

---

## ContextConfig

### Proposed shape

```json
{
  "context": {
    "context_window_override": null,
    "reserved_output_tokens": 8192,
    "safety_margin_tokens": 2048,
    "workspace": {"path": ".gestalt/workspace.md"},
    "memory": {"path": ".gestalt/memory.md"}
  }
}
```


### Changes

- rename `max_context_window` to `context_window_override`;
- make model catalog metadata authoritative by default;
- use the override only for unknown/custom models or operator correction;
- add `safety_margin_tokens`;
- require positive token limits.

### Effective request budget

effective model context limit  
- reserved output  
- safety margin  
= maximum compiled input budget

Context compaction strategy remains internal or experimental until stable.

---

## ToolsConfig

### Proposed shape

```json
{  
  "tools": {  
    "default_timeout_secs": 60,  
    "bash_timeout_secs": 60,  
    "max_output_bytes": 1048576,  
    "max_output_tokens": 4000,  
    "max_parallel_calls": 4,  
    "sandbox_type": "none",  
    "ignore_patterns": [  
      ".git/**",  
      "target/**"  
    ]  
  }  
}
```

### Semantics

- `max_output_bytes` is the hard execution/storage boundary;
- `max_output_tokens` controls model-visible shaped output;
- `max_parallel_calls` bounds concurrent read-only tool execution;
- tool-specific timeouts override the default;
- sandbox values are enums and include only implemented backends;
- `sandbox_type=none` must not imply security containment.

---

## PoliciesConfig

The existing policy domains are retained:

paths  
bash  
network

### Policy actions

Use:

allow  
confirm  
deny

as the shared policy action enum.

### Bash policy

Preferred v0.1 representation:

```json
{  
  "bash": {  
    "default": "confirm",  
    "allow": [  
      "cargo test",  
      "cargo check",  
      "git status",  
      "rg"  
    ],  
    "confirm": [  
      "rm",  
      "git push",  
      "curl",
      "wget",
      "chmod"  
    ],  
    "deny": [  
      "mkfs",  
      "dd"  
    ]  
  }  
}
```

Pre-hardening policy aliases are rejected as unknown fields.

### Deterministic precedence

deny  
→ confirm  
→ allow  
→ default

Path and network policy follow the same deny-first principle.

### Scope rules

- workspace config may add deny rules;
- workspace config may narrow allow rules;
- managed deny rules cannot be removed;
- trust and credentials are not inferred from policy allow rules.

---

## McpConfig

### Proposed shape

```json
{  
  "mcp": {  
    "discovery_threshold": 20,  
    "servers": {  
      "workspace-search": {  
        "display_name": "Workspace Search",  
        "enabled": true,  
        "lifecycle": "lazy",  
        "allow_sampling": false,  
        "trust_level": "local_stdio",  
        "transport": {  
          "type": "stdio",  
          "command": "workspace-search-mcp",  
          "args": [],  
          "cwd": ".",  
          "env": {}  
        },  
        "timeouts": {  
          "connect_ms": 5000,  
          "request_ms": 60000  
        }  
      },  
      "remote-search": {  
        "enabled": false,  
        "trust_level": "remote",  
        "transport": {  
          "type": "http",  
          "url": "https://example.com/mcp",  
          "headers": {}  
        }  
      }  
    }  
  }  
}
```



### Changes

- add `enabled`;
- rename `name` to `display_name`, since the map key is the canonical ID;
- keep `env` only inside stdio transport;
- add `cwd` for local servers;
- use `stdio` and `http`/`remote` terminology aligned with the implemented MCP client;
- add typed timeouts;
- validate URLs and transport-specific properties;
- set `additionalProperties: false` on every transport variant;
- keep sampling disabled by default;
- do not treat user-supplied trust labels as proof of safety.

### Tool exposure

MCP lifecycle and MCP tool exposure are separate:

server enabled  
→ server may start/discover  
→ catalog planner decides which tools are exposed this turn  
→ runtime policy still gates execution

This prevents large MCP catalogs from automatically consuming prompt budget.

---

## SkillsConfig

Retain:

```json
{  
  "skills": {  
    "explicit_paths": [],  
    "active": [],  
    "trusted": []  
  }  
}
```

Definitions:

- `explicit_paths`: discovery locations;
- `active`: force activation for the session/workspace;    
- `trusted`: allow automatic activation from otherwise non-auto-trusted sources.


Skills remain capability-narrowing instruction packages. They cannot grant authority or register runtime capabilities.

---

## ExtensionsConfig

### Proposed shape

```json
{  
  "extensions": {  
    "explicit_loads": [],  
    "disabled": [],  
    "trusted": [],  
    "allow_untrusted": false,  
    "instances": {
      "review-primary": {
        "package": "com.example.review",
        "enabled": true,
        "components": {
          "lifecycle": true,
          "client-metadata": true
        },
        "config": {
          "policySet": "default"
        },
        "grants": {
          "workspaceRead": true,
          "workspaceWrite": false,
          "network": []
        }
      }
    },
    "timeouts": {  
      "initialize_ms": 10000,  
      "hook_ms": 5000,  
      "context_ms": 15000,  
      "tool_ms": 60000,  
      "shutdown_ms": 5000  
    },  
    "limits": {  
      "max_message_bytes": 8388608,  
      "max_pending_requests": 16,  
      "max_protocol_errors": 3  
    }  
  }  
}
```

### Semantics

- `explicit_loads`: additional discovery paths;
- `disabled`: explicitly disable IDs; deny wins if listed in both;
- `trusted`: exact package ID/hash pairs approved by the user; bare IDs do not
  establish trust;
- `allow_untrusted`: unsafe development escape hatch, false by default;
- `instances`: configured extension package instances keyed by stable instance ID;
- timeouts may override runtime defaults;
- limits protect the broker from unbounded messages and protocol abuse.

An explicitly loaded extension is **discoverable**, not automatically trusted.

`instances.<id>.package` selects a discovered package. `instances.<id>.components`
enables or disables package components by component ID. `instances.<id>.config`
is the canonical location for extension-specific configuration. `instances.<id>.grants`
records host-owned grants and cannot expand authority beyond package-requested
permissions or runtime policy.

Profiles do not contain inline extension configuration. A future profile field may
select existing instance IDs, but it must remain additive and must not reshape
`extensions.instances`.

---

## ObserveConfig

### Proposed shape

```json
{  
  "observe": {  
    "enabled": true,  
    "run_log_dir": ".gestalt/runs",  
    "log_format": "jsonl",  
    "redact_secrets": true,  
    "runtime_history_limit": 10000  
  }  
}
```

Rules:

- `jsonl` is the only stable v0.1 log format;
- secret redaction is always active; the flag may be omitted if it cannot be disabled;
- replay-critical events must not be dropped from persisted traces;
- in-memory runtime event history must be bounded independently from trace persistence.

---

## TuiConfig

TUI preferences remain isolated from runtime semantics.

The long-term preferred layout is:

gestalt.json  → runtime/workspace behavior  
tui.json      → user-interface preferences

For v0.1, retaining the existing `tui` section is acceptable. It should not expand beyond implemented stable settings.

---

## ExperimentalConfig

Add:

```json
{  
  "experimental": {}  
}
```

Candidate experimental fields:

- provider option pass-through;
- extension concurrent RPC;
- extension cancellation notifications;
- hot reload;
- remote extensions;
- hook effect aggregation;
- dynamic provider switching;
- context compaction controls.

Experimental keys:

- may change within the same major pre-1.0 release;
- must be excluded from stable compatibility guarantees;
- must emit a startup notice when used;
- still require typed schema definitions where possible.

---

# Extension Protocol Review

## Current assessment

The existing protocol is a good MVP foundation:

- JSON-RPC 2.0 is language-neutral and widely understood;
- NDJSON over stdio is simple to implement;
- the host initiates requests;
- request IDs permit future concurrency;
- stderr is separate from protocol output;
- extension processes receive a scrubbed environment;
- extension tools register through normal tool interfaces;
- host policy and approval remain authoritative;
- lifecycle activity is emitted through `RuntimeEvent`.

The protocol should **not** be replaced before v0.1.

It does, however, require hardening before being treated as a durable third-party extension contract.

---

## Extension Protocol v1.1 Goals

1. Negotiate protocol compatibility.
2. Distinguish host, manifest, extension, and protocol versions.
3. Validate response envelopes strictly.
4. Fail active requests on malformed protocol output.
5. Add message and pending-request limits.
6. Add method-specific timeouts.
7. Define cancellation and shutdown behavior.
8. Define extension-to-host notifications or explicitly prohibit them by negotiated capability.
9. Make hook failure and composition semantics contractual.
10. Preserve deterministic tool registration and namespace behavior.
11. Bind trust decisions to extension identity and integrity metadata.
12. Keep all changes outside `AgentLoop`.

---

## Protocol versioning and handshake

### Manifest additions

manifest_version = 1  
protocol_version = "1.1"  
id = "acme.search"  
name = "Acme Search"  
version = "0.3.0"  
runtime = "stdio"

Definitions:

- `manifest_version`: schema version of `gestalt.extension.toml`;
- `protocol_version`: requested extension protocol version/range;
- `version`: extension package version;
- `runtime`: transport implementation.

### Initialize request

```json
{  
  "jsonrpc": "2.0",  
  "id": "uuid",  
  "method": "initialize",  
  "params": {  
    "protocol": {  
      "version": "1.1",  
      "supported": ["1.0", "1.1"]  
    },  
    "host": {  
      "name": "gestalt",  
      "version": "0.1.0"  
    },  
    "extension": {  
      "id": "acme.search",  
      "manifest_version": 1,  
      "package_version": "0.3.0",  
      "manifest_hash": "sha256:..."  
    },  
    "workspace": {  
      "id": "opaque-workspace-id"  
    },  
    "capabilities": {  
      "tools": true,  
      "hooks": true,  
      "context": false,  
      "notifications": false,  
      "cancellation": true,  
      "concurrent_requests": false  
    },  
    "limits": {  
      "max_message_bytes": 8388608,  
      "max_pending_requests": 16  
    }  
  }  
}
```

Do not send raw workspace paths unless the extension has already been granted the relevant permission and the method requires the path.

### Initialize response

```json
{  
  "jsonrpc": "2.0",  
  "id": "uuid",  
  "result": {  
    "status": "ok",  
    "protocol_version": "1.1",  
    "capabilities": {  
      "notifications": false,  
      "cancellation": true,  
      "concurrent_requests": false  
    }  
  }  
}
```


### Compatibility rule

- same major protocol version: potentially compatible;
- selected protocol is the highest mutually supported version;
- no mutually supported version: reject before registration;
- protocol 1.0 is rejected and has no compatibility adapter;
- protocol negotiation is recorded in `ExtensionLoaded`.

---

## RPC envelope validation

Every response must satisfy:

- `jsonrpc == "2.0"`;
- `id` is present for responses;
- `id` matches an active request;
- exactly one of `result` or `error` is present;
- response is below the configured byte limit;
- method result validates against the method's response schema.

Malformed lines must not be silently discarded.

Required behavior:

malformed stdout line  
→ publish ExtensionProtocolError  
→ fail the oldest/active serialized request  
→ increment protocol error count  
→ terminate extension when threshold is reached

If concurrent requests are enabled later and a malformed response has no usable ID, fail all pending requests because response correlation is no longer reliable.

Unknown response IDs produce a protocol warning and count toward the fault threshold.

---

## Message size and output handling

The current unlimited line size is not acceptable as a stable contract.

Add:

max_message_bytes  
max_tool_result_bytes  
max_context_result_bytes  
max_stderr_line_bytes

For large outputs, prefer artifact references over huge inline JSON:

```json
{  
  "content": "Conversion complete.",  
  "artifacts": [  
    {  
      "path": ".gestalt/artifacts/run-123/result.json",  
      "mime_type": "application/json",  
      "size_bytes": 52428800  
    }  
  ],  
  "truncated": false,  
  "metadata": {}  
}
```


Artifact paths are validated by the host before entering session history.

---

## Typed tool results

Replace the string-only effective contract with:

```rust
interface ExtensionToolResult {  
  content: string;  
  is_error?: boolean;  
  artifacts?: ToolArtifact[];  
  truncated?: boolean;  
  original_bytes?: number;  
  metadata?: object;  
}
```

The host converts this to canonical `ToolExecutionResult`.

A missing `content` field is a protocol validation failure. Do not silently
serialize the entire result object as fallback. Protocol 1.0 is unsupported.

---

## Context injection results

Return a typed context contribution rather than only a raw system string:

```rust
interface ContextInjectResult {  
  items: Array<{  
    id: string;  
    content: string;  
    trust: "trusted" | "untrusted";  
    stability: "session_static" | "activation_static" | "turn_dynamic" | "ephemeral";  
    priority: "critical" | "high" | "medium" | "low";  
    source?: object;  
  }>;  
}
```

Security rules:

- extensions cannot self-declare content as trusted unless the host has explicitly granted trusted-context authority;
- default trust is `untrusted`;
- stability is advisory and may be downgraded by the host;
- `critical` priority is reserved for host-owned instructions unless explicitly permitted;
- context contributions are subject to token budgeting and trust rendering.

This aligns extensions with cache-aware prompt assembly rather than injecting undifferentiated `Message::System` values.

---

## Hook protocol

### Current protocol decision

Retain `hooks/call`, but tighten its schema and behavior.

### Typed lifecycle request

Each lifecycle point should have a distinct parameter schema rather than one large optional `HookContext`.

Example methods may remain multiplexed:

hooks/call + lifecycle_point

but deserialization must dispatch to a typed enum:

```rust
enum HookCallParams {  
    BeforeContextBuild(BeforeContextBuildParams),  
    AfterContextBuild(AfterContextBuildParams),  
    BeforeToolPolicy(BeforeToolPolicyParams),  
    AfterToolResult(AfterToolResultParams),  
    PrepareNextTurn(PrepareNextTurnParams),  
    OnEvent(OnEventParams),  
}
```


### Hook outcomes

Use one tagged shape consistently:

```json
{ "type": "continue" }  
{ "type": "block", "reason": "..." }  
{ "type": "add_context", "items": [...] }  
{ "type": "annotate", "metadata": {} }  
{ "type": "switch_model", "selection": { "provider": "...", "model": "...", "variant": "..." } }

```

Do not support both raw string `"continue"` and tagged-object forms. Protocol
1.0 raw-string outcomes are unsupported.

### Hook composition

Replace implicit last-writer-wins behavior with explicit aggregation where possible:

```rust
pub struct HookEffects {  
    pub blocks: Vec<HookBlock>,  
    pub context_additions: Vec<ContextItem>,  
    pub annotations: BTreeMap<String, Value>,  
    pub next_turn_override: Option<ModelSelection>,  
}
```


Rules:

- any block stops the applicable operation;
- context additions append deterministically in registered extension order;
- annotation key collisions use explicit namespacing by extension ID;
- at most one next-turn model override may win;
- conflicting model overrides produce a deterministic conflict event rather than silently selecting the last extension.

### Failure modes

Each hook declaration gains a host-side failure mode:

[[hooks]]  
name = "company-policy"  
lifecycle_point = "before_tool_policy"  
failure_mode = "closed"  
timeout_ms = 3000

Allowed modes:

closed  
open  
disable_extension  
stop_session

The host may restrict valid modes by lifecycle point:

- `before_tool_policy`: defaults to `closed`;
- context hooks: defaults to `open`;
- `after_tool_result`: defaults to `open`;
- `prepare_next_turn`: defaults to `open`;
- `on_event`: always observational and cannot stop execution.

Workspace configuration cannot change a managed policy hook from closed to open.

---

## Cancellation

Add host-to-extension notification support:

```json
{  
  "jsonrpc": "2.0",  
  "method": "$/cancelRequest",  
  "params": {  
    "id": "original-request-id",  
    "reason": "session_cancelled"  
  }  
}
```


Rules:

- available only when negotiated;
- timeout or session cancellation triggers notification;
- the host stops waiting after the normal cancellation grace period;
- an extension that ignores cancellation may be terminated;
- cancellation does not imply rollback.

Protocol 1.0 extensions are rejected before lifecycle execution.

---

## Shutdown

Add a graceful shutdown request:

```json
{  
  "jsonrpc": "2.0",  
  "id": "uuid",  
  "method": "shutdown",  
  "params": {  
    "reason": "runtime_exit"  
  }  
}
```

Then:

```json
{  
  "jsonrpc": "2.0",  
  "method": "exit"  
}
```

Lifecycle:

shutdown request  
→ wait shutdown timeout  
→ exit notification  
→ wait process grace period  
→ kill if still running

`kill_on_drop` remains the final safety net.

---

## Concurrency and backpressure

Keep one outstanding request per extension as the default for v0.1.

The handshake may negotiate:

```json
{  
  "concurrent_requests": false,  
  "max_concurrency": 1  
}
```


Later concurrency requirements:

- request IDs remain mandatory;
- pending request count is bounded;
- hooks affecting ordering remain serialized;
- read-only tool calls may use concurrency only if both extension and tool declaration allow it;
- output ordering is restored by the host;
- event queues are bounded;
- dropped observational events emit a lag/drop event.

Do not enable concurrent extension RPC merely because JSON-RPC IDs technically permit it.

---

## Notifications and extension-initiated messages

The current host-initiated-only rule is safe and should remain the v0.1 default.

Future extension notifications may be negotiated for:

- structured extension logs;
- progress events;
- artifact progress;
- health status.

Extensions must not be allowed to initiate:

- arbitrary tool executions;
- session messages;
- policy changes;
- model requests;
- context mutations.

Those operations must remain host-requested or flow through explicit runtime APIs.

---

## Protocol errors and runtime events

Add or refine runtime events:

```rust
ExtensionProtocolNegotiated {  
    extension_id: String,  
    protocol_version: String,  
    capabilities: ExtensionProtocolCapabilities,  
}  
​  
ExtensionProtocolError {  
    extension_id: String,  
    request_id: Option<String>,  
    kind: ProtocolErrorKind,  
    message: String,  
    fault_count: u32,  
}  
​  
ExtensionLog {  
    extension_id: String,  
    level: Option<String>,  
    message: String,  
}  
​  
ExtensionDisabled {  
    extension_id: String,  
    reason: String,  
}  
​  
ExtensionRequestCancelled {  
    extension_id: String,  
    request_id: String,  
    reason: String,  
}
```

Do not classify every stderr line as an extension error. Stderr is a log channel; protocol failures are separate events.

Persist replay-relevant extension events to the trace. High-volume logs may remain runtime-only or be sampled.

---

## Extension manifest refinements

### Required validation

Add validation for:

- `manifest_version`;
- protocol version/range;
- semantic extension version;
- extension ID pattern;
- duplicate tool names;
- duplicate hook names;
- duplicate context injector names;
- valid lifecycle points;
- non-empty descriptions;
- valid JSON Schema objects;
- reserved namespaces;
- valid risk levels;
- valid failure modes;
- valid timeout ranges;
- valid permission hosts and paths.

Recommended extension ID:

^[a-z][a-z0-9.-]{0,63}$

### Capability declarations

For v0.1, retain the existing capability booleans for compatibility.

During resolution:

- declarations must imply matching capability flags;
- a future manifest version may derive capability flags from declarations;
- the initialize handshake sends resolved capabilities, not blindly copied raw booleans.

### Trust and integrity

A trust record should be keyed by:

extension ID  
+ manifest hash  
+ executable identity/hash where practical

If the manifest or executable changes:

- previous trust is invalidated or requires reconfirmation;
- runtime emits a changed-integrity event.
- managed trust policy may pin hashes    

Location alone does not imply trust.

---

## Permission model interaction

The protocol hardening does not turn host-side permission checks into OS sandboxing.

Required user-facing language:

> Extension permissions govern host-mediated operations and provide auditability. Native extension processes are not isolated from the operating system unless a configured sandbox backend is active.

Current V2 extension declarations should support explicit resource annotations:

```toml
[[tools.resources]]  
json_path = "$.input_path"  
kind = "filesystem"  
access = "read"  
​  
[[tools.resources]]  
json_path = "$.output_path"  
kind = "filesystem"  
access = "write"  
​  
[[tools.resources]]  
json_path = "$.url"  
kind = "network"  
access = "connect"
```

Host behavior:

tool call input  
→ evaluate declared resource fields  
→ apply workspace policy  
→ apply extension manifest ceiling  
→ request approval if required  
→ send RPC

The manifest permission set is a ceiling, never a grant beyond workspace policy.

---

# Additional Considerations

## 5. Provider allowlists and denylists

Add optional controls:

```json
{  
  "providers": {  
    "...": {}  
  },  
  "provider_policy": {  
    "enabled": ["openrouter", "anthropic"],  
    "disabled": ["untrusted-gateway"]  
  }  
}

To avoid another root key in v0.1, this may instead live under:

{  
  "policies": {  
    "providers": {  
      "allow": [],  
      "deny": []  
    }  
  }  
}
```

Deny wins.

This becomes useful for managed deployments and deterministic provider resolution.

## 6. Small/utility model routing

A harness often uses lightweight model calls for:

- session title generation;
- compression;
- extraction;
- tool selection;
- summarization;
- verification.

Do not hard-code a single `small_model` field. Reuse profiles:

```json
{  
  "routing": {  
    "utility": "profiles.fast",  
    "compression": "profiles.fast",  
    "verification": "profiles.verifier"  
  }  
}
```

Because `routing` is not currently stable, keep it under `experimental` or defer it until actual subsystems consume it.

## 7. Attachments and multimodal limits

Future-safe configuration may need:

maximum image dimensions  
maximum encoded bytes  
automatic resizing  
document size limits

Do not add the section until Gestalt supports these inputs consistently across providers. Hard safety limits should still exist internally in v0.1.

## 8. Session, trace, and artifact retention

The current schema configures the trace directory but not:

- retention;
- maximum artifact bytes;
- session checkpoint frequency;
- cleanup behavior.

These are valid harness concerns but not release blockers. Define internal conservative defaults and add stable configuration only when session persistence behavior is settled.

## 9. Schema publication and editor support

Publish the generated schema at a stable URL:

https://gestalt.noentic.com/schema/gestalt-v1.json

The generic alias:

https://gestalt.noentic.com/schema/gestalt.json

may point to the latest stable schema.

Repository validation must test that:

- checked-in schema matches generated Rust types;
- examples validate;
- invalid fixtures fail for the intended reason;
- schema URL is reachable in release CI where feasible.

## 10. JSON versus JSONC

The canonical file remains `gestalt.json`.

Optional JSONC support may be added as:

gestalt.jsonc

but must not be required for v0.1.

If both exist in one scope, startup fails with an ambiguity error rather than guessing.

## 11. Migration and deprecation

The following alias strategy is superseded by ADR-031. Stable config version 1
does not accept pre-hardening aliases.

The effective config renderer prints only canonical names.

## 12. Deterministic fingerprints

Compute stable hashes for:

- effective config;
- provider/model/variant selection;
- active tool catalog;
- policy plan;
- extension manifest and negotiated protocol;
- prompt snapshot.

Record fingerprints in the run trace and `RuntimeInspect`.

This enables replay diagnostics without logging secrets.

---

# Proposed `gestalt.json` Example

```json
{  
  "$schema": "https://gestalt.dev/schema/gestalt-v1.json",  
  "version": 1,  
​  
  "defaults": {  
    "profile": "default",  
    "mode": "confirm",  
    "max_turns": 50,  
    "max_output_tokens": 8192  
  },  
​  
  "profiles": {  
    "default": {  
      "provider": "openrouter",  
      "model": "openai/gpt-5.5",  
      "variant": "high"  
    },  
    "fast": {  
      "provider": "openrouter",  
      "model": "anthropic/claude-haiku-4.5",  
      "variant": "low",  
      "max_turns": 20  
    }  
  },  
​  
  "providers": {  
    "openrouter": {  
    "kind": "openai-compatible",  
    "display_name": "OpenRouter",  
    "base_url": "https://openrouter.ai/api/v1",  
    "api_key_env": "OPENROUTER_API_KEY",  
    "default_model": "openrouter/free",  
    "request": {  
      "timeout_ms": 300000,  
      "stream_chunk_timeout_ms": 30000  
      },  
      "models": {  
        "openai/gpt-5.5": {  
          "variants": {  
            "low": {  
              "options": {  
                "reasoning_effort": "low",  
                "text_verbosity": "low"  
              }  
            },  
            "high": {  
              "options": {  
                "reasoning_effort": "high",  
                "text_verbosity": "low"  
              }  
            },  
            "xhigh": {  
              "options": {  
                "reasoning_effort": "xhigh",  
                "text_verbosity": "low"  
              }  
            }  
          }  
        }  
      }  
    }  
  },  
​  
  "prompt": {  
    "assembly_strategy": "snapshot"  
  },  
​  
  "context": {  
    "reserved_output_tokens": 8192,  
    "safety_margin_tokens": 2048,  
    "workspace": {"path": ".gestalt/workspace.md"},
    "memory": {"path": ".gestalt/memory.md"}
  },  
​  
  "tools": {  
    "default_timeout_secs": 60,  
    "bash_timeout_secs": 60,  
    "max_output_bytes": 1048576,  
    "max_output_tokens": 4000,  
    "max_parallel_calls": 4,  
    "sandbox_type": "none",  
    "ignore_patterns": [  
      ".git/**",  
      "target/**"  
    ]  
  },  
​  
  "policies": {  
    "paths": {  
      "allow_read": ["."],  
      "allow_write": ["docs/", ".gestalt/"],  
      "deny_read": [".env", ".env.*", "*.key", "*.pem"],  
      "deny_write": [".git/", ".env", "*.key", "*.pem"]  
    },  
    "bash": {  
      "default": "confirm",  
      "allow": ["cargo test", "cargo check", "git status", "rg"],  
      "confirm": ["rm", "git push", "curl", "wget"],  
      "deny": ["mkfs", "dd", "fdisk"]  
    },  
    "network": {  
      "default": "confirm",  
      "allow_domains": ["github.com", "crates.io", "docs.rs"],  
      "deny_domains": []  
    }  
  },  
​  
  "mcp": {  
    "discovery_threshold": 20,  
    "servers": {}  
  },  
​  
  "skills": {  
    "explicit_paths": [],  
    "active": [],  
    "trusted": []  
  },  
​  
  "extensions": {  
    "explicit_loads": [],  
    "enabled": [],  
    "disabled": [],  
    "trusted": [],  
    "required": [],  
    "allow_untrusted": false,  
    "timeouts": {  
      "initialize_ms": 10000,  
      "hook_ms": 5000,  
      "context_ms": 15000,  
      "tool_ms": 60000,  
      "shutdown_ms": 5000  
    },  
    "limits": {  
      "max_message_bytes": 8388608,  
      "max_pending_requests": 16,  
      "max_protocol_errors": 3  
    }  
  },  
​  
  "observe": {  
    "enabled": true,  
    "run_log_dir": ".gestalt/runs",  
    "log_format": "jsonl",  
    "runtime_history_limit": 10000  
  },  
​  
  "experimental": {}  
}

```

---

# Implementation Plan

## Phase 1 — Schema correctness and resolution

### Deliverables

- require config version 1;
- replace free-form closed strings with enums;
- remove redundant provider ID;
- define merge semantics;
- implement config provenance;
- add reference validation;
- add canonical effective-config rendering;
- implement `config validate`, `show`, `explain`, and `paths`;
- add migration aliases and warnings;
- add schema golden tests.

### Exit criteria

- every invalid closed value fails before runtime construction;
- the same inputs produce the same effective config and fingerprint;
- secrets never appear in effective-config output;
- all shipped examples validate.

## Phase 2 — Model definitions and variants

### Deliverables

- provider-scoped model map;
- named model variants;
- optional same-model variant inheritance;
- profile/default variant selection;
- typed generic generation options;
- typed OpenAI and Anthropic adapter option sets;
- capability validation;
- CLI `provider/model@variant` shorthand;
- trace fields for variant and option fingerprint.

### Exit criteria

- OpenAI low through xhigh variants resolve without duplicate model entries;
- Anthropic adaptive thinking variants resolve through the Anthropic adapter;
- unsupported variants fail before network execution;
- provider-native options do not enter `AgentLoop`.

## Phase 3 — Tool, MCP, and observability limits

### Deliverables

- hard byte and token output limits;
- bounded parallel tool calls;
- default and tool-specific timeouts;
- MCP `enabled`, `cwd`, and typed timeout fields;
- strict transport schemas;
- bounded runtime event history;
- trace/runtime log separation.

### Exit criteria

- oversized tool output is truncated or artifact-routed deterministically;
- MCP servers can be disabled without deleting config;
- malformed MCP transport combinations fail schema validation;
- long sessions do not produce unbounded in-memory runtime history.

## Phase 4 — Extension protocol hardening (superseded in part)

Protocol 1.0 compatibility work in this phase is superseded by ADR-031.
Lifecycle Protocol V2 is the only supported Gestalt lifecycle protocol.

### Deliverables

- manifest and protocol version fields;
- negotiated initialize handshake;
- strict RPC envelope validation;
- message-size and pending-request limits;
- typed tool and context results;
- method-specific timeouts;
- protocol error events;
- graceful shutdown;
- optional negotiated cancellation;
- protocol 1.0 rejection and compatibility-adapter removal;
- extension integrity-aware trust records.

### Exit criteria

- incompatible extensions fail at initialization with actionable errors;
- malformed stdout cannot degrade only into a timeout;
- a large extension response cannot allocate without a configured bound;
- protocol 1.0 fixtures are rejected before activation;
- all extension lifecycle decisions are observable.

## Phase 5 — Hook contract stabilization

### Deliverables

- typed hook call contexts;
- single tagged outcome format;
- deterministic effect aggregation;
- namespaced annotations;
- explicit model selection including variant;
- per-hook failure modes;
- bounded observation queue;
- hook protocol golden fixtures.

### Exit criteria

- hook conflicts produce deterministic outcomes;
- policy hooks remain fail-closed;
- context additions from multiple extensions do not overwrite silently;
- observational hook lag is visible and cannot exhaust memory.

---

# Testing Strategy

## Configuration fixtures

Required fixtures:

minimal_valid  
full_valid  
unknown_top_level_key  
unknown_nested_key  
invalid_version  
invalid_mode  
profile_missing_provider  
profile_unknown_variant  
variant_cycle  
unsupported_variant_option  
prompt_override_conflict  
workspace_attempts_policy_widen  
managed_deny_preserved  
provider_header_redaction  
mcp_invalid_transport  
extension_duplicate_lists

## Resolution golden tests

Each fixture records:

- effective configuration;
- provenance per important field;
- warnings;
- errors;
- stable fingerprint.

## Provider variant tests

Required cases:

openai_reasoning_none  
openai_reasoning_xhigh  
anthropic_adaptive_low  
anthropic_adaptive_high  
anthropic_legacy_budget  
variant_inheritance  
variant_disabled  
variant_unsupported_by_model  
profile_variant_override  
cli_variant_override

## Extension protocol fixtures

Required fixtures:

v1_0_initialize_success  
v1_1_negotiate_success  
incompatible_protocol  
malformed_json  
invalid_jsonrpc_version  
response_missing_id  
response_unknown_id  
result_and_error_present  
oversized_response  
tool_result_typed  
context_items_typed  
hook_block  
hook_multi_context  
hook_conflicting_model_switch  
request_timeout  
request_cancelled  
graceful_shutdown  
process_exit_with_pending_request  
protocol_fault_threshold

## Security tests

- untrusted extension cannot become trusted by location alone;
- changed manifest hash invalidates trust;
- workspace config cannot remove managed denies;
- extension-declared resource access cannot exceed workspace policy;
- raw provider credentials are redacted;
- context extension content defaults to untrusted;
- extension cannot mark itself as critical trusted context    

---

# Acceptance Criteria

## Configuration

- [x]  `version` is required and equal to `1`.
- [x]  Unknown properties fail validation at every stable schema level.
- [x]  Closed sets are represented as enums.
- [x]  Merge semantics are documented and covered by tests.
- [x]  Arrays replace rather than concatenate unless explicitly documented.
- [x]  Workspace security configuration cannot silently widen managed authority.
- [x]  `config show` is deterministic and secret-redacted.
- [x]  `config explain` reports value provenance.
- [x]  The default generated config remains short.

## Models and variants

- [x]  Profiles and defaults can select a model variant.
- [x]  Variants are scoped to a provider/model pair.
- [x]  OpenAI-style reasoning efforts, including `xhigh` where supported, are representable.
- [x]  Anthropic thinking/adaptive effort options are representable.
- [x]  Model and variant options are validated by provider adapters.
- [x]  Variant resolution is recorded in traces.
- [x]  Provider-specific options never leak into `AgentLoop`.

## Extensions

- [x]  Protocol version is negotiated during initialization.
- [x]  Manifest version, extension version, and protocol version are distinct.
- [x]  Malformed responses fail visibly.
- [x]  Message size is bounded.
- [x]  Tool results use a typed contract.
- [x]  Context contributions carry trust, stability, and priority metadata.
- [x]  Hook contexts and outcomes have stable tagged schemas.
- [x]  Hook conflict behavior is deterministic.
- [x]  Timeouts are operation-specific.
- [x]  Graceful shutdown is attempted before process termination.
- [x]  Cancellation is negotiated and optional.
- [x]  Protocol and lifecycle errors emit runtime events.
- [ ]  Protocol 1.0 compatibility code is removed under ADR-031.
- [x]  Extension trust is not inferred solely from discovery location.

---

# Release Recommendation

The current configuration and extension architecture is sufficient for a v0.1 binary. The release should not wait for every future option in this specification.

The minimum release gate is:

1. schema versioning and strict enum validation;
2. deterministic config merge and provenance;
3. model variant support;
4. essential tool and RPC size/time limits;
5. extension protocol negotiation;
6. visible malformed-protocol failures;
7. honest trust and sandbox documentation;
8. stable config/extension diagnostics.

Typed context contribution items, cancellation, hook aggregation, and integrity-pinned trust can be implemented immediately after the minimum v0.1 contract if schedule pressure requires it, provided the protocol is explicitly marked pre-stable and does not claim full forward compatibility.

The key principle is:

> Stabilize the boundaries that third-party users and extensions depend on; keep implementation details and experimental options free to evolve behind those boundaries.

---

# Release Verification Matrix

The configuration and extension protocol refinements have been verified through complete test coverage:

| Target Domain | Verification Scope | Status | Verification Mechanism |
|---|---|---|---|
| **Configuration** | Strict schema validation, version requirement, enums | Verified | `config_schema_tests.rs` |
| **Merging & Provenance** | Layered precedence, array replacing, monotonicity | Verified | `config_tests.rs` |
| **Provider Mappings** | Named model variants, OpenAI reasoning, Anthropic thinking | Verified | `catalog_tests.rs`, `provider_model_cli_tests.rs` |
| **Extension Trust** | Integrity-aware validation, load/trust separation | Verified | `runtime_manifest_tests.rs`, `runtime_discovery_tests.rs` |
| **Protocol Limits** | message size, pending requests, timeouts, fault handling | Verified | `runtime_process_extension_tests.rs` |
| **Composition Hooks** | Aggregation, namespaced annotations, switch model conflict | Verified | `runtime_process_extension_tests.rs` |
| **Observability** | Fingerprints: effective config, model variant, negotiated proto | Verified | `runtime_process_extension_tests.rs`, `config_tests.rs` |
