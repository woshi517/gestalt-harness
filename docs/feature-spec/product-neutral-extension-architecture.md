---
title: "feat: Product-Neutral Extension Architecture"
date: 2026-06-23
status: proposed
type: feature-spec
depth: deep
target: post-v0.1
owners:
  - gestalt-runtime
  - gestalt-cli
  - gestalt-core
  - gestalt-mcp

---

# feat: Product-Neutral Extension Architecture

## 1. Summary

Refactor Gestalt's extensibility model into a product-neutral architecture that can support:

- standalone harness usage;
- embedded agents inside other applications;
- desktop or graphical clients;
- headless servers;
- remote workers;
- workload-specific distributions;
- third-party extension packages.

The architecture must preserve Gestalt as a harness rather than turning it into a desktop or application framework.

The central design is:

> One extension package format, multiple narrow component interfaces, one immutable runtime snapshot, and separate runtime and client extension hosts.

An extension package may contain both runtime capabilities and client/product contributions, but those components use different APIs and are activated independently by each host.

```text
Extension Package
├── Runtime Components
│   ├── Native runtime modules
│   ├── MCP servers
│   ├── Command tools
│   ├── Context providers
│   ├── Policy guards
│   ├── Turn routers
│   ├── Verifiers
│   ├── Event observers
│   └── Skills
│
├── Client/Product Components
│   ├── Commands
│   ├── Views or panels
│   ├── Artifact renderers
│   ├── Settings contributions
│   ├── Forms
│   └── Host-specific presentation metadata
│
├── Configuration Schema
├── Assets
└── Package Manifest
```

The feature also introduces:

- clear separation between package metadata, user configuration, and resolved dependency state;
- a narrow Gestalt-specific lifecycle protocol;
- MCP and command adapters as the preferred external tool paths;
- an application-neutral `RuntimeControl` interface;
- immutable runtime generations;
- transactional hot reload;
- host capability negotiation;
- extension-specific configuration schemas;
- deterministic composition and failure behavior.

---

# 2. Background

Gestalt currently exposes a broad extension abstraction through a runtime registry and process-backed JSON-RPC extensions.

The current model allows extensions to register or contribute:

- tools;
- lifecycle hooks;
- context injectors;
- providers;
- verifiers;
- runtime event listeners.

This direction is useful because it keeps extension execution outside `gestalt-core` and avoids placing extension-specific logic inside `AgentLoop`.

However, the current model has several architectural limitations:

1. **One extension abstraction represents too many responsibilities.**
   Packaging, process lifecycle, tools, hooks, context, providers, verification, and client concerns risk converging into one universal plugin model.

2. **The process protocol duplicates general-purpose tool protocols.**
   Gestalt currently defines its own tool declaration and invocation path even though MCP can serve as the public external tool protocol.

3. **Lifecycle hooks expose internal orchestration details.**
   Public extensions are coupled to internal hook names and generic hook outcomes rather than stable capability-oriented interfaces.

4. **Runtime composition is largely static.**
   Registries and composed components are built during startup. Existing reload behavior does not atomically replace a live runtime.

5. **Client integration does not yet have a formal boundary.**
   A future client or application host needs commands, views, approvals, events, settings, and artifact access without exposing runtime internals.

6. **Configuration ownership is ambiguous.**
   A unified `gestalt.json` is desirable for users, but extension package metadata and resolved package versions should not be embedded into the same file.

7. **A desktop-style extension ecosystem could accidentally pull UI concerns into the harness.**
   The harness must remain usable without any graphical client or product-specific API.

---

# 3. Problem Statement

Gestalt needs an extension architecture that is broad enough to make the harness malleable into many kinds of products, while keeping the core runtime:

- small;
- deterministic;
- embeddable;
- auditable;
- client-independent;
- provider-neutral;
- safe to evolve;
- suitable for local and remote execution.

The architecture must support rich extension packages without introducing a universal plugin interface that gives every extension direct access to:

- runtime internals;
- session state;
- UI internals;
- tool execution;
- policy decisions;
- filesystem state;
- application state.

---

# 4. Goals

## 4.1 Architectural goals

1. Separate package distribution from runtime behavior.
2. Separate runtime extensions from client/product extensions.
3. Keep `gestalt-core` unaware of extension packages, manifests, processes, clients, and files.
4. Keep extension behavior outside `AgentLoop`.
5. Preserve one canonical runtime tool, policy, context, trace, and approval path.
6. Allow one package to contain multiple independently activated component types.
7. Allow different hosts to activate different components from the same package.
8. Keep external protocols narrow and versioned.
9. Make runtime composition deterministic and inspectable.
10. Support transactional hot reload without mutating active turns.
11. Preserve `gestalt.json` as the canonical user-facing runtime configuration.
12. Keep package metadata in a separate extension manifest.
13. Introduce a generated lockfile for exact package resolution.
14. Make client integration possible through a stable control API rather than runtime internals.
15. Keep future sandboxing, remote transports, and signing possible without another architecture rewrite.

## 4.2 Developer-experience goals

1. A simple custom tool should require minimal boilerplate.
2. An external tool should not require implementing the full lifecycle protocol.
3. Runtime extensions should be authorable in Rust, Python, TypeScript, Go, or any language that can implement supported transports.
4. Extension configuration should be validated with JSON Schema.
5. Client hosts should be able to generate settings forms from the same schema.
6. Extension authors should have scaffolding, validation, inspection, and conformance tooling.
7. Reload failures should preserve the currently active extension generation.

## 4.3 Product-neutrality goals

1. No client type is assumed by the runtime.
2. No graphical interface is required.
3. No product-specific navigation or window model appears in `gestalt-core` or the runtime lifecycle protocol.
4. Remote workers may ignore client components.
5. Embedded hosts may provide their own client implementation.
6. Client/product extensions may be omitted entirely.

---

# 5. Non-Goals

This feature does not:

- define a marketplace;
- define billing or licensing;
- define a specific desktop application;
- define a specific graphical framework;
- define a specific server product;
- implement OS-level sandboxing;
- allow extensions to bypass policy or approval;
- allow extensions to mutate canonical session history directly;
- allow arbitrary extension code inside `gestalt-core`;
- replace MCP;
- turn `gestalt.json` into a general workflow language;
- add multi-agent topology to the extension manifest;
- add arbitrary providers to the public process extension protocol;
- guarantee state-preserving process reload;
- expose internal Rust types as permanent wire contracts;
- make every component hot-reloadable in the first implementation;
- allow client/product extensions to execute tools directly.

