---

title: "feat: Workspace Initialization and Persistent Memory Context"
status: proposed
type: feat
depth: deep
scope:

  - gestalt-cli
  - gestalt-app
  - gestalt-runtime
  - gestalt-core

---

# feat: Workspace Initialization and Persistent Memory Context

## Summary

Formalize workspace initialization, workspace instructions, and persistent memory as first-class but independently configurable harness capabilities.

Gestalt currently loads two workspace files:

* `.gestalt/workspace.md` as trusted project instructions.
* `.gestalt/memory.md` as trusted persistent context.

Both are currently converted directly into `Message::System`, treated as critical context, and never trimmed. Their paths are configurable, but neither capability can be explicitly disabled. Missing files are silently skipped during runtime loading while diagnostic commands report them as missing.

This feature preserves the files-first design while introducing clearer semantics:

* `workspace.md` is a stable, high-authority workspace instruction source.
* `memory.md` is user-approved contextual state, not an instruction source.
* Both files are implemented as first-party context contributors.
* Both contributors can be enabled, disabled, required, or replaced.
* Workspace instructions remain indivisible critical context.
* General memory becomes budget-aware and trim-eligible.
* Pinned memory may remain high-priority.
* All loading, skipping, failures, hashes, and context decisions become observable.
* Memory updates pass through a dedicated proposal and approval lifecycle.
* Workspace initialization remains a CLI/runtime concern and does not enter the core agent loop.

The feature must preserve Gestalt’s existing architectural invariants:

* `gestalt-core` remains I/O-free.
* The `AgentLoop` remains unaware of workspace file paths.
* Context compilation remains deterministic.
* Host policy always outranks workspace instructions and memory.
* Context provenance survives through compilation and tracing.
* Prompt-cache stability is preserved through session snapshots.
* Memory cannot grant tools or permissions.
* Missing optional files do not constitute runtime failure.
* Existing configurations remain valid during migration.

---

# 1. Problem Statement

Gestalt’s current workspace context implementation is simple and useful, but it conflates two semantically different sources.

## 1.1 Workspace instructions and memory have different authority

`workspace.md` defines project-level instructions such as:

* Project goals.
* Output standards.
* Tone.
* Terminology.
* Development constraints.
* Source requirements.

`memory.md` contains persistent facts and decisions accumulated across sessions, such as:

* Prior architectural decisions.
* User preferences.
* Known project facts.
* Historical notes.
* Previously accepted conclusions.

These sources must not receive identical treatment.

Workspace instructions are instructions.

Memory is contextual information that may become stale, conflict with newer evidence, or be irrelevant to the current task.

## 1.2 Memory is currently unbounded critical context

Treating the entire memory file as never-trimmed context causes:

* Permanent token overhead on every turn.
* Reduced room for the current request and recent history.
* Poor scaling for long-lived workspaces.
* Increased interference from stale facts.
* Frequent invalidation of provider prompt caches.
* Pressure on users to avoid using memory.

## 1.3 Missing and unreadable files are conflated

The current loading pattern silently ignores all file read failures.

This makes the following cases indistinguishable:

* File does not exist.
* File is unreadable.
* Path resolves to a directory.
* File contains invalid UTF-8.
* File exceeds practical limits.
* Filesystem access failed.
* Path canonicalization failed.

Only an optional missing file should be silently skippable.

## 1.4 File conventions are leaking into the harness abstraction

Markdown files are an appropriate default implementation, but the abstract harness capability should not require those exact files.

Embedding applications may use:

* Database-backed memory.
* GUI-managed instructions.
* Remote enterprise context.
* In-memory test contributors.
* No persistent memory.
* Custom context contributors.

The runtime must support these implementations without changing the agent loop.

## 1.5 Diagnostics do not reflect configuration intent

A missing optional file should not always produce a warning.

Diagnostics must distinguish:

* Optional and absent.
* Disabled.
* Required and absent.
* Present but unreadable.
* Present but invalid.
* Oversized.
* Outside allowed workspace boundaries.
* Never initialized.
* Previously initialized and now incomplete.

---

# 2. Goals

This feature must:

1. Preserve `.gestalt/workspace.md` as the default project instruction file.
2. Preserve `.gestalt/memory.md` as the default transparent memory backend.
3. Represent workspace instructions and memory as distinct context kinds.
4. Make each contributor independently configurable.
5. Allow both contributors to be explicitly disabled.
6. Support required and optional file semantics.
7. Replace broad silent file-read failure handling with typed outcomes.
8. Preserve source provenance through context compilation and tracing.
9. Apply explicit size and token limits.
10. Snapshot stable context at session initialization.
11. Keep workspace instructions stable across the session.
12. Make general memory budget-aware.
13. Support pinned memory with elevated priority.
14. Require a proposal and approval lifecycle for memory mutation.
15. Make `status` informational and `doctor` configuration-aware.
16. Keep all file I/O outside `gestalt-core`.
17. Avoid modifications to the sacred turn loop unless strictly required.
18. Reject removed `workspace_file` and `memory_file` aliases.

---

# 3. Non-Goals

This feature does not implement:

* Semantic vector retrieval over memory.
* Automatic embeddings.
* A hidden memory database.
* Cross-device synchronization.
* Shared team memory.
* Per-user identity memory.
* Autonomous memory acceptance.
* Automatic contradiction resolution using an LLM.
* Remote workspace storage.
* Workspace sandboxing.
* A full workspace GUI.
* Automatic trust of cloned repository instructions.
* Knowledge graph construction.
* Memory expiry based on model inference.
* Multi-agent memory coordination.

These capabilities may be added later behind the contributor and memory-store interfaces.

---

# 4. Architectural Decisions