---

# 6. Design Principles

## 6.1 Package is not interface

An extension package is a distribution unit.

It may contain several components, but those components do not share one universal runtime interface.

## 6.2 Host owns authority

An extension manifest may declare or request capabilities and permissions.

Only the host may:

- enable components;
- grant permissions;
- accept trust;
- select configuration;
- expose client contributions;
- publish runtime generations.

## 6.3 Runtime owns execution

All tools, regardless of origin, pass through the same canonical path:

```text
Tool discovery
    ↓
Tool schema adaptation
    ↓
Model-visible tool catalog
    ↓
Tool-call validation
    ↓
Policy
    ↓
Approval
    ↓
Execution
    ↓
Output shaping
    ↓
Trace
    ↓
Canonical ToolResult
```

## 6.4 Clients control and observe; they do not bypass

Clients use `RuntimeControl` to:

- start or stop sessions;
- send messages;
- respond to approvals;
- inspect runtime state;
- subscribe to events;
- request extension reload.

Clients do not mutate the runtime registry or session history directly.

## 6.5 One turn, one runtime generation

Every turn pins one immutable runtime snapshot from context construction through tool completion and next-turn preparation.

## 6.6 Public DTOs are not core models

Protocol request and response types are explicitly versioned and separate from:

- `Session`;
- `Message`;
- `ContextPacket`;
- `AgentEvent`;
- provider-native types;
- internal tool executor state.

## 6.7 Minimal public protocols

Gestalt should only define protocols for Gestalt-specific behavior.

Use existing protocols where they already solve the problem.

## 6.8 Deterministic composition

Every capability type must define:

- ordering;
- concurrency;
- conflicts;
- failure behavior;
- timeout behavior;
- data scope;
- reduction semantics.

---

# 7. Terminology

## 7.1 Extension package

A versioned, installable bundle with:

- identity;
- compatibility metadata;
- components;
- configuration schema;
- assets;
- requested permissions;
- content hashes.

## 7.2 Extension component

One independently activated capability inside a package.

Examples:

- MCP server;
- command tool;
- lifecycle process;
- native runtime module;
- skill;
- client contribution bundle.

## 7.3 Runtime extension component

A component that contributes behavior to the agent runtime.

## 7.4 Client/product extension component

A component loaded by an application or client host to contribute presentation or user interaction behavior.

## 7.5 Runtime module

A trusted native Rust composition module linked into the embedding binary.

This replaces the current universal use of the name `GestaltExtension` for native composition.

## 7.6 Extension instance

A configured activation of an extension package.

The same package may have several instances with different configuration.

## 7.7 Extension process instance

A running external component process with:

- negotiated protocol version;
- active capabilities;
- process state;
- in-flight request count;
- content fingerprint.

## 7.8 Runtime snapshot

An immutable, executable composition of:

- tools;
- context providers;
- policy guards;
- routers;
- verifiers;
- observers;
- extension instances.

## 7.9 Runtime generation

A monotonically increasing identifier assigned to a published runtime snapshot.

## 7.10 Client host

Any application surface that uses Gestalt through `RuntimeControl`.

Examples include:

- command-line clients;
- desktop clients;
- graphical applications;
- IDE integrations;
- web frontends;
- service APIs;
- embedded product interfaces.

---

# 8. Target Architecture

```text
┌──────────────────────────────────────────────────────────────────────┐
│                        Extension Package                             │
│                                                                      │
│ Identity · version · compatibility · components · schemas · assets   │
└───────────────┬────────────────────────────┬─────────────────────────┘
                │                            │
                ▼                            ▼
┌──────────────────────────────┐  ┌───────────────────────────────────┐
│ Runtime Components           │  │ Client/Product Components         │
│                              │  │                                   │
│ • Native modules             │  │ • Commands                        │
│ • MCP servers                │  │ • Views/panels                     │
│ • Command tools              │  │ • Artifact renderers              │
│ • Lifecycle components       │  │ • Settings contributions          │
│ • Skills                     │  │ • Forms/presentation metadata      │
└──────────────┬───────────────┘  └────────────────┬──────────────────┘
               │                                   │
               ▼                                   ▼
┌──────────────────────────────┐  ┌───────────────────────────────────┐
│ ExtensionManager             │  │ ClientExtensionHost               │
│                              │  │                                   │
│ Discovery                    │  │ Client compatibility              │
│ Launch                       │  │ Contribution registration         │
│ Negotiation                  │  │ Client bundle lifecycle           │
│ Validation                   │  │ Client reload                     │
│ Reload                       │  └────────────────┬──────────────────┘
└──────────────┬───────────────┘                   │
               ▼                                   │
┌──────────────────────────────┐                   │
│ RuntimeSnapshot              │                   │
│                              │                   │
│ ToolCatalog                  │                   │
│ ContextPlan                  │                   │
│ PolicyPlan                   │                   │
│ RoutingPlan                  │                   │
│ VerificationPlan             │                   │
│ ObserverPlan                 │                   │
│ Generation/fingerprint       │                   │
└──────────────┬───────────────┘                   │
               ▼                                   │
┌──────────────────────────────┐                   │
│ AgentRuntime                 │                   │
│                              │                   │
│ Pin snapshot per turn        │                   │
│ Execute canonical paths      │                   │
│ Emit stable runtime events   │                   │
└──────────────┬───────────────┘                   │
               ▼                                   ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         RuntimeControl                               │
│                                                                      │
│ Sessions · messages · approvals · events · artifacts · inspection   │
│ cancellation · configuration · extension reload                     │
└──────────────────────────────────────────────────────────────────────┘
```

---

# 9. Extension Package Model

## 9.1 Package manifest

Every package must contain:

```text
gestalt.extension.toml
```

The package manifest describes package facts, not user-specific runtime choices.

Example:

```toml
manifest_version = 2

[package]
id = "com.example.document-review"
name = "Document Review"
version = "1.2.0"
description = "Document analysis tools, verification, and client contributions"
license = "MIT"

[compatibility]
gestalt = ">=0.4,<0.6"

[[components]]
id = "document-tools"
kind = "mcp-server"
transport = "stdio"

[components.entrypoint]
command = "python"
args = ["-m", "document_review.tools"]

[[components]]
id = "document-lifecycle"
kind = "gestalt-lifecycle"
transport = "stdio"

[components.entrypoint]
command = "python"
args = ["-m", "document_review.lifecycle"]

[[components]]
id = "document-skill"
kind = "skill"
path = "skills/document-review"

[[components]]
id = "document-client"
kind = "client-product"
entrypoint = "client/index.js"

[configuration]
schema = "schemas/config.schema.json"

[requested_permissions]
workspace_read = true
workspace_write = false
network = ["api.example.com"]
```

## 9.2 Supported component kinds

Initial stable component kinds:

```rust
pub enum ExtensionComponentKind {
    NativeRuntimeModule,
    McpServer,
    CommandTool,
    GestaltLifecycle,
    Skill,
    ClientProduct,
}
```

Future component kinds require a manifest schema revision or compatible extension mechanism.

## 9.3 Host compatibility

A component may declare compatible host classes:

```toml
[components.compatibility]
hosts = ["runtime", "client"]
```

Defined host classes:

- `runtime`;
- `client`;
- `remote-worker`;
- `embedded`;
- `cli`.

Hosts may advertise additional capability identifiers, but extensions must not depend on unrecognized free-form host names for core behavior.

## 9.4 Package file layout

Recommended layout:

```text
document-review/
├── gestalt.extension.toml
├── gestalt.extension.lock
├── runtime/
│   ├── tools/
│   ├── lifecycle/
│   └── verifier/
├── client/
│   ├── index.js
│   └── assets/
├── skills/
│   └── document-review/
│       └── SKILL.md
├── schemas/
│   └── config.schema.json
└── README.md
```

The package-local lockfile is optional and belongs to the package build.

The workspace-level `gestalt.lock` records installed package resolution.

---

# 10. Runtime Component Types

## 10.1 Native runtime modules

Trusted Rust modules linked into the embedding binary use:

```rust
pub trait RuntimeModule: Send + Sync {
    fn id(&self) -> &str;

    fn register(
        &self,
        registry: &mut RuntimeRegistryBuilder,
    ) -> Result<()>;
}
```

Characteristics:

- compile-time inclusion;
- no process protocol;
- strong Rust typing;
- intended for built-ins, trusted embedders, and first-party integrations;
- may register providers when the embedding application explicitly supports this;
- not distributed as arbitrary third-party dynamic libraries.

The existing `GestaltExtension` trait should be renamed or superseded by `RuntimeModule` to avoid conflating trusted native composition with installable external extensions.

## 10.2 MCP servers

MCP is the preferred public path for external tools and compatible MCP capabilities.

Gestalt remains responsible for:

- canonical tool IDs;
- tool schema normalization;
- tool-catalog planning;
- policy;
- approval;
- retry eligibility;
- output shaping;
- tracing;
- trust metadata;
- artifact extraction.

MCP owns:

- capability discovery;
- external invocation;
- transport;
- compatible server metadata.

## 10.3 Command tools

Simple custom tools may be declared without implementing an MCP server.

Example:

```toml
[[components]]
id = "word-count"
kind = "command-tool"

[components.tool]
name = "word_count"
description = "Count words in a text file"
read_only = true
idempotent = true

[components.tool.input_schema]
type = "object"
required = ["path"]

[components.tool.input_schema.properties.path]
type = "string"

[components.execution]
command = "python"
args = ["scripts/word_count.py"]
input = "json-stdin"
output = "json-stdout"
```

Command tools must support:

- JSON input on stdin;
- JSON output on stdout;
- structured error output;
- timeout;
- cancellation by process termination;
- bounded output size.

## 10.4 Gestalt lifecycle components

Lifecycle components implement Gestalt-specific runtime capabilities:

- context providers;
- policy guards;
- turn routers;
- verifiers;
- event observers.

They do not implement general-purpose tools.

## 10.5 Skills

Skills remain a distinct concept.

A package may distribute skills, but skills are loaded through the skill system rather than the lifecycle protocol.

---

# 11. Client/Product Extension Components

## 11.1 Purpose

Client/product extension components allow a host application to provide workload-specific user experience without adding graphical or product concepts to the harness.

## 11.2 Supported contribution categories

Initial contribution categories:

```rust
pub enum ClientContributionKind {
    Command,
    View,
    ArtifactRenderer,
    Settings,
    Form,
    PresentationMetadata,
}
```

## 11.3 Client contribution examples

A client component may contribute:

- a command that calls `RuntimeControl`;
- a view displaying runtime events;
- an artifact renderer for a custom MIME type;
- a settings page generated from an extension schema;
- a form that creates a structured runtime request;
- presentation hints for tool results or verification reports.

## 11.4 Prohibited direct access

Client/product components must not directly:

- execute runtime tools;
- register runtime tools;
- mutate canonical session history;
- modify runtime snapshots;
- bypass policy;
- respond to approvals without going through `RuntimeControl`;
- access provider adapters;
- access raw extension process handles.

## 11.5 Client host interface

A generic host-facing API:

```rust
pub trait ClientExtensionHost {
    fn host_capabilities(&self) -> ClientHostCapabilities;

    fn register_command(
        &mut self,
        contribution: CommandContribution,
    ) -> Result<RegistrationHandle>;

    fn register_view(
        &mut self,
        contribution: ViewContribution,
    ) -> Result<RegistrationHandle>;

    fn register_artifact_renderer(
        &mut self,
        contribution: ArtifactRendererContribution,
    ) -> Result<RegistrationHandle>;

    fn register_settings(
        &mut self,
        contribution: SettingsContribution,
    ) -> Result<RegistrationHandle>;
}
```

All registrations return disposable handles.

```rust
pub trait RegistrationHandle {
    fn dispose(self: Box<Self>) -> Result<()>;
}
```

This supports client-side reload and clean deactivation.

## 11.6 Declarative-first client contributions

Prefer declarative contributions where possible:

```json
{
  "commands": [
    {
      "id": "document-review.run",
      "title": "Review Current Document",
      "action": {
        "type": "runtime.request",
        "method": "sessions.start",
        "template": "review-document"
      }
    }
  ],
  "artifactRenderers": [
    {
      "id": "document-review.report",
      "mimeTypes": ["application/vnd.example.review+json"],
      "renderer": "client/renderers/review-report.js"
    }
  ]
}
```

Client code should be required only for behavior that cannot be represented declaratively.

---

# 12. Configuration Architecture

## 12.1 Configuration files

The architecture uses three distinct files:

```text
gestalt.extension.toml
    Package-owned declaration

gestalt.json
    User/workspace/runtime configuration

gestalt.lock
    Generated exact resolution
```

## 12.2 `gestalt.extension.toml`

Owns:

- package identity;
- version;
- compatibility;
- component declarations;
- entrypoints;
- requested permissions;
- configuration schema path;
- package assets.

It does not own:

- whether the package is enabled;
- trust grants;
- user credentials;
- workspace-specific values;
- active profile selection;
- exact installed resolution for the workspace.

## 12.3 `gestalt.json`

Remains the canonical human-editable configuration format.

It owns:

- package activation;
- extension instances;
- enabled components;
- instance-specific configuration;
- runtime grants;
- profile associations;
- host activation settings;
- runtime limits;
- layered overrides.

Example:

```json
{
  "$schema": "https://gestalt.example/schema/gestalt.json",
  "version": 2,

  "extensions": {
    "instances": {
      "primary-document-review": {
        "package": "com.example.document-review",
        "version": "^1.2",
        "enabled": true,

        "components": {
          "document-tools": true,
          "document-lifecycle": true,
          "document-skill": true,
          "document-client": true
        },

        "config": {
          "language": "en",
          "severityThreshold": "medium"
        },

        "grants": {
          "workspaceRead": true,
          "workspaceWrite": false,
          "network": ["api.example.com"]
        }
      }
    }
  }
}
```

## 12.4 Extension instances

Instance IDs are separate from package IDs.

This allows multiple configurations of the same package:

```json
{
  "extensions": {
    "instances": {
      "review-policy-a": {
        "package": "com.example.document-review",
        "config": {
          "policySet": "a"
        }
      },
      "review-policy-b": {
        "package": "com.example.document-review",
        "config": {
          "policySet": "b"
        }
      }
    }
  }
}
```

Every runtime component registration must include both:

- package ID;
- instance ID.

Canonical extension component identity:

```text
extension:<package-id>:<instance-id>:<component-id>
```

## 12.5 `gestalt.lock`

The lockfile records:

- exact package version;
- package source;
- content hash;
- manifest hash;
- resolved dependencies;
- compatibility metadata;
- optionally resolved component fingerprints.

Example:

```json
{
  "version": 1,
  "packages": {
    "com.example.document-review": {
      "version": "1.2.4",
      "source": "registry:default",
      "contentHash": "sha256:...",
      "manifestHash": "sha256:..."
    }
  }
}
```

The lockfile is generated and must not contain secrets.

## 12.6 Extension configuration schemas

Each package may expose:

```toml
[configuration]
schema = "schemas/config.schema.json"
```

The host must:

1. load the schema;
2. resolve layered extension configuration;
3. validate the effective configuration;
4. reject unknown or invalid fields unless the schema explicitly allows them;
5. pass only validated configuration to components;
6. expose validation diagnostics with source provenance.

The same schema may drive:

- CLI validation;
- generated documentation;
- client settings forms;
- remote worker input validation;
- migration tooling.

## 12.7 Host-specific settings

Portable runtime configuration belongs in `gestalt.json`.

Purely presentational client preferences should live under a host namespace or in host-local settings.

Example:

```json
{
  "hosts": {
    "client": {
      "extensions": {
        "primary-document-review": {
          "defaultView": "summary",
          "showDetailedTrace": false
        }
      }
    }
  }
}
```

Host-specific settings must not affect tool authority, policy, or runtime execution unless they are promoted into validated runtime configuration.

## 12.8 Permissions and grants

The package manifest may request permissions:

```toml
[requested_permissions]
workspace_read = true
network = ["api.example.com"]
```

`gestalt.json` or managed policy grants them:

```json
{
  "extensions": {
    "instances": {
      "primary-document-review": {
        "grants": {
          "workspaceRead": true,
          "network": []
        }
      }
    }
  }
}
```

Effective authority is the intersection of:

```text
Package request
∩ user/workspace grant
∩ host policy
∩ managed policy
∩ runtime execution policy
```

A package request never grants authority by itself.

## 12.9 Merge semantics

Extension instances merge by instance ID.

Rules:

- missing field: inherit;
- scalar: higher-precedence value replaces;
- object: merge recursively;
- component activation: explicit boolean override;
- arrays: replace by default;
- permission grants: monotonic narrowing;
- unknown fields: error;
- raw secrets: prohibited;
- credentials: referenced through credential handles only.

---

# 13. RuntimeControl API

## 13.1 Purpose

`RuntimeControl` is the stable boundary between the runtime and any client host.

It prevents clients and client/product extensions from depending on runtime internals.

## 13.2 Interface

```rust
#[async_trait]
pub trait RuntimeControl: Send + Sync {
    async fn start_session(
        &self,
        request: StartSessionRequest,
    ) -> Result<SessionHandle>;

    async fn send_message(
        &self,
        request: SendMessageRequest,
    ) -> Result<MessageQueueAck>;

    async fn cancel_session(
        &self,
        session_id: SessionId,
    ) -> Result<()>;

    async fn respond_to_approval(
        &self,
        response: ApprovalResponse,
    ) -> Result<()>;

    async fn inspect_runtime(
        &self,
    ) -> Result<RuntimeInspection>;

    async fn reload_extensions(
        &self,
        request: ReloadRequest,
    ) -> Result<ReloadReport>;

    async fn list_artifacts(
        &self,
        request: ArtifactListRequest,
    ) -> Result<Vec<ArtifactDescriptor>>;

    fn subscribe(
        &self,
        filter: RuntimeEventFilter,
    ) -> RuntimeEventStream;
}
```

## 13.3 Transport independence

`RuntimeControl` is an in-process Rust trait first.

Optional transports may later expose it over:

- stdio;
- Unix socket;
- named pipe;
- WebSocket;
- HTTP.

The in-process contract remains authoritative.

## 13.4 Runtime event projection

Clients should consume stable client-facing runtime event envelopes rather than raw internal events.

```rust
pub struct RuntimeEventEnvelopeV1 {
    pub sequence: u64,
    pub timestamp: Timestamp,
    pub runtime_generation: RuntimeGeneration,
    pub session_id: Option<SessionId>,
    pub kind: RuntimeEventKindV1,
    pub payload: serde_json::Value,
}
```

Internal events may be richer.

The runtime maps them into stable client projections.

---

# 14. Lifecycle Protocol

## 14.1 Scope

The Gestalt-specific lifecycle protocol owns only:

- context providers;
- policy guards;
- turn routers;
- verifiers;
- event observers.

It does not own:

- package installation;
- client views;
- general tool execution;
- provider-native APIs;
- session storage;
- arbitrary runtime mutation.

## 14.2 Transport