## AD-1: Workspace initialization is a CLI/runtime capability

`gestalt init`, `gestalt status`, and `gestalt doctor` belong outside `gestalt-core`.

Responsibilities:

```text
gestalt-cli
  ├── init command
  ├── status command
  ├── doctor command
  └── user-facing diagnostics

gestalt-runtime
  ├── workspace resolution
  ├── contributor construction
  ├── workspace trust state
  └── session snapshot creation

gestalt-context
  ├── context item classification
  ├── priority assignment
  ├── budget allocation
  └── rendering

gestalt-trace
  └── context load and memory mutation audit events

gestalt-core
  ├── provider-neutral context types
  ├── session state
  └── agent loop
```

The agent loop must never:

* Resolve `.gestalt` paths.
* Read Markdown files.
* Parse memory sections.
* Create workspace directories.
* Decide whether a workspace file is required.

## AD-2: Markdown files are default contributors, not hard-coded primitives

Introduce or formalize two first-party context contributors:

```rust
WorkspaceInstructionContributor
MarkdownMemoryContributor
```

Both implement the existing context contribution boundary.

Conceptual interface:

```rust
#[async_trait]
pub trait ContextContributor: Send + Sync {
    fn descriptor(&self) -> ContextContributorDescriptor;

    async fn contribute(
        &self,
        context: &ContextBuildContext,
    ) -> Result<Vec<ContextItem>, ContextContributionError>;
}
```

The concrete trait may remain compatible with the existing implementation. The important requirement is that file-specific behavior stays behind contributor implementations.

## AD-3: Workspace instructions and memory remain distinct until final rendering

Do not immediately convert both sources into undifferentiated `Message::System` values.

Represent them first as typed context items:

```rust
pub enum ContextKind {
    Instruction,
    Memory,
    Source,
    Excerpt,
    Note,
    Document,
    Decision,
    ToolResult,
}
```

Workspace instructions:

```rust
ContextItem {
    kind: ContextKind::Instruction,
    priority: ContextPriority::Critical,
    trust: ContentTrust::Trusted,
    stability: ContextStability::SessionStatic,
    source: ContextSource::WorkspaceInstruction,
    pinned: true,
}
```

General memory:

```rust
ContextItem {
    kind: ContextKind::Memory,
    priority: ContextPriority::Low,
    trust: ContentTrust::Trusted,
    stability: ContextStability::SessionStatic,
    source: ContextSource::WorkspaceMemory,
    pinned: false,
}
```

Pinned memory:

```rust
ContextItem {
    kind: ContextKind::Memory,
    priority: ContextPriority::High,
    trust: ContentTrust::Trusted,
    stability: ContextStability::SessionStatic,
    source: ContextSource::WorkspaceMemory,
    pinned: true,
}
```

Provider rendering may encode memory into a system-role block where required, but Gestalt’s internal semantic model must preserve the distinction.

## AD-4: Workspace instructions are critical but bounded

`workspace.md` must not be partially trimmed.

The contributor must apply the following rule:

> Workspace instructions are either included completely or context construction fails with a typed diagnostic.

Do not silently truncate the file.

Configuration must provide a maximum token or byte limit.

Default recommendation:

```json
{
  "max_tokens": 12000,
  "max_bytes": 131072
}
```

If either limit is exceeded:

```rust
ContextContributionError::SourceTooLarge {
    source: WorkspaceInstruction,
    path,
    estimated_tokens,
    max_tokens,
}
```

## AD-5: Memory is budget-aware

General memory must not be classified as permanently critical.

The context compiler may:

1. Include pinned memory.
2. Rank relevant general memory.
3. Include relevant entries within the memory budget.
4. Omit low-relevance entries.
5. Report omissions in context diagnostics.
6. Never silently reinterpret omitted memory as deleted memory.

Initial implementation may use deterministic section order and token limits without semantic ranking.

A later implementation may add lexical or semantic selection behind the same interface.

## AD-6: Stable context is snapshotted at session initialization

At session initialization:

1. Resolve configured context contributors.
2. Load workspace instructions.
3. Load memory.
4. Validate limits.
5. Parse memory sections.
6. Compute content hashes.
7. Build a session-stable context snapshot.
8. Record snapshot metadata in the trace.

The snapshot remains stable for the duration of the session.

Memory accepted during the session:

* Is written atomically to persistent storage.
* Does not mutate the current session’s stable prefix.
* Becomes visible in the next session by default.

A future explicit refresh operation may rebuild the snapshot.

## AD-7: Trust and authority are separate concepts

`ContentTrust::Trusted` means that content originates from a user-controlled or accepted workspace source.

It does not mean:

* The content is always correct.
* The content is current.
* The content may grant permissions.
* The content may override host policy.
* The content may bypass approval.
* The content is safe merely because it exists in a repository.

Introduce an authority classification if the existing model cannot express this distinction:

```rust
pub enum ContextAuthority {
    HostPolicy,
    Application,
    WorkspaceInstruction,
    ActiveSkill,
    UserInput,
    PersistentMemory,
    ExternalSource,
}
```

Minimum precedence:

```text
Host policy
Application/runtime instructions
Workspace instructions
Active skill instructions
Current user intent
Pinned memory
Recent history
General memory
Workspace sources
External untrusted sources
```

The exact ordering between active skill instructions and current user input may remain governed by existing skill semantics, but memory must never outrank current user intent or workspace instructions.

## AD-8: Memory cannot widen authority

Memory content must not:

* Register tools.
* Enable tools.
* Grant filesystem access.
* Grant network access.
* Grant shell access.
* Disable approval.
* Mark extensions trusted.
* Override policy.
* Activate untrusted extensions.
* Change execution mode.

Memory is contextual state only.