Initial transport:

```text
JSON-RPC 2.0 over newline-delimited stdio
```

Future transports require no semantic protocol redesign.

## 14.3 Minimal method set

```text
initialize
capabilities/describe
lifecycle/invoke
shutdown
$/cancelRequest
```

## 14.4 Initialize

Host request:

```json
{
  "jsonrpc": "2.0",
  "id": "init-1",
  "method": "initialize",
  "params": {
    "protocolVersions": ["2.0"],
    "packageId": "com.example.document-review",
    "instanceId": "primary-document-review",
    "componentId": "document-lifecycle",
    "hostCapabilities": {
      "cancellation": true,
      "eventBatching": true
    },
    "config": {
      "language": "en"
    }
  }
}
```

Extension response:

```json
{
  "jsonrpc": "2.0",
  "id": "init-1",
  "result": {
    "protocolVersion": "2.0",
    "capabilities": [
      "context-provider",
      "policy-guard",
      "verifier"
    ]
  }
}
```

## 14.5 Capability description

Request:

```json
{
  "jsonrpc": "2.0",
  "id": "describe-1",
  "method": "capabilities/describe"
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": "describe-1",
  "result": {
    "contextProviders": [
      {
        "id": "document-metadata",
        "priority": 50,
        "dataScope": [
          "workspace.metadata",
          "turn.lastUserMessage"
        ]
      }
    ],
    "policyGuards": [
      {
        "id": "protected-document-guard",
        "priority": 100,
        "failureMode": "closed",
        "timeoutMs": 5000
      }
    ],
    "verifiers": [
      {
        "id": "citation-verifier",
        "priority": 50
      }
    ]
  }
}
```

The host validates that runtime descriptors are compatible with the package manifest.

## 14.6 Lifecycle invocation

Request:

```json
{
  "jsonrpc": "2.0",
  "id": "invoke-1",
  "method": "lifecycle/invoke",
  "params": {
    "invocationId": "inv-123",
    "capability": "policy-guard",
    "handlerId": "protected-document-guard",
    "payload": {
      "tool": {
        "id": "builtin:write",
        "risk": "high"
      },
      "input": {
        "path": "contracts/final.pdf"
      }
    }
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": "invoke-1",
  "result": {
    "decision": "require-approval",
    "reason": "The target is marked as a protected document"
  }
}
```

## 14.7 Data minimization

Handlers declare required data scopes.

The host sends only the stable projection needed by the capability.

Do not send raw:

- full session objects;
- full internal history by default;
- internal context packets;
- provider request structures;
- process handles;
- client state.

Example DTO:

```rust
pub struct PolicyGuardRequestV1 {
    pub session_id: PublicSessionId,
    pub turn_index: u32,
    pub tool: ToolSummaryV1,
    pub input: serde_json::Value,
    pub policy_context: PolicyContextSummaryV1,
}
```

## 14.8 Capability-specific responses

Do not use one universal hook outcome.

### Context provider

```rust
pub struct ContextProviderResponseV1 {
    pub contributions: Vec<ContextContributionV1>,
}
```

### Policy guard

```rust
pub enum PolicyGuardDecisionV1 {
    Abstain,
    Annotate { metadata: serde_json::Value },
    RequireApproval { reason: String },
    Deny { reason: String },
}
```

### Turn router

```rust
pub enum TurnRouteAdviceV1 {
    NoChange,
    Route {
        model: String,
        provider: Option<String>,
        variant: Option<String>,
    },
    Stop {
        reason: String,
    },
}
```

### Verifier

```rust
pub struct VerificationReportV1 {
    pub status: VerificationStatus,
    pub findings: Vec<VerificationFindingV1>,
}
```

### Event observer

```rust
pub struct EventObserverAckV1 {
    pub accepted_through_sequence: u64,
}
```

---

# 15. Composition Semantics

## 15.1 Context providers

Execution:

- concurrent where safe;
- bounded by per-handler and aggregate timeout;
- results sorted deterministically.

Sort order:

```text
priority descending
package ID ascending
instance ID ascending
handler ID ascending
contribution ID ascending
```

The host assigns:

- final trust;
- final context placement;
- effective priority;
- token-budget eligibility.

Extensions may request priority but cannot self-elevate trust.

## 15.2 Policy guards

Reduction:

```text
Deny
> RequireApproval
> Annotate
> Abstain
```

Rules:

- base policy always remains authoritative;
- an extension cannot override a denial;
- all denials and approval requests are recorded;
- guards run exactly once per policy decision;
- guard failures follow declared and host-approved failure mode.

## 15.3 Turn routers

Rules:

1. highest-priority `Stop` wins;
2. otherwise highest-priority valid `Route` wins;
3. equal-priority conflicting routes produce a deterministic conflict;
4. invalid routes are ignored and traced;
5. the runtime remains authoritative.

## 15.4 Verifiers

All verifier reports are collected.

They are not reduced through last-writer-wins.

The host or configured verification policy determines whether findings:

- block completion;
- request revision;
- annotate output;
- remain informational.

## 15.5 Event observers

Observers are non-authoritative.

They:

- receive stable event batches;
- cannot block execution;
- cannot mutate runtime state;
- may be dropped or disabled after repeated failure;
- must expose delivery lag through diagnostics.

---

# 16. Runtime Registry and Snapshot Model

## 16.1 Registry builder

```rust
pub struct RuntimeRegistryBuilder {
    tools: BTreeMap<CanonicalToolId, ToolRegistration>,
    context_providers: BTreeMap<HandlerId, ContextProviderRegistration>,
    policy_guards: Vec<PolicyGuardRegistration>,
    turn_routers: Vec<TurnRouterRegistration>,
    verifiers: Vec<VerifierRegistration>,
    observers: Vec<EventObserverRegistration>,
    runtime_modules: Vec<RuntimeModuleDescriptor>,
}
```

The builder is mutable and temporary.

## 16.2 Immutable snapshot

```rust
pub struct RuntimeExtensionSnapshot {
    pub generation: RuntimeGeneration,
    pub fingerprint: RuntimeFingerprint,

    pub tool_catalog: Arc<dyn ToolCatalog>,
    pub context_plan: Arc<ContextProviderPlan>,
    pub policy_plan: Arc<PolicyGuardPlan>,
    pub routing_plan: Arc<TurnRoutingPlan>,
    pub verification_plan: Arc<VerificationPlan>,
    pub observer_plan: Arc<EventObserverPlan>,

    pub extension_instances: Vec<Arc<ExtensionProcessInstance>>,
    pub component_descriptors: Arc<[ResolvedComponentDescriptor]>,
}
```

Published snapshots are immutable.

## 16.3 Turn pinning

At the pre-context safe point:

```rust
let runtime_snapshot = extension_manager.current_snapshot();

let turn_runtime = TurnRuntime {
    snapshot: runtime_snapshot,
    // ...
};
```

The turn uses the same snapshot for:

- context contributors;
- model-visible tool schemas;
- tool-name resolution;
- policy guards;
- tool execution adapters;
- verifiers;
- turn routers.

## 16.4 Trace identity

Every turn must record:

- runtime generation;
- runtime fingerprint;
- active extension package IDs and versions;
- active component IDs;
- tool catalog fingerprint;
- lifecycle plan fingerprint.

---

# 17. ExtensionManager

## 17.1 Responsibilities

```rust
pub struct ExtensionManager {
    discovery: Arc<dyn ExtensionDiscovery>,
    package_resolver: Arc<dyn ExtensionPackageResolver>,
    launcher: Arc<dyn ExtensionLauncher>,
    active: Arc<ArcSwap<RuntimeExtensionSnapshot>>,
    instances: Mutex<BTreeMap<ComponentInstanceId, Arc<ExtensionProcessInstance>>>,
    reload_lock: tokio::sync::Mutex<()>,
    event_bus: RuntimeEventBus,
}
```

Responsibilities:

- discover packages;
- resolve enabled instances;
- validate manifests;
- validate configuration;
- launch processes;
- negotiate protocols;
- build candidate registries;
- publish runtime snapshots;
- coordinate draining;
- shut down removed components;
- report diagnostics.

## 17.2 Launcher abstraction

```rust
#[async_trait]
pub trait ExtensionLauncher: Send + Sync {
    async fn launch(
        &self,
        component: &ResolvedRuntimeComponent,
    ) -> Result<Arc<ExtensionProcessInstance>>;
}
```

Initial implementation:

```rust
LocalProcessLauncher
```

Future implementations may include:

- sandboxed local launcher;
- container launcher;
- remote launcher.

This feature does not implement those future launchers.

---

# 18. Hot Reload

## 18.1 Reload invariant

> Reload never mutates the runtime snapshot currently pinned by an active turn.

## 18.2 Transactional reload flow

```text
1. Acquire reload lock
2. Rediscover packages
3. Resolve extension instances
4. Validate manifests and configuration
5. Compute component diff
6. Reuse unchanged component instances
7. Launch changed and added runtime components
8. Negotiate protocols
9. Validate capability descriptors
10. Build candidate registry
11. Validate duplicates, conflicts, and compatibility
12. Construct candidate runtime snapshot
13. Validate client/product components with each client host
14. Atomically publish runtime snapshot
15. Activate new client contributions
16. Mark replaced components as draining
17. Dispose replaced client contributions
18. Stop old runtime instances after in-flight calls drain
19. Emit reload report
```

If any step before publication fails:

- destroy candidate processes;
- keep the current snapshot active;
- leave active client contributions unchanged;
- emit a failed reload report.

## 18.3 Blue/green model

```text
Generation N
    ├── continues serving active turns
    │
    └── candidate N+1 is built separately
                         │
                         ├── validation failure
                         │       └── destroy N+1, keep N
                         │
                         └── validation success
                                 ├── publish N+1
                                 ├── new turns use N+1
                                 └── N drains and shuts down
```

## 18.4 Process state

```rust
pub enum ExtensionProcessState {
    Starting,
    Ready,
    Draining,
    Stopping,
    Stopped,
    Failed {
        reason: String,
    },
}
```

## 18.5 In-flight behavior

A draining instance:

- accepts no new calls;
- continues existing calls;
- shuts down when in-flight count reaches zero;
- may be force-terminated after a configured grace period.

## 18.6 Component fingerprints

Fingerprint inputs:

```text
package manifest hash
+ package version
+ component declaration
+ entrypoint content hash
+ dependency lock hash
+ validated instance configuration fingerprint
+ protocol version
+ relevant host compatibility data
```

Diff classification:

```rust
pub enum ComponentDiff {
    Unchanged,
    Added,
    Removed,
    CodeChanged,
    ConfigurationChanged,
    CompatibilityChanged,
}
```

Unchanged processes should be reused.

## 18.7 Manual reload

Required commands:

```text
gestalt extension reload
gestalt extension reload <instance-id>
gestalt extension reload --dry-run
gestalt extension reload --force
```

`--dry-run` returns:

- package diff;
- component diff;
- expected restarts;
- configuration errors;
- compatibility errors;
- expected runtime fingerprint.

## 18.8 Development watch

Optional development command:

```text
gestalt extension dev <path>
```

Behavior:

- watch manifest, schema, source, and assets;
- debounce changes;
- ignore temporary and build output files;
- build a candidate generation;
- keep the active generation when validation fails;
- surface structured diagnostics.

Automatic production watching is not enabled by default.

## 18.9 Client reload

Client/product components reload independently from runtime components.

Client reload must:

- validate the candidate bundle;
- dispose prior registrations;
- activate candidate registrations;
- restore only explicitly serializable UI state;
- preserve runtime state.

A client reload failure must not invalidate the runtime snapshot.

## 18.10 Coordinated package reload

When both runtime and client components change:

1. validate runtime candidate;
2. validate client candidate;
3. publish runtime candidate;
4. activate client candidate;
5. drain old runtime components;
6. dispose old client contributions.

If a client candidate fails before runtime publication, the package reload fails.

If client activation fails after runtime publication, the host must:

- report partial activation failure;
- disable the failed client component;
- keep the valid runtime generation;
- avoid rolling back active runtime turns.

## 18.11 State transfer

Initial reload behavior is process restart.

Durable extension state must live outside the process.

Generic state export/import is deferred.

---

# 19. Client/Product Extension Activation

## 19.1 Host capability negotiation

A client host advertises:

```rust
pub struct ClientHostCapabilities {
    pub api_version: String,
    pub contribution_kinds: BTreeSet<ClientContributionKind>,
    pub supported_artifact_mime_types: BTreeSet<String>,
    pub supports_hot_reload: bool,
    pub supports_custom_code: bool,
}
```

A client component is activated only when compatibility requirements are satisfied.

## 19.2 Runtime linkage

Client contributions identify runtime instances using stable public IDs.

They do not hold pointers to runtime internals.

Example:

```json
{
  "command": {
    "id": "document-review.run",
    "runtimeAction": {
      "method": "sessions.start",
      "extensionInstance": "primary-document-review",
      "template": "document-review"
    }
  }
}
```

## 19.3 Client code isolation

Client code executes under the client host's extension model.

The runtime does not assume:

- JavaScript;
- WebAssembly;
- React;
- Tauri;
- browser APIs;
- native UI plugins.

Each client host chooses its implementation while preserving contribution semantics.

---

# 20. Failure Semantics

## 20.1 Package failure

Invalid package manifests are rejected before component launch.

## 20.2 Component launch failure

A required component failure rejects the candidate instance.

An optional component failure disables only that component and records degraded status.

## 20.3 Lifecycle failure

Each handler declares a requested failure mode:

```rust
pub enum LifecycleFailureMode {
    Open,
    Closed,
    DisableHandler,
}
```

The host may override the requested mode with a stricter mode.

## 20.4 Tool failure

Tool failures remain canonical tool execution failures and are handled through existing tool result semantics.

## 20.5 Client failure

A failed client contribution:

- is disposed;
- is marked disabled;
- cannot affect runtime execution;
- emits a client-extension diagnostic.

## 20.6 Reload failure

Candidate failure preserves the active generation.

Reload reports include:

- stage;
- package;
- instance;
- component;
- error category;
- retryability;
- active generation;
- candidate fingerprint.

---

# 21. Observability

## 21.1 Runtime events

Required events:

```rust
ExtensionDiscoveryStarted
ExtensionPackageDiscovered
ExtensionPackageRejected
ExtensionComponentStarting
ExtensionComponentReady
ExtensionComponentFailed
ExtensionReloadStarted
ExtensionReloadCandidateBuilt
ExtensionReloadFailed
ExtensionRuntimePublished
ExtensionComponentDraining
ExtensionComponentStopped
ClientExtensionActivated
ClientExtensionFailed
ClientExtensionDisposed
RuntimeGenerationAdopted
```

## 21.2 Reload report

```rust
pub struct ReloadReport {
    pub previous_generation: RuntimeGeneration,
    pub active_generation: RuntimeGeneration,
    pub previous_fingerprint: RuntimeFingerprint,
    pub active_fingerprint: RuntimeFingerprint,
    pub packages: Vec<PackageReloadResult>,
    pub components: Vec<ComponentReloadResult>,
    pub warnings: Vec<Diagnostic>,
    pub success: bool,
}
```

## 21.3 Inspection

Required inspection commands:

```text
gestalt extension list
gestalt extension inspect <instance-id>
gestalt extension validate <path>
gestalt extension doctor <instance-id>
gestalt runtime inspect
gestalt runtime extensions
gestalt runtime generation
```

Inspection must show:

- package source;
- exact version;
- package hash;
- active components;
- process state;
- protocol version;
- requested permissions;
- granted permissions;
- configuration fingerprint;
- runtime generation;
- in-flight requests;
- last failure;
- reload eligibility.

---

# 22. Security Readiness

Sandboxing is deferred, but this architecture must establish the insertion points now.

Required boundaries:

- `ExtensionLauncher`;
- host-owned grants;
- requested versus granted permission model;
- data-scope declarations;
- process environment construction;
- bounded protocol messages;
- timeouts;
- cancellation;
- package hashes;
- exact resolved versions;
- extension identity in every trace;
- no client-to-tool bypass.

The absence of sandboxing must be visible in diagnostics.

A local unsandboxed extension must never be described as isolated.

---

# 23. Package and Protocol Versioning

Independent version domains:

```text
gestalt.json schema version
extension manifest version
extension lifecycle protocol version
client contribution API version
runtime event projection version
trace format version
lockfile version
```

They must not share one version number.

Compatibility examples:

```toml
[compatibility]
gestalt = ">=0.4,<0.6"
lifecycle_protocol = "^2.0"
client_api = "^1.0"
```

Unknown required capabilities reject activation.

Unknown optional capabilities are ignored with diagnostics.

---

# 24. Developer Tooling

Required commands:

```text
gestalt extension init
gestalt extension validate
gestalt extension inspect
gestalt extension dev
gestalt extension test
gestalt extension doctor
gestalt extension reload
gestalt extension package
gestalt extension lock
```

## 24.1 `extension init`

Templates:

```text
command-tool
mcp-server
lifecycle-python
lifecycle-typescript
lifecycle-rust
client-product
combined-package
```

## 24.2 `extension test`

Must support:

- initialize handshake;
- capability description validation;
- fixture invocation;
- malformed response testing;
- timeout behavior;
- cancellation;
- oversized message rejection;
- reload compatibility;
- configuration-schema fixtures.

## 24.3 SDKs

Recommended SDKs:

```text
gestalt-extension-sdk-rust
gestalt-lifecycle-sdk-python
@gestalt/lifecycle-sdk
```

The protocol specification remains authoritative.

SDKs must not introduce behavior absent from the protocol.

---

# 25. Migration From Current Architecture

## Phase 1: Terminology and internal separation

1. Introduce `RuntimeModule`.
2. Deprecate `GestaltExtension` as the universal external concept.
3. Add `ExtensionPackage`, `ExtensionComponent`, and `ExtensionInstance`.
4. Split process transport from tool, context, and hook adapters.
5. Preserve existing behavior through adapters.

## Phase 2: Package manifest v2

1. Add component-based manifest schema.
2. Support current manifests through a v1-to-v2 loader.
3. Map existing:
   - tools to command-tool or legacy process-tool components;
   - context injectors to context-provider handlers;
   - hooks to lifecycle handlers.
4. Add extension configuration schema support.

## Phase 3: Lifecycle protocol v2

1. Add `initialize`.
2. Add `capabilities/describe`.
3. Add `lifecycle/invoke`.
4. Add typed DTOs.
5. Add data-scope projection.
6. Add deterministic reducers.
7. Preserve v1 process extensions behind a compatibility client.

## Phase 4: External tool simplification

1. Promote MCP as the preferred external tool protocol.
2. Add command-tool adapter.
3. Mark proprietary process `tools/call` as legacy for new extensions.
4. Preserve existing process tools during migration.

## Phase 5: Runtime snapshots

1. Add `RuntimeRegistryBuilder`.
2. Add `RuntimeExtensionSnapshot`.
3. Pin one snapshot per turn.
4. Add runtime generation and fingerprint tracing.
5. Eliminate mutable registry access during execution.