## AD-9: Memory mutation uses a dedicated lifecycle

Ordinary file-writing behavior must not be treated as the canonical memory update mechanism.

Introduce a structured proposal:

```rust
pub struct MemoryProposal {
    pub proposal_id: String,
    pub source_session_id: String,
    pub base_hash: String,
    pub operations: Vec<MemoryOperation>,
    pub rationale: Option<String>,
}
```

Operations:

```rust
pub enum MemoryOperation {
    Add {
        section: String,
        content: String,
    },
    Replace {
        entry_id: String,
        content: String,
    },
    Remove {
        entry_id: String,
        reason: String,
    },
    Supersede {
        entry_id: String,
        content: String,
        reason: Option<String>,
    },
}
```

Lifecycle:

```text
Session evidence
    ↓
Candidate extraction
    ↓
Proposal construction
    ↓
Deduplication and base-hash validation
    ↓
User approval
    ↓
Atomic storage update
    ↓
Trace event
    ↓
Available to future session snapshots
```

V1 may support only `Add` operations if necessary, but the proposal envelope should support future operation types.

## AD-10: Existing runtime events remain the observability foundation

Workspace and memory events should be emitted through the existing event and tracing architecture rather than introducing a separate logger.

Add runtime or trace events for:

* Contributor resolved.
* Context file loaded.
* Context file missing.
* Context file disabled.
* Context file rejected.
* Context source oversized.
* Context snapshot created.
* Memory proposal created.
* Memory proposal accepted.
* Memory proposal rejected.
* Memory write conflict.
* Memory write completed.

Do not expose file contents in events by default.

---

# 5. Configuration Schema

## 5.1 Canonical configuration

Add structured configuration under `context`:

```json
{
  "context": {
    "workspace": {
      "enabled": true,
      "path": ".gestalt/workspace.md",
      "required": false,
      "max_tokens": 12000,
      "max_bytes": 131072,
      "snapshot": "session"
    },
    "memory": {
      "enabled": true,
      "path": ".gestalt/memory.md",
      "required": false,
      "strategy": "budgeted",
      "max_tokens": 8000,
      "max_bytes": 524288,
      "pinned_section": "Pinned",
      "snapshot": "session",
      "write_mode": "proposal"
    }
  }
}
```

## 5.2 Proposed Rust types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    #[serde(default)]
    pub workspace: WorkspaceContextConfig,

    #[serde(default)]
    pub memory: MemoryContextConfig,

    // Existing context budget fields remain here.
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceContextConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_workspace_path")]
    pub path: PathBuf,

    #[serde(default)]
    pub required: bool,

    #[serde(default = "default_workspace_max_tokens")]
    pub max_tokens: usize,

    #[serde(default = "default_workspace_max_bytes")]
    pub max_bytes: usize,

    #[serde(default)]
    pub snapshot: ContextSnapshotMode,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryContextConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_memory_path")]
    pub path: PathBuf,

    #[serde(default)]
    pub required: bool,

    #[serde(default)]
    pub strategy: MemorySelectionStrategy,

    #[serde(default = "default_memory_max_tokens")]
    pub max_tokens: usize,

    #[serde(default = "default_memory_max_bytes")]
    pub max_bytes: usize,

    #[serde(default = "default_pinned_section")]
    pub pinned_section: String,

    #[serde(default)]
    pub snapshot: ContextSnapshotMode,

    #[serde(default)]
    pub write_mode: MemoryWriteMode,
}
```

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSnapshotMode {
    Session,
}
```

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySelectionStrategy {
    Full,
    Budgeted,
}
```

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteMode {
    Disabled,
    Proposal,
}
```

Only `Session` snapshot mode needs to ship initially.

## 5.3 Explicit disabling

Users must be able to disable either contributor:

```json
{
  "context": {
    "workspace": {
      "enabled": false
    },
    "memory": {
      "enabled": false
    }
  }
}
```

Disabled contributors:

* Do not attempt file access.
* Do not warn when files are absent.
* Appear as disabled in `status`.
* Do not produce missing-file warnings in `doctor`.

## 5.4 Required semantics

Example:

```json
{
  "context": {
    "workspace": {
      "enabled": true,
      "required": true
    }
  }
}
```

If required and missing, runtime initialization must fail before the first provider call.

## 5.5 Compatibility

Pre-hardening context path aliases are unsupported. Use
`context.workspace.path` and `context.memory.path`.

Legacy behavior defaults:

```text
enabled  = true
required = false
```

Resolution precedence:

```text
new structured field
    >
legacy field
    >
built-in default
```

If both forms are present and conflict:

* Structured configuration wins.
* `config validate` emits a deprecation warning.
* `config explain` shows both sources and the winning value.

Do not remove the legacy fields in the same release that introduces structured configuration.

---

# 6. Workspace Initialization

## 6.1 Command

```bash
gestalt init
```

Optional future arguments:

```bash
gestalt init --template minimal
gestalt init --template coding
gestalt init --template knowledge
gestalt init --force
gestalt init --format json
```

Only the default or `minimal` template is required for V1.

## 6.2 Initialization behavior

The command must be:

* Explicit.
* Idempotent.
* Atomic where practical.
* Non-destructive by default.
* Machine-readable when requested.
* Safe to re-run.

It must not overwrite existing user-authored files unless the user explicitly uses an overwrite option.

## 6.3 Default scaffold

```text
.gestalt/
├── workspace.md
└── memory.md
gestalt.json
```

Generated runtime directories should remain lazy:

```text
.gestalt/runs/
.gestalt/source-cache/
.gestalt/artifacts/
```

Do not create empty directories that are not immediately needed.

## 6.4 Workspace initialization metadata

Add an initialization marker to configuration:

```json
{
  "workspace": {
    "initialized": true,
    "format_version": 1
  }
}
```

Alternative internal metadata storage is acceptable, but it must remain human-readable.

The marker enables diagnostics to distinguish:

* Uninitialized directory.
* Initialized healthy workspace.
* Initialized workspace missing expected files.
* Deliberately disabled context contributors.

## 6.5 Default `workspace.md`

```markdown
# Workspace Instructions

## Purpose

Describe what this workspace is for.

## Operating Constraints

- Follow the current user request.
- Do not modify files outside approved workspace paths.
- Preserve existing project conventions.

## Output Standards

Describe expected output formats, tone, testing requirements, and citation rules.
```

Keep the generated file concise.

## 6.6 Default `memory.md`

```markdown
# Workspace Memory

## Pinned

<!-- Stable facts and constraints that should usually remain in context. -->

## Decisions

<!-- User-approved decisions from prior sessions. -->

## Preferences

<!-- Persistent project or output preferences. -->

## Historical Notes

<!-- Lower-priority context that may be omitted under token pressure. -->
```

---

# 7. Loading and Validation

## 7.1 Typed load result

Introduce a typed load outcome:

```rust
pub enum ContextFileLoadResult {
    Loaded(LoadedContextFile),
    Missing {
        path: PathBuf,
    },
    Disabled {
        configured_path: PathBuf,
    },
}
```

Errors remain typed:

```rust
pub enum ContextFileLoadError {
    PermissionDenied {
        path: PathBuf,
    },
    IsDirectory {
        path: PathBuf,
    },
    InvalidUtf8 {
        path: PathBuf,
    },
    CanonicalizationFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    TooLarge {
        path: PathBuf,
        bytes: usize,
        max_bytes: usize,
    },
    TokenLimitExceeded {
        path: PathBuf,
        estimated_tokens: usize,
        max_tokens: usize,
    },
    InvalidFormat {
        path: PathBuf,
        reason: String,
    },
}
```

## 7.2 Read behavior

Replace broad error swallowing:

```rust
if let Ok(content) = fs::read_to_string(path) {
    // ...
}
```

with explicit handling:

```rust
match fs::read_to_string(path) {
    Ok(content) => { /* validate and load */ }
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
        ContextFileLoadResult::Missing { path }
    }
    Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
        return Err(ContextFileLoadError::PermissionDenied { path });
    }
    Err(error) => {
        return Err(ContextFileLoadError::ReadFailed {
            path,
            source: error,
        });
    }
}
```

## 7.3 Behavior matrix

| State          | Optional                                   | Required                                |
| -------------- | ------------------------------------------ | --------------------------------------- |
| Missing        | Skip and emit informational status         | Fail initialization                     |
| Disabled       | Skip without warning                       | Configuration error if also required    |
| Empty          | Load empty and report informational status | Warn or fail validation based on policy |
| Unreadable     | Fail initialization                        | Fail initialization                     |
| Directory      | Fail initialization                        | Fail initialization                     |
| Invalid UTF-8  | Fail initialization                        | Fail initialization                     |
| Oversized      | Fail context snapshot                      | Fail context snapshot                   |
| Invalid format | Fail validation                            | Fail validation                         |

## 7.4 Path restrictions

Configured workspace and memory paths must:

* Resolve deterministically.
* Be evaluated relative to the workspace root unless absolute.
* Respect existing path policy.
* Be canonicalized where possible.
* Reject unexpected traversal outside the workspace unless explicitly allowed by host policy.

Loading a trusted context file outside the workspace should require explicit configuration and policy permission.

---

# 8. Context Snapshot

## 8.1 Snapshot type

Introduce a session snapshot structure:

```rust
pub struct WorkspaceContextSnapshot {
    pub workspace: Option<SnapshotContextSource>,
    pub memory: Option<MemorySnapshot>,
    pub created_for_session: String,
    pub snapshot_hash: String,
}
```

```rust
pub struct SnapshotContextSource {
    pub configured_path: PathBuf,
    pub canonical_path: PathBuf,
    pub content_hash: String,
    pub byte_size: usize,
    pub token_estimate: usize,
    pub content: String,
    pub trust: ContentTrust,
    pub stability: ContextStability,
}
```

```rust
pub struct MemorySnapshot {
    pub source: SnapshotContextSource,
    pub entries: Vec<MemoryEntry>,
    pub pinned_entry_ids: Vec<String>,
}
```

## 8.2 Snapshot creation point

Create the snapshot after:

* Effective configuration resolution.
* Workspace root resolution.
* Workspace trust resolution.
* Contributor registration.

Create it before:

* The first context build.
* The first model request.
* Session execution begins.

## 8.3 Snapshot hash

Compute a deterministic hash from:

* Contributor IDs.
* Contributor versions.
* Canonical paths.
* Source content hashes.
* Parsing version.
* Selection strategy.
* Relevant configuration fields.

Do not include timestamps in the deterministic hash.

## 8.4 Cache-aware placement

Classify:

```text
workspace instructions → SessionStatic
pinned memory          → SessionStatic
selected general memory → SessionStatic for current session
recent history         → TurnDynamic
tool results           → TurnDynamic
one-turn notices       → Ephemeral
```

The stable snapshot must not change when memory is accepted during the same session.

---

# 9. Memory Format

## 9.1 V1 format

Use section-based Markdown.

Recognized default sections:

```markdown
## Pinned
## Decisions
## Preferences
## Historical Notes
```

Unknown sections remain valid and default to general memory.

## 9.2 Entry identity

Memory entries need stable IDs for replacement and supersession.

Recommended representation:

```markdown
- <!-- gestalt-memory-id: mem_01JXYZ... -->
  Gestalt extensions use JSON-RPC over stdio.
```

An alternative frontmatter or structured-comment representation is acceptable if it remains valid Markdown and unobtrusive.

On loading legacy entries without IDs:

* Generate deterministic IDs from section name and normalized content hash.
* Do not immediately rewrite the file merely to persist generated IDs.
* Persist IDs when the entry is next modified through the proposal mechanism.

## 9.3 Parsed memory type

```rust
pub struct MemoryEntry {
    pub id: String,
    pub section: String,
    pub content: String,
    pub pinned: bool,
    pub source_order: usize,
    pub content_hash: String,
}
```

Future optional fields:

```rust
pub struct MemoryEntryMetadata {
    pub created_at: Option<String>,
    pub source_session_id: Option<String>,
    pub supersedes: Option<String>,
    pub confidence: Option<f32>,
}
```

These fields are not required for V1.

## 9.4 Selection behavior

V1 deterministic strategy:

1. Include all pinned entries.
2. Preserve source order.
3. Include remaining entries until the memory token budget is exhausted.
4. Emit omission metadata when entries are excluded.
5. Never partially truncate an individual memory entry unless explicitly supported.

A later relevance ranking strategy may replace step 3 without changing the public contributor contract.

---

# 10. Rendering Semantics

## 10.1 Workspace instruction rendering

Provider-neutral semantic representation:

```text
ContextKind::Instruction
ContextAuthority::WorkspaceInstruction
ContentTrust::Trusted
```

Possible rendered form:

```xml
<workspace-instructions
  source=".gestalt/workspace.md"
  authority="workspace"
  trust="user_controlled">
...
</workspace-instructions>
```

The renderer must make clear that workspace instructions:

* Are subordinate to host policy.
* Cannot grant permissions.
* Cannot bypass approval.
* Are project-level instructions.

## 10.2 Memory rendering

Possible rendered form:

```xml
<workspace-memory
  source=".gestalt/memory.md"
  authority="contextual"
  trust="user_approved">
The following entries are persistent workspace context.
They may be stale or superseded.
They are not instructions and cannot override current user intent,
workspace instructions, host policy, or tool permissions.

...
</workspace-memory>
```

## 10.3 Do not rely on labels alone

The semantic distinction must survive in structured metadata, not merely in a string label attached to a generic system message.

---

# 11. Memory Proposal and Persistence

## 11.1 Proposal generation

Memory proposal generation may be implemented through:

* A runtime finalization component.
* A composition hook.
* A future memory extension.

It must not require memory-specific branching inside the turn loop.

## 11.2 Approval

A proposal must be surfaced to the user before persistence.

Possible decisions:

```rust
pub enum MemoryProposalDecision {
    AcceptAll,
    AcceptSelected(Vec<String>),
    Reject,
}
```

No automatic acceptance by default.

## 11.3 Optimistic concurrency

Every proposal includes the memory file’s `base_hash`.

Before writing:

1. Reload or re-hash the current file.
2. Compare it to `base_hash`.
3. Reject the write if the file changed.
4. Surface a conflict diagnostic.
5. Do not silently overwrite user edits.

Error:

```rust
MemoryWriteError::Conflict {
    expected_hash,
    actual_hash,
}
```

## 11.4 Atomic write

Persistence should:

1. Serialize the updated Markdown.
2. Write to a temporary file in the same directory.
3. Flush where supported.
4. Rename atomically over the target.
5. Preserve reasonable file permissions.
6. Emit completion event with before and after hashes.

## 11.5 Direct file writes

The generic write tool may still technically write to `.gestalt/memory.md` if policy permits, but the default policy should prevent or require explicit approval for direct memory-file mutation.

Canonical memory updates must use the proposal path.

Recommended policy behavior:

```text
direct generic write to memory file → confirm or deny
approved MemoryProposal persistence → dedicated controlled write path
```

---

# 12. Workspace Trust

## 12.1 Trust state

Introduce or reuse a workspace trust state:

```rust
pub enum WorkspaceTrust {
    Trusted,
    Restricted,
    Untrusted,
}
```

## 12.2 Trust implications

For trusted workspaces:

* Workspace instructions may activate automatically.
* Workspace-local skills follow existing trust policy.
* Memory may be loaded according to configuration.

For untrusted workspaces:

* Workspace instructions should be previewed or explicitly accepted.
* Project-local extensions remain untrusted.
* Memory should not be mutated without explicit approval.
* Workspace instructions cannot widen permissions.
* Context source metadata should reflect repository origin.

For restricted workspaces:

* Read-only context loading may be allowed.
* Writes and extension activation remain blocked or require approval.

Workspace trust enforcement may be staged if the current product lacks a trust workflow, but the context model must not assume all repository-local instructions are equivalent to explicit user-authored instructions.

---

# 13. CLI Behavior

## 13.1 `gestalt status`

`status` reports state neutrally.

Example:

```text
Workspace
  root                    /work/project
  initialized             yes
  trust                   trusted

Context contributors
  workspace instructions  loaded
    path                  .gestalt/workspace.md
    tokens                842
    hash                  sha256:...
  persistent memory       not present (optional)
  skills                  3 discovered
```

Possible statuses:

```text
loaded
missing (optional)
missing (required)
disabled
invalid
oversized
unreadable
outside allowed path
```

## 13.2 `gestalt doctor`

Warn or fail only for actionable health issues.

Warnings/errors include:

* Required file missing.
* Existing file unreadable.
* Existing file oversized.
* Invalid memory format.
* Invalid workspace frontmatter.
* Conflicting legacy and structured configuration.
* Context file outside permitted paths.
* Memory write mode enabled without writable destination.
* Initialized workspace missing expected required assets.
* Snapshot creation failure.

Do not warn merely because an optional file does not exist.

## 13.3 `gestalt config explain`

Show effective source:

```text
context.workspace.enabled
  value: true
  source: built-in default

context.workspace.path
  value: docs/project-spec.md
  source: workspace gestalt.json

context.memory.enabled
  value: false
  source: workspace gestalt.json
```

## 13.4 `gestalt context explain`

Extend context diagnostics to show:

* Contributor.
* Context kind.
* Priority.
* Stability.
* Trust.
* Token estimate.
* Included or omitted.
* Omission reason.
* Source hash.

Example:

```text
workspace.md
  kind        instruction
  priority    critical
  stability   session_static
  included    yes
  tokens      842

memory.md / Pinned
  kind        memory
  priority    high
  included    yes
  tokens      220

memory.md / Historical Notes
  kind        memory
  priority    low
  included    no
  reason      memory budget exhausted
```

---

# 14. Events and Trace Metadata

## 14.1 Runtime events

Add variants conceptually equivalent to:

```rust
RuntimeEvent::ContextContributorResolved {
    contributor_id: String,
    enabled: bool,
}
```

```rust
RuntimeEvent::ContextSourceLoaded {
    contributor_id: String,
    source_kind: String,
    path: String,
    content_hash: String,
    bytes: usize,
    estimated_tokens: usize,
}
```

```rust
RuntimeEvent::ContextSourceSkipped {
    contributor_id: String,
    source_kind: String,
    path: Option<String>,
    reason: String,
}
```

```rust
RuntimeEvent::ContextSnapshotCreated {
    session_id: String,
    snapshot_hash: String,
    source_count: usize,
}
```

```rust
RuntimeEvent::MemoryProposalCreated {
    session_id: String,
    proposal_id: String,
    operation_count: usize,
    base_hash: String,
}
```

```rust
RuntimeEvent::MemoryProposalDecision {
    proposal_id: String,
    accepted: bool,
    accepted_operations: usize,
}
```

```rust
RuntimeEvent::MemoryWriteCompleted {
    proposal_id: String,
    path: String,
    before_hash: String,
    after_hash: String,
}
```

```rust
RuntimeEvent::MemoryWriteConflict {
    proposal_id: String,
    path: String,
    expected_hash: String,
    actual_hash: String,
}
```

Exact event ownership between `AgentEvent`, `RuntimeEvent`, and trace envelopes should follow existing boundaries:

* Model-visible or loop-semantic events belong in `AgentEvent`.
* Workspace I/O and persistence lifecycle events belong in `RuntimeEvent`.
* Timestamps and correlation metadata belong in trace envelopes.

## 14.2 Sensitive content

Events must not include complete workspace or memory contents by default.

Allowed event metadata:

* Paths.
* Hashes.
* Sizes.
* Token estimates.
* Section names.
* Entry IDs.
* Operation counts.
* Status and reason.

---

# 15. Error Model

Introduce typed errors rather than string-only diagnostics.

```rust
pub enum WorkspaceContextError {
    Configuration(WorkspaceContextConfigError),
    Load(ContextFileLoadError),
    Parse(ContextFileParseError),
    Snapshot(ContextSnapshotError),
    MemoryWrite(MemoryWriteError),
}
```

Errors should include:

* Context source type.
* Configured path.
* Canonical path where available.
* Whether the source was required.
* Actionable repair guidance.

Example:

```text
workspace instruction file exceeds configured token limit:
  path: .gestalt/workspace.md
  estimated: 18,422 tokens
  limit: 12,000 tokens

Reduce the file size or raise context.workspace.max_tokens.
Gestalt did not partially truncate the instruction file.
```

---

# 16. Implementation Plan

## Phase 1: Configuration and typed loading

### Tasks

1. Add structured workspace and memory context configuration types.
2. Add defaults.
3. Add legacy-field compatibility.
4. Add conflict and deprecation diagnostics.
5. Replace broad `if let Ok(...)` reads with typed load handling.
6. Add byte-size validation.
7. Add token-limit validation.
8. Update generated JSON Schema.
9. Add config parsing tests.

### Exit criteria

* Existing configurations continue working.
* Contributors can be explicitly disabled.
* Missing optional files are skipped.
* Missing required files fail deterministically.
* Non-`NotFound` errors no longer disappear silently.

## Phase 2: Typed context contributors

### Tasks

1. Implement `WorkspaceInstructionContributor`.
2. Implement `MarkdownMemoryContributor`.
3. Preserve context kind, trust, stability, priority, and provenance.
4. Remove direct file-to-`Message::System` loading from CLI execution path.
5. Route both sources through the context compiler.
6. Add source hash and token metadata.
7. Add contributor registration tests.

### Exit criteria

* Workspace instructions and memory remain distinct until rendering.
* Core does not read files.
* Context compilation remains deterministic.
* Existing prompt behavior remains compatible where budgets permit.

## Phase 3: Session snapshot

### Tasks

1. Add `WorkspaceContextSnapshot`.
2. Create snapshot during session initialization.
3. Compute deterministic snapshot hash.
4. Feed snapshot sources into the context pipeline.
5. Mark stable items as `SessionStatic`.
6. Record snapshot metadata in traces.
7. Verify accepted memory changes do not mutate the active snapshot.

### Exit criteria

* Stable prompt prefix remains unchanged across session turns.
* Mid-session memory writes become visible only to later sessions.
* Snapshot hashes are deterministic for identical inputs.

## Phase 4: Budgeted memory

### Tasks

1. Parse memory Markdown into sections and entries.
2. Detect pinned section.
3. Assign stable entry IDs.
4. Add deterministic selection strategy.
5. Reserve a memory-specific token budget.
6. Record included and omitted memory entries.
7. Extend `context explain`.
8. Add tests for pinned and omitted entries.

### Exit criteria

* Pinned memory survives trimming.
* General memory may be omitted under budget pressure.
* Workspace instructions are never partially truncated.
* Context diagnostics explain omitted memory.

## Phase 5: Initialization and diagnostics

### Tasks

1. Update `gestalt init`.
2. Add initialization metadata.
3. Make initialization idempotent.
4. Update `gestalt status`.
5. Update `gestalt doctor`.
6. Update `gestalt config explain`.
7. Add machine-readable output where existing CLI conventions permit.
8. Add integration tests for initialized and uninitialized workspaces.

### Exit criteria

* Missing optional files do not produce doctor warnings.
* Missing required files do produce errors.
* Disabled contributors appear as disabled.
* Re-running initialization does not overwrite files.

## Phase 6: Memory proposal lifecycle

### Tasks

1. Add structured memory proposal types.
2. Add user decision surface.
3. Add base-hash conflict detection.
4. Add atomic Markdown persistence.
5. Add runtime events.
6. Add trace rendering.
7. Restrict canonical memory updates to proposal persistence.
8. Add tests for accept, reject, conflict, and partial acceptance.

### Exit criteria

* No memory update occurs without explicit approval.
* Concurrent user edits are not overwritten.
* All accepted writes are atomic and traceable.
* Current session snapshot remains unchanged.

## Phase 7: Workspace trust integration

### Tasks

1. Integrate context loading with workspace trust state.
2. Mark repository-local instructions with source provenance.
3. Add restricted behavior for untrusted workspaces.
4. Ensure local instructions cannot alter tool policy.
5. Add diagnostics and tests.

### Exit criteria

* Opening an untrusted repository does not silently grant its instructions host-level authority.
* Project-local files never widen permissions.
* Trust status is visible in `status` and traces.

---

# 17. Code-Area Guidance

The implementation plan must first inspect the current code rather than assuming exact APIs.

Likely areas:

```text
crates/gestalt-context/src/lib.rs
crates/gestalt-context/src/item.rs
crates/gestalt-runtime/src/runtime.rs
crates/gestalt-runtime/src/context_pipeline.rs
crates/gestalt-runtime/src/composition_hooks.rs
crates/gestalt-core/src/context.rs
crates/gestalt-core/src/message.rs
crates/gestalt-core/src/event.rs
crates/gestalt-runtime/src/trace/
src/workspace.rs
src/config.rs
src/run.rs
src/doctor.rs
```

The implementer must verify actual paths before editing.

## Do not immediately refactor

Before making changes:

1. Read the current workspace initialization path.
2. Read effective config resolution.
3. Read current context injection and trimming behavior.
4. Identify current contributor and snapshot abstractions.
5. Identify how context stability is represented.
6. Identify how runtime events are serialized.
7. Identify how memory proposals currently work, if implemented.
8. Map all constructors and pattern matches affected by type changes.
9. Run the existing test suite.
10. Produce a concise implementation map.

Avoid broad context-pipeline rewrites unless necessary.

Prefer adding small typed boundaries and migrating current behavior incrementally.

---

# 18. Invariants

The implementation must preserve the following invariants.

## Core invariants

1. `gestalt-core` performs no filesystem I/O.
2. The agent loop does not resolve workspace paths.
3. The agent loop does not parse Markdown.
4. Context contributors execute before provider request construction.
5. Context compilation remains deterministic.
6. The current user request cannot be trimmed before general memory.
7. Host policy outranks all workspace context.
8. Memory cannot grant authority.
9. Memory cannot bypass policy or approval.
10. Context load failures are typed and observable.

## Workspace instruction invariants

1. Workspace instructions are loaded only when enabled.
2. Missing optional instructions do not fail startup.
3. Missing required instructions fail before provider invocation.
4. Instructions are either included fully or rejected as oversized.
5. Instructions remain stable during a session.
6. Instructions preserve path and content-hash provenance.

## Memory invariants

1. Memory is context, not policy.
2. Pinned memory outranks general memory.
3. General memory may be omitted under budget pressure.
4. Memory updates require explicit approval.
5. Memory writes use optimistic concurrency.
6. Memory writes are atomic.
7. Accepted memory updates do not mutate the current session snapshot.
8. Rejected proposals produce no file mutation.
9. Direct memory mutation is not treated as canonical persistence.
10. Memory contents are not emitted in diagnostics or events by default.

---

# 19. Testing Strategy

## 19.1 Configuration tests

Test:

* Default structured configuration.
* Custom workspace path.
* Custom memory path.
* Disabled workspace contributor.
* Disabled memory contributor.
* Required workspace file.
* Required memory file.
* Legacy field mapping.
* Structured-over-legacy precedence.
* Invalid combination: `enabled = false`, `required = true`.
* JSON Schema generation.

## 19.2 Loader tests

Test:

* Existing valid file.
* Missing optional file.
* Missing required file.
* Permission denied.
* Path is directory.
* Invalid UTF-8.
* Oversized bytes.
* Excessive token estimate.
* Relative path resolution.
* Absolute path resolution.
* Path traversal.
* Canonical path outside workspace.

## 19.3 Context tests

Test:

* Workspace instruction classified as `Instruction`.
* Memory classified as `Memory`.
* Pinned memory classified as high priority.
* General memory classified below recent history.
* Workspace instructions included fully.
* Oversized workspace instructions rejected.
* General memory omitted under tight budget.
* Pinned memory retained under tight budget.
* Deterministic context output.
* Deterministic snapshot hash.

## 19.4 Cache stability tests

Test:

* Same session produces identical stable prefix across turns.
* Dynamic history does not modify stable snapshot hash.
* Accepted memory proposal does not modify active snapshot.
* New session observes updated memory.
* Changed workspace instructions produce a new snapshot hash.

## 19.5 CLI tests

Test:

* `gestalt init` creates minimal scaffold.
* Re-running `gestalt init` preserves existing files.
* `status` reports optional missing file neutrally.
* `doctor` ignores disabled contributor.
* `doctor` warns on required missing file.
* `doctor` reports unreadable file.
* `config explain` reports effective source.
* `context explain` reports omitted memory.

## 19.6 Memory proposal tests

Test:

* Add proposal accepted.
* Add proposal rejected.
* Selected operations accepted.
* Base hash conflict.
* Atomic write success.
* Temporary write failure.
* Serialization failure.
* Event emission.
* Trace replay includes proposal decision.
* No current-session snapshot mutation.

## 19.7 Regression tests

Preserve tests for:

* Existing default workspace loading.
* Existing custom paths.
* Context trimming.
* Provider rendering.
* Prompt-cache placement.
* Run replay.
* Config doctor.
* Workspace status.
* Extension context contributors.
* Composition hook context additions.

---

# 20. Acceptance Criteria

The feature is complete when all of the following are true:

* [ ] `.gestalt/workspace.md` remains the default workspace instruction file.
* [ ] `.gestalt/memory.md` remains the default memory file.
* [ ] Either contributor can be explicitly disabled.
* [ ] Either contributor can be marked required.
* [ ] Missing optional files do not fail runtime initialization.
* [ ] Missing required files fail before the first model request.
* [ ] Existing but unreadable files do not disappear silently.
* [ ] Workspace instructions and memory use distinct context kinds.
* [ ] Workspace instructions are indivisible critical context.
* [ ] Oversized workspace instructions fail with actionable diagnostics.
* [ ] Pinned memory receives elevated priority.
* [ ] General memory is budget-aware.
* [ ] Memory is never treated as policy or permission.
* [ ] Stable context is snapshotted once per session.
* [ ] Mid-session memory persistence does not mutate the active snapshot.
* [ ] Context source hashes and token estimates are traceable.
* [ ] `status` distinguishes loaded, missing, disabled, and invalid states.
* [ ] `doctor` is configuration-aware.
* [ ] Removed context path aliases are rejected.
* [ ] Memory writes require an approved proposal.
* [ ] Memory writes use base-hash conflict detection.
* [ ] Memory persistence is atomic.
* [ ] No file I/O is added to `gestalt-core`.
* [ ] No workspace-specific branching is added to the sacred agent loop.
* [ ] Existing context, runtime, and replay tests remain green.

---

# 21. Migration Strategy

## Release N

* Introduce structured configuration.
* Continue accepting legacy fields.
* Add deprecation notices only when users run validation or explanation commands.
* Preserve current paths and optional behavior.
* Change memory priority semantics behind the context compiler.
* Add explicit disable support.

## Release N+1

* Update generated examples and documentation to structured configuration.
* Continue legacy parsing.
* Add stronger warning for conflicting legacy fields.

## Future major release

* Remove legacy fields only after:

  * Migration tooling exists.
  * Config explanation clearly shows replacements.
  * Existing users have had at least one stable migration cycle.

No automatic rewrite of user configuration is required in the first release.

---

# 22. Risks and Mitigations

## Risk: Behavior changes because memory is no longer always included

Mitigation:

* Preserve pinned memory.
* Expose omission details through `context explain`.
* Provide `strategy: "full"` for users who explicitly want full inclusion.
* Record memory selection in traces.

## Risk: Additional types increase context-system complexity

Mitigation:

* Reuse existing `ContextItem`, `ContextKind`, `ContextPriority`, and `ContextStability`.
* Keep file-specific behavior inside contributors.
* Avoid adding memory logic to the agent loop.

## Risk: Snapshot behavior surprises users after accepting memory

Mitigation:

* State clearly that accepted memory applies to future sessions.
* Display the persistence result.
* Add a future explicit refresh command if necessary.

## Risk: Workspace trust scope expands the feature

Mitigation:

* Implement provenance and authority classification now.
* Stage interactive trust UX if necessary.
* Never allow workspace instructions to widen policy, regardless of trust UX status.

## Risk: Generic write tools bypass memory proposal flow

Mitigation:

* Add a default policy rule for configured memory paths.
* Treat direct mutation as high-risk or denied.
* Keep the proposal writer as a dedicated host-controlled code path.

---

# 23. Future Extensions

The following features should build on this design without changing the loop:

* `MemoryStore` abstraction.
* SQLite-backed memory.
* Team memory services.
* Lexical memory search.
* Semantic memory retrieval.
* Confidence and expiry metadata.
* Automatic supersession suggestions.
* Per-profile memory.
* Per-user memory.
* Workspace branch memory.
* GUI memory review.
* Memory import/export.
* Signed enterprise instruction bundles.
* Explicit session context refresh.
* Multiple named instruction contributors.
* Organization-level instruction layers.

---

# 24. Final Design Position

Workspace initialization and persistent memory are valid harness capabilities because they improve repeatable, inspectable, project-scoped model execution.

The harness must not, however, assume that two Markdown files are mandatory universal state.

The final model is:

> Gestalt provides first-party workspace-instruction and persistent-memory contributors. Markdown files are the transparent default backend. Workspace instructions are stable, bounded, critical instructions. Memory is user-approved contextual state selected under a budget. Both are configurable, observable, replaceable, cache-aware, and subordinate to host policy.

This preserves Gestalt’s files-first philosophy without coupling the long-term harness architecture to a single CLI storage convention.