## Phase 6: ExtensionManager and reload

1. Add package discovery and resolution.
2. Add component fingerprints.
3. Add candidate runtime build.
4. Add atomic snapshot publication.
5. Add process draining.
6. Replace count-only reload behavior with real reload.

## Phase 7: RuntimeControl

1. Introduce in-process `RuntimeControl`.
2. Route CLI and other clients through it where practical.
3. Add stable client event projections.
4. Add extension reload and inspection APIs.

## Phase 8: Client/product extension host

1. Define client contribution contracts.
2. Add disposable registration handles.
3. Add host capability negotiation.
4. Add independent client reload.
5. Add generated settings integration.

---

# 26. Testing Strategy

## 26.1 Unit tests

- package manifest validation;
- instance ID resolution;
- component identity generation;
- configuration merging;
- permission intersection;
- component fingerprinting;
- composition reducers;
- protocol DTO serialization;
- compatibility negotiation;
- runtime snapshot immutability.

## 26.2 Integration tests

- mixed native, MCP, command, and lifecycle components;
- multiple instances of one package;
- different host component selections;
- failed process initialization;
- lifecycle timeout;
- policy guard denial;
- client activation failure;
- package configuration validation;
- runtime generation adoption.

## 26.3 Reload tests

- unchanged component reuse;
- one changed component;
- configuration-only change;
- added component;
- removed component;
- invalid candidate manifest;
- candidate protocol mismatch;
- in-flight tool call during reload;
- active turn during reload;
- client-only reload;
- combined runtime/client reload;
- forced drain timeout;
- reload rollback before publication.

## 26.4 Golden trace tests

Golden traces must include:

- active runtime generation;
- component identities;
- lifecycle decisions;
- extension failures;
- reload publication;
- turn generation adoption;
- draining and shutdown.

## 26.5 Compatibility tests

- current manifest support;
- current protocol support;
- manifest v2 with lifecycle v2;
- future optional capability ignored;
- unknown required capability rejected;
- stale lockfile detection.

---

# 27. Acceptance Criteria

## 27.1 Architecture

- `gestalt-core` has no dependency on extension manifests, package discovery, process launching, client extensions, or `gestalt.json`.
- `AgentLoop` has no extension-package-specific branch.
- Native runtime modules and external extension packages are distinct concepts.
- Runtime and client/product components use separate host APIs.
- One package can contain both component categories.

## 27.2 Configuration

- `gestalt.json` activates and configures extension instances.
- `gestalt.extension.toml` declares package facts and components.
- `gestalt.lock` records exact package resolution.
- Extension configuration is validated against package-provided JSON Schema.
- Requested permissions and granted permissions are distinct.
- The same package can be instantiated more than once.

## 27.3 Protocol

- General external tools can use MCP.
- Simple tools can use the command-tool adapter.
- The lifecycle protocol does not include general-purpose tool calls.
- Lifecycle protocol v2 uses typed capability-specific DTOs.
- Public protocol DTOs do not expose internal session or context types.
- Composition semantics are deterministic.

## 27.4 Runtime

- Runtime composition produces an immutable snapshot.
- Every turn pins one runtime generation.
- Model-visible tools and executed tools come from the same generation.
- Runtime generation and fingerprint appear in traces.
- Clients interact through `RuntimeControl`.

## 27.5 Reload

- Reload builds and validates a candidate before publication.
- Failed candidates do not change the active runtime.
- Publication is atomic.
- Active turns continue on the old generation.
- New turns use the new generation.
- Replaced processes drain before shutdown.
- Unchanged component processes are reused.
- Client/product contributions can reload independently.

## 27.6 Developer experience

- A command tool can be built without implementing JSON-RPC.
- A lifecycle extension can be scaffolded in at least Rust, Python, and TypeScript.
- Validation and conformance commands exist.
- Reload diagnostics identify the failing package, component, and stage.

---

# 28. Open Questions

These questions may be resolved during implementation planning:

1. Should extension instance configuration be allowed inside profiles, or should profiles only select instance IDs?
2. Should client contribution bundles be declarative-only initially?
3. Should package dependencies be supported in manifest v2 or deferred?
4. Should client/product components share the package version exactly, or allow independent component versions?
5. Should configuration migrations execute as extension code or host-declared transformations?
6. Should remote lifecycle components use the same protocol over another transport or require a separate worker protocol?
7. How should optional components affect package health and reload success?
8. Should MCP servers remain configured directly under `mcp`, be activatable through extension packages, or support both?
9. At what release should legacy process `tools/call` stop being recommended for new extensions?
10. Which runtime events belong in the stable client event projection v1?

---

# 29. Architectural Decisions Summary

| ID    | Decision                                                     |
| ----- | ------------------------------------------------------------ |
| AD-1  | An extension is a package, not one universal interface.      |
| AD-2  | Runtime components and client/product components have separate hosts and APIs. |
| AD-3  | Native Rust composition uses `RuntimeModule`.                |
| AD-4  | MCP is the preferred external tool protocol.                 |
| AD-5  | Simple tools may use a declarative command adapter.          |
| AD-6  | Gestalt's lifecycle protocol is limited to context, policy, routing, verification, and observation. |
| AD-7  | Public protocol DTOs are versioned separately from core models. |
| AD-8  | `gestalt.json` configures extension instances.               |
| AD-9  | `gestalt.extension.toml` declares package identity and components. |
| AD-10 | `gestalt.lock` records exact package resolution.             |
| AD-11 | Requested permissions never grant authority.                 |
| AD-12 | Clients use `RuntimeControl` rather than runtime internals.  |
| AD-13 | Runtime composition is represented by immutable snapshots.   |
| AD-14 | Each turn pins one runtime generation.                       |
| AD-15 | Reload uses candidate validation and atomic publication.     |
| AD-16 | Client reload is independent from runtime process reload.    |
| AD-17 | Initial process reload is stateless; generic state transfer is deferred. |
| AD-18 | Sandboxing is deferred, but launcher and permission boundaries are established now. |

---

# 30. Final Outcome

After this feature is implemented, Gestalt will remain a focused agent harness while becoming adaptable enough to serve as:

- a standalone runtime;
- an embedded agent engine;
- a local or remote worker;
- a headless service component;
- the execution layer behind arbitrary client applications;
- the foundation for workload-specific extension ecosystems.

The key invariant is:

> The harness owns execution, policy, context, trace, and runtime composition. Hosts own presentation. Extension packages may distribute both kinds of capability, but their APIs and authority remain separate.