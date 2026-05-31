## gestalt-harness — Product Requirements Document

**Version:** 3.0 — Definitive  
**Status:** Active  
**License:** MIT  
**Form factor:** Rust crate · CLI-first · embeddable library

> **Document scope:** This PRD owns product vision, problem statement, user needs, feature specification, governance philosophy, and roadmap. All runtime interfaces, trait contracts, Rust type definitions, and implementation diagrams live in [[gestalt-harness-architecture]]. When the two documents conflict, the architecture document governs implementation decisions; this document governs scope and priority decisions.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Reference Architecture Lessons](#2-reference-architecture-lessons)
3. [What Makes a Harness Lightweight and Great](#3-what-makes-a-harness-lightweight-and-great)
4. [Problem Statement](#4-problem-statement)
5. [Target Users](#5-target-users)
6. [Design Philosophy](#6-design-philosophy)
7. [Product Principles](#7-product-principles)
8. [Execution Modes](#8-execution-modes)
9. [Workspace Model](#9-workspace-model)
10. [Tool System](#10-tool-system)
11. [MCP Integration](#11-mcp-integration)
12. [Skill System](#12-skill-system)
13. [Context Engineering](#13-context-engineering)
14. [Document & Research Pipeline](#14-document--research-pipeline)
15. [Provider Layer](#15-provider-layer)
16. [CLI Interface](#16-cli-interface)
17. [Configuration](#17-configuration)
18. [Governance & Permissions](#18-governance--permissions)
19. [Verification](#19-verification)
20. [Observability](#20-observability)
21. [Roadmap & Build Order](#21-roadmap--build-order)
22. [What gestalt-harness Is Not](#22-what-gestalt-harness-is-not)

- [Appendix A: AGENTS.md](#appendix-a-agentsmd)
- [Appendix B: Dependency Audit](#appendix-b-dependency-audit)
- [Appendix C: Engineering Efficiency Rules](#appendix-c-engineering-efficiency-rules)
- [Appendix D: Open Questions](#appendix-d-open-questions)
- [Appendix E: Success Metrics](#appendix-e-success-metrics)

---

## 1. Executive Summary

`gestalt-harness` is a lightweight, local-first AI agent harness written in Rust. It is optimized for knowledge work — synthesizing academic papers, PDFs, architecture docs, web research, and Markdown notes — while remaining natively capable at system execution, coding, and tool calling via bash and MCP.

Rather than building a heavy, magic-filled orchestration framework, `gestalt-harness` delivers a small, transparent execution harness. It establishes explicit permission gates before any tool runs, provides deterministic context assembly, tracks every session event to a human-readable JSONL log, and allows complete user auditability.

The design ethos is:

> mini-swe-agent's sacred loop + pi's provider extensibility + a local-first knowledge workspace + Rust's compile-time safety and zero-overhead binary distribution.

**What you get:** `cargo install gestalt-harness` produces a single binary. You run it in your project directory. It reads your notes, your PDFs, your architecture docs, and your code. It uses the tools you allow. It logs everything it does. You can replay, inspect, and audit every session.

**What you don't get:** a cloud account requirement, a hidden embedding database, an opaque vector store, or a framework that wants to own your application.

---

## 2. Reference Architecture Lessons

Before specifying `gestalt-harness`, this section records what was extracted from three reference architectures and why.

### 2.1 pi (earendil-works/pi)

A TypeScript multi-package AI agent toolkit. The lessons that transfer to Rust:

**Keep:** Lazy provider registration (factory closures, never static instantiation). A standardized five-event stream contract that every provider maps to (`Text`, `Thinking`, `ToolCall`, `Usage`, `Stop`). Strict typing at package boundaries. Configurable defaults on every user-facing behavior.

**Discard:** Monorepo-as-distribution-unit. TUI as a first-class core concern. Node.js cold-start overhead — the Rust story inverts this entirely.

### 2.2 mini-swe-agent

A ~100-line Python agent achieving >74% on SWE-bench. The single most important architectural reference for the loop design.

**Three radical simplifications to internalize:**

Bash is the universal primitive. The model already knows the shell. You do not need to build a tool for something that has been a stable Unix command for decades. For gestalt-harness, `BashTool` is the primary execution substrate; document tools and MCP are layered on top, not underneath.

Linear, append-only history. Every step appends to the message list. No sliding window, no graph state, no history rewriter on the fast path. The trajectory is the conversation. This means deterministic replay, zero preprocessing for fine-tuning export, and debuggability at a glance.

Stateless per-action execution. Each bash invocation is an independent subprocess with an explicit working directory. No persistent shell session across turns. Sandboxing becomes trivial, parallelism becomes trivial, and the harness can never enter an unrecoverable shell state.

**Where mini-swe-agent falls short for knowledge work:** The bash-only model is optimal for software engineering where the environment already has all tools installed. For PDF ingestion with structure preservation, web fetch with readability extraction, and token-budget-aware source management, structured native tools are necessary. The lesson is to start from mini's loop simplicity and extend the tool boundary cleanly.

### 2.3 awesome-agent-harness Taxonomy

A curated catalog of agent harness resources across nine categories. The key principle:

> "Your agent needs a harness, not a framework."

A framework prescribes how you build your application. A harness is reliability infrastructure that wraps any agent loop — providing sandboxing, observability, resumability, and permission enforcement without dictating the agent's internal logic. `gestalt-harness` is a harness.

---

## 3. What Makes a Harness Lightweight and Great

**Lightweight** means:

- Single compiled binary, under 10 MB stripped, with zero runtime language dependencies (no Node, Python, or JVM).
- Sub-100ms cold start for CLI and interactive sessions.
- Minimal allocation on the hot path (the turn loop itself).
- Core crate compiles in under 30 seconds.
- Total direct dependencies strictly limited per crate.

**Great** means:

- **Sacred loop:** The core agent loop is under 200 lines of readable Rust. If it grows beyond that, something belongs in middleware, not the loop.
- **Honest abstractions:** `Provider`, `Tool`, and `ContextPipeline` each do one thing well. No god objects.
- **Explicit failure:** No silent panics, no swallowed errors. Every fallible path returns a typed result. No unexpected behavior on known error paths.
- **Inspectable and replayable:** Every event — prompt sent, tool called, token consumed, policy decision made — is logged to a JSONL file the user can read with `cat` and replay with `gestalt replay`.

**The tradeoff table — where we chose lightweight:**

|Concern|gestalt-harness choice|Heavy alternative (rejected)|
|---|---|---|
|Core loop|~150 lines, append-only history|Graph executor with state machine nodes|
|Tool dispatch|Trait object + JSON Schema via `schemars`|Macro-generated reflection system|
|Provider abstraction|Single async stream → 5 unified events|Protocol-buffer codegen per provider|
|Context management|Middleware stack on message history|Embedding database + retrieval pipeline|
|Sandboxing|Working-dir isolation by default; Docker opt-in|Container orchestrator required|
|Configuration|TOML file + env vars|GUI config editor|
|Memory|Human-editable `memory.md`|Opaque binary vector store|
|UI|`ratatui` behind `--features tui` flag|Electron app|

---

## 4. Problem Statement

Modern knowledge work demands an agent that bridges files, web sources, reports, and code. Existing solutions fail on three fronts.

**No document-native tool layer.** Current harnesses treat documents as unstructured text blobs. A knowledge worker needs token-budget-aware line ranges, structure-preserving PDF parsing, citation tracking, and clean HTML-to-Markdown extraction. These are not optional features; they determine whether the agent's outputs are auditable.

**Fragile context architectures.** Coding agents assume the workspace is a code tree. Knowledge workers maintain a structured document corpus — sources, notes, drafts, memory facts — that accumulates across sessions. The harness must understand `workspace.md`, `memory.md`, and `/sources/` without requiring re-injection on every turn.

**No Rust-native production harness.** All major harnesses (pi, mini-swe-agent, LangGraph) are Python or TypeScript. A Rust implementation provides: single-binary distribution, zero GC pauses, safe async concurrency, compile-time tool schema validation, and WASM compilation for embedding in browser-based frontends.

---

## 5. Target Users

|Persona|Core Job|The gestalt-harness Unlock|
|---|---|---|
|**Research-heavy builders**|Synthesize scientific papers, technical docs, web pages into structured outputs|PDF ingestion with citation contracts, source budget management, knowledge-mode context|
|**Architects and technical leads**|Work across ADRs, codebase repositories, and requirements documents|Architecture-aware context tagging, skill templates for ADR workflows, git-native run logs|
|**Power CLI users**|Fast, local-first terminal companion for complex tasks|Sub-100ms cold start, no cloud dependency, full JSONL traceability|
|**Teams building custom agents**|Need a lightweight, reusable harness crate — not an opinionated platform|Clean `gestalt-core` trait surface; use as a library without the CLI|

---

## 6. Design Philosophy

Six principles, in priority order. They govern every product decision — feature scope, roadmap sequencing, and tradeoffs.

**1. The Loop Is Sacred.** The core engine is small and stays small. Changes to tools, providers, policies, or context must never pollute the agent loop. If a new concern requires editing the loop, it belongs in middleware.

**2. Files Over Hidden State.** Project configuration, memory, run logs, and skills reside in a readable, versionable `.gestalt/` directory as Markdown and TOML files. No opaque binary databases. No hidden embedding caches. Every piece of state the agent has access to is human-readable and human-editable.

**3. Deterministic Context Compilation.** The context pipeline behaves like a compiler: it consumes workspace sources with defined priority, applies budget constraints, ranks by relevance, and outputs a well-formed context packet. The same inputs always produce the same packet. This makes behavior predictable and bugs reproducible.

**4. Explicit Permissions.** The model is a probabilistic generator. The harness is a deterministic policy validator. These are different jobs. The policy engine runs before any tool executes, not after, and its decisions are logged as first-class events. A minimal policy layer ships in v0.1; no tool executes without a gate.

**5. Bash First, Not Bash Only.** Bash is the primary execution substrate and the escape hatch for anything not covered by structured tools. Document ingestion and index building require native tools that are faster and more reliable than shell scripts. Both coexist.

**6. Events as the Ground Truth.** The UI, the trace log, the cost analyzer, the replay engine, and the test harness all consume the same event stream. There is no separate logging path. An event that is not emitted did not happen.

---

## 7. Product Principles

These are operating constraints that flow from the design philosophy. They are not negotiable during v0.1–v0.3.

**Security before convenience.** A harness that auto-executes arbitrary shell commands without a policy gate is not a harness — it is a liability. The confirmation UX may occasionally slow a user down. That is acceptable. Silent destructive execution is not.

**Readable state beats faster state.** `memory.md` is slower to query than a vector store. It is also readable, editable, versionable, and auditable. For a local-first tool, these properties matter more than query speed in the common case.

**Explicit beats implicit.** If the agent omits a source because the token budget is tight, it should say so. If a tool call was denied by policy, the trace should record why. If context was compressed, the user should be able to see what was removed. No silent degradation.

**The harness does not own the user's files.** Write operations require at least `Medium` risk classification and are subject to policy. The harness never auto-writes to paths outside the configured workspace without explicit approval. `.git/`, secret files, and credential files are denied by default.

**External content is untrusted until the user says otherwise.** Web pages, PDFs, MCP responses, and retrieved documents are tagged as untrusted and rendered inside explicit boundaries before the model sees them. This is a structural prompt injection defense, not a suggestion.

---

## 8. Execution Modes

The execution mode controls how the policy engine routes tool calls and how the agent interacts with the user during a session.

|Mode|Behavior|
|---|---|
|`confirm`|**Default.** High-risk actions suspend execution and prompt the user for approval, skip, or edit before proceeding. Medium-risk writes show a diff. Low-risk reads auto-execute.|
|`yolo`|Evaluates tool calls strictly against the `policies.toml` allow-lists. Commands on the allow-list execute automatically. Unlisted or high-risk commands still require confirmation.|
|`human`|The agent proposes tool calls but does not execute them. The user runs tools manually and pastes results back. Useful for auditing proposed actions before delegating.|
|`dry-run`|Generates tool invocation payloads and tracks planned file changes without executing any side effects. Produces a plan that can be reviewed before running for real.|
|`replay`|Reads a JSONL run log and reproduces the session display offline without making any provider calls or executing tools.|

Mode can be set in `config.toml`, in `workspace.md` front matter, via a CLI flag, or toggled mid-session with `/mode`.

---

## 9. Workspace Model

### 9.1 Workspace Discovery

On startup, `gestalt` assembles initial workspace state from the project directory:

1. **`.gestalt/workspace.md`** — Prepended as the system prompt prefix. Defines the project's operating goals, output standards, and tone. Treated as a trusted instruction. If absent, gestalt starts in plain mode with a minimal default system prompt.
2. **`.gestalt/memory.md`** — Prepended as persistent context. Contains accumulated facts from prior sessions that the user has approved.
3. **`/sources/`** — Enumerated to build a file index (names, sizes, types) for token-budget-aware loading.
4. **`/docs/`** — Enumerated as the deliverables index.
5. **`.gestalt/skills/`** — Skill names and descriptions loaded on startup (not full bodies). Full bodies are loaded on activation only.

Progressive disclosure: gestalt is useful on day one without any configuration. Power features surface as the workspace matures.

### 9.2 Workspace Context File Roles

|File|Role|Modifiable by agent?|
|---|---|---|
|`workspace.md`|Project instructions, goals, tone, constraints|No (user-authored, trusted)|
|`memory.md`|Persistent facts accumulated across sessions|Only with user approval at session end|
|`/sources/`|Input documents: papers, specs, data, reference material|No (read-only inputs)|
|`/docs/`|Output deliverables: reports, analyses, plans|Yes (agent writes here)|
|`.gestalt/skills/`|Reusable procedural instruction sets|No (user-managed)|
|`.gestalt/runs/`|JSONL trace logs for every session|Append-only|
|`.gestalt/source-cache/`|Extracted chunks and summaries of ingested sources|Managed by harness|

### 9.3 Session Modes

**Interactive mode (default):** The user types; the agent responds. History persists within the session. The session is named and written to a run log.

```bash
gestalt run "Summarize the papers in /sources/ and output a synthesis to /docs/"
gestalt run --workspace ./research --mode confirm
gestalt run --resume .gestalt/runs/2026-05-30T14:22.jsonl
```

**Pipeline mode:** A defined sequence of tasks runs autonomously from a Markdown pipeline file. Suitable for CI, cron jobs, and repeatable research workflows.

```bash
gestalt pipeline --file .gestalt/pipelines/weekly-brief.md
```

**Library mode:** Use `gestalt-core` as a dependency in another application. The harness does not own the application; it provides the loop, tools, and policy infrastructure.

```rust
let agent = AgentLoop::builder()
    .provider(AnthropicProvider::from_env()?)
    .tool(BashTool::default())
    .tool(WebFetchTool::default())
    .tool(MCPBridge::connect("http://localhost:3000")?)
    .workspace("./my-project")
    .build()?;

let result = agent.run_task("Extract key findings from all PDFs in /sources/").await?;
```

### 9.4 Knowledge Work vs. Coding Mode

Context mode is auto-detected from `workspace.md` content or set explicitly. It adjusts default tool registration and context middleware behavior.

|Dimension|Knowledge Work Mode|Coding Mode|
|---|---|---|
|Default tools|bash, read, write, patch, web_fetch, pdf, search, ingest_doc|bash, read, write, patch, search|
|Context injected|workspace.md + memory.md + source index|workspace.md + git status|
|System prompt|Research and analysis focused|Software engineering focused|
|Token budget strategy|Summarize old sources first|Trim oldest tool outputs first|
|Output format|Markdown documents with citations|Code files and diffs|

---

## 10. Tool System

Tools are small, typed, policy-aware capabilities exposed to the model through JSON Schema. Each tool defines a strongly typed input contract; the harness derives the JSON Schema automatically from the Rust type.

The tool system has four design goals:

1. **Typed at the boundary.** Tool inputs are Rust structs, not untyped maps.
2. **Schema-first for models.** Every tool exposes machine-readable JSON Schema.
3. **Policy-gated before execution.** Risk is assessed before any side effect happens.
4. **Stateless by default.** Tool calls are deterministic, isolated, and easy to replay.

For complete interface contracts and schema definitions, see the architecture document §9.

### 10.1 Built-in Tools

|Tool|Purpose|Default Risk|
|---|---|---|
|`BashTool`|Execute a shell command as a fresh subprocess. The primary execution substrate.|Context-dependent (see §10.3)|
|`ReadTool`|Read a file from the workspace with optional line-range selection and token limiting.|Low|
|`WriteTool`|Write full replacement content to a file. Shows a diff by default.|Medium|
|`PatchTool`|Apply a unified diff patch to a file. Safer than full replacement for code edits.|Medium|
|`SearchTool`|Fast local search over the workspace with glob filtering.|Low|
|`WebFetchTool`|Fetch a URL and return readability-extracted Markdown. Respects network policy.|Medium|
|`PdfTool`|Extract text from a PDF file with optional page selection. Feature-gated.|Low|
|`IngestDocTool`|Convert a PDF, HTML page, or Markdown file into indexed workspace knowledge.|Medium|

### 10.2 Tool Selection Philosophy

Structured tools and bash complement each other. Use structured tools when path policy matters, output needs line ranges, results need citation metadata, the operation should be replayable, or output must be token-limited. Use bash when the task is developer-native, the user expects shell semantics, no structured tool exists, or the command is explicitly approved by policy.

The model is not forced to choose. Both are available in the default tool registry.

### 10.3 Bash Risk Classification

Every bash command is classified before execution. The classifier uses string matching as a fast first pass; it is conservative by design, not exhaustive.

|Classification|Examples|Default behavior|
|---|---|---|
|**Critical**|`rm -rf /`, `mkfs`, `dd if=`, fork bombs|Always denied|
|**High**|`sudo`, `docker`, `git push`, `ssh`, `curl`, `wget`|Confirm required|
|**Medium**|`rm`, `mv`, `cp`, `mkdir`, write redirects (`>`), package installs|Confirm or policy-dependent|
|**Low**|`ls`, `cat`, `grep`, `rg`, `find`, `cargo check`, `git status`, `git diff`|Auto-execute|
|**Unknown**|Anything not matching a known pattern|Treated as Medium (confirm)|

The classifier is a hint layer, not a security boundary. The policy engine is the final authority. Sandboxing (when available) is the enforcement layer.

### 10.4 Parallel Tool Execution

When a model response contains multiple tool calls in a single turn, gestalt can execute read-only tools in parallel to reduce latency. Write tools, network tools, and tools touching shared workspace state execute sequentially in the order proposed. All results are reordered back to the original call sequence before being appended to history.

This capability is compatible with Anthropic's programmatic tool calling feature, where Claude can call tools from within a code execution container to process large datasets without repeated model round-trips. In v0.1, all tools are called directly. Programmatic tool calling is a Phase 3 capability.

---

## 11. MCP Integration

`gestalt-harness` is a standard MCP client. It connects to configured MCP servers and exposes their tools as native gestalt tools. From the agent loop's perspective, an MCP tool is invoked the same way as a built-in tool.

Internally, however, MCP tools remain distinguishable. They carry a trust level (`LocalStdio` or `RemoteHttp`), a server namespace (`mcp:<server_name>:<tool_name>`), and their results are always tagged `Untrusted` before entering the context window. Policy rules can target the MCP namespace, a specific server, or a specific tool.

### 11.1 MCP Configuration

```toml
# .gestalt/config.toml

[mcp.servers.brave-search]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-brave-search"]
env = { BRAVE_API_KEY = "${BRAVE_API_KEY}" }
permissions = ["network", "search"]

[mcp.servers.google-drive]
transport = "http"
url = "https://drivemcp.googleapis.com/mcp/v1"
permissions = ["drive-read"]
```

### 11.2 MCP Safety Defaults

- MCP `sampling` capability is **disabled by default.** Enabling it requires explicit opt-in per server.
- MCP `roots` capability is restricted to the workspace root. Servers are informed of this boundary during initialization.
- MCP server prompts and resources are not auto-injected into the trusted context.
- All MCP tool results pass through the trust boundary renderer before the model sees them.

---

## 12. Skill System

Skills provide specialized, reusable procedural instructions and tool restrictions. They encode how a user's knowledge work should proceed — not generic AI behavior. A literature review workflow, an ADR-writing workflow, and a competitor analysis workflow all deserve different default behaviors. Skills are how those behaviors are captured and reused.

### 12.1 Directory Structure

```
.gestalt/skills/
└── literature-synthesis/
    ├── SKILL.md          # Required: metadata + step-by-step instructions
    ├── scripts/          # Optional: executable helpers
    ├── references/       # Optional: domain manuals and example docs
    └── assets/           # Optional: schemas and templates
```

### 12.2 Skill Front Matter

```markdown
---
name: literature-synthesis
description: Synthesizes scientific literature PDFs. Use when analyzing papers in /sources/.
license: MIT
metadata:
  version: "1.0.0"
triggers:
  - "summarize papers"
  - "literature review"
  - "synthesize research"
permissions:
  tools: ["read", "search", "pdf", "write"]
  network: false
  write_paths: ["/docs/"]
  scripts: false
  max_token_budget: 80000
inputs:
  - "/sources/*.pdf"
output: "/docs/synthesis.md"
---
```

### 12.3 Three-Phase Progressive Disclosure

**Phase 1 — Discovery (startup):** The harness loads only `name` and `description` from all skill files. Minimal token impact. Skills appear in the system context as available capabilities.

**Phase 2 — Activation (trigger match):** When a user prompt matches a trigger phrase, the full body of `SKILL.md` is appended to the system instructions for that session. The agent now operates under the skill's specific procedure.

**Phase 3 — Deep Fetching (execution):** If the skill requires reference guides or script execution, the agent reads files under `references/` or runs scripts from `scripts/` using standard tools, just like any other file access.

### 12.4 Skill Trust

Skills authored in the local workspace (`.gestalt/skills/`) are trusted and activate automatically on trigger match. Skills downloaded from a community registry are untrusted until the user explicitly reviews and accepts them. Signed skills with a verified publisher are trusted if the signature is valid.

```bash
gestalt skill validate .gestalt/skills/literature-synthesis
gestalt skill list
gestalt skill fetch <name>    # Phase 3 community registry
```

---

## 13. Context Engineering

Context engineering is the highest-leverage concern in a knowledge-work harness. The context window is scarce execution memory; every token must justify its presence.

### 13.1 Assembly Priority

The context pipeline assembles the final message list using priority tiers. Higher-priority context survives trimming; lower-priority context is summarized or removed first when the token budget is tight.

|Priority|Content|Trimming behavior|
|---|---|---|
|**Critical — never trimmed**|System policy, `workspace.md`, active skill instructions, user-pinned facts|Never removed|
|**High — trimmed last**|Recent conversation history, current user request, active tool results|Removed last|
|**Medium — summarized first**|File excerpts, loaded source documents, search results, workspace index summaries|Summarized before dropping|
|**Low — trimmed first**|Long-term memory facts, older summaries, low-confidence retrieved context, background material|Removed first|

The guiding rule: never trim instructions, rarely trim recent intent, summarize sources before dropping them, aggressively remove weakly relevant background material.

### 13.2 Source Budget Strategy

Knowledge-work sessions often reference large PDFs, papers, or documentation trees. The harness uses a demand-loaded source budget model:

- If the budget allows and the source is relevant: inject the full relevant excerpt.
- If the budget is tight: inject a summary plus the most relevant chunks.
- If the budget is exhausted: inject a source card only (path, title, size, cached summary ID) and ask tools to retrieve on demand.

Source summaries are cached in `.gestalt/source-cache/` keyed by a content hash. Repeated references to the same source across sessions are free after the first ingestion.

### 13.3 Trust Boundaries

Every piece of content in the context window carries a trust level. User-authored instructions, `workspace.md`, and approved memory facts are **trusted**. External web pages, PDFs, MCP results, and retrieved documents are **untrusted** and are rendered inside explicit guardrail markup before the model processes them. This is a structural defense against prompt injection — not a configuration option.

### 13.4 Context Rules

The ten invariants that the context system enforces:

1. Current user intent beats historical context.
2. Instructions outrank sources.
3. Sources outrank summaries.
4. User-pinned facts outrank inferred memory.
5. Recent tool results outrank old tool results.
6. Exact excerpts beat broad summaries.
7. Workspace files are never silently rewritten into memory.
8. Generated summaries remain distinguishable from original sources.
9. The agent knows what it did not load.
10. When context is omitted due to budget, the omission is explicit when relevant.

The goal is not to stuff the context window. The goal is to construct the smallest sufficient context packet for the next model call.

---

## 14. Document & Research Pipeline

### 14.1 Supported Formats

|Format|Extraction method|Tier|
|---|---|---|
|Plain text / Markdown|Direct|Default|
|HTML|Readability extraction → Markdown|Default|
|PDF (text-based)|PDFium text extraction|Feature-gated: `pdf`|
|PDF (scanned)|OCR fallback|Phase 3|
|CSV|Structured table extraction|Phase 2|

Complex PDF features — tables, multi-column layout, footnotes, figure captions, formulas — are handled by extraction tiers. v0.2 targets text-based PDFs. OCR and visual interpretation are Phase 3.

### 14.2 Citation Contract

Every factual report generated in knowledge mode must carry source citations. This is a harness-enforced invariant, not a suggestion.

**Citation format:** `[^SourceID:PageRef-ChunkID]`

Where `SourceID` is the source file stem or URI hash recorded in the run log, `PageRef` is the page number, and `ChunkID` is the zero-indexed chunk from the ingestion pipeline.

**Example:** `[^wang-2025:p12-c21]` refers to page 12, chunk 21 of the file identified as `wang-2025`.

Citations carry structured metadata in the trace — source hash, byte range, retrieval timestamp — so the verification pipeline can confirm that every citation in a research output actually matches a real chunk in the source index. Unverifiable citations are flagged as errors in the assertion report.

---

## 15. Provider Layer

The provider layer isolates gestalt from model-specific APIs. Every provider maps its native streaming format to the same five internal events: `Text`, `Thinking`, `ToolCall`, `Usage`, `Stop`. The agent loop never sees provider-specific wire formats.

### 15.1 Supported Providers

|Provider|v0.1|v0.2|Notes|
|---|---|---|---|
|Anthropic|✓|✓|Primary. Extended thinking, prompt caching.|
|OpenAI|✓|✓|Primary.|
|Mistral|—|✓||
|Groq|—|✓|Fast inference for extraction tasks.|
|Ollama|—|✓|Local models; no API key required.|

Providers are registered lazily via factory functions. Adding a new provider does not require modifying the agent loop or any other crate.

### 15.2 Task-Based Model Routing

Different tasks within a session can be routed to different models. Expensive frontier models handle final synthesis; fast or local models handle extraction, summarization, and search.

```toml
# .gestalt/models.toml

[models.default]
provider = "anthropic"
model = "claude-sonnet-4-6"

[models.tasks.document_extraction]
provider = "ollama"
model = "qwen2.5-coder:7b"

[models.tasks.fast_search]
provider = "groq"
model = "llama-3.1-8b-instant"
```

Routing is advisory. CLI flags and session config always override task routing.

### 15.3 Model Catalog

Gestalt maintains a local model catalog for token budgeting, feature detection, and cost analysis. The catalog includes context window size, cost per token, and capability flags (vision, tool use, thinking, JSON schema).

```bash
gestalt models list
gestalt models refresh   # Pull updated catalog
```

Catalog entries include a `last_updated` field. Users should refresh periodically as provider pricing and capabilities change.

---

## 16. CLI Interface

The CLI serves three audiences: interactive users working session by session, pipeline operators running repeatable workflows, and developers inspecting and replaying prior runs.

### 16.1 Command Structure

```
gestalt [OPTIONS] <COMMAND>

COMMANDS:
  run        Execute an interactive prompt or one-shot task
  pipeline   Run a Markdown pipeline file non-interactively
  replay     Replay a trace from JSONL (display mode by default)
  cost       Summarize token usage and estimated cost across runs
  models     List available models or refresh the model catalog
  mcp        Inspect or configure MCP server connections
  skill      List, validate, or trigger workspace skills
  export     Export run logs as jsonl, markdown, or sharegpt
  config     Validate, explain, or show effective configuration

OPTIONS:
  --workspace <PATH>   Project workspace directory (default: current directory)
  --mode <MODE>        confirm | yolo | human | dry-run | replay
  --model <MODEL>      Override the session model
  --no-tui             Use plain stdout (implied when not attached to a TTY)
  --max-turns <N>      Maximum agent turns (default: 50)
```

### 16.2 Interactive Slash Commands

During a session, users control runtime state with slash commands handled locally by the harness — not sent to the model.

|Command|Effect|
|---|---|
|`/context`|Show structured context and token allocation by tier.|
|`/sources`|List active files, URLs, summaries, and token estimates.|
|`/add <path\|url>`|Add a file or URL to the active workspace index.|
|`/drop <id>`|Remove a source from the active session context.|
|`/mode <mode>`|Switch execution mode mid-session.|
|`/skill <name>`|Load or trigger a specific workspace skill.|
|`/compact`|Compress older turns into summary fragments.|
|`/cost`|Show current token usage and estimated cost.|
|`/export <format>`|Export the current run as `jsonl`, `md`, or `sharegpt`.|
|`/quit`|End the session.|

### 16.3 Session Output Preview

```
$ gestalt run --workspace ./research \
  "Summarize the papers in /sources and output a synthesis"

━━━ gestalt v0.1.0 ━━━
workspace: /home/user/research
model: claude-sonnet-4-6 · anthropic
mode: confirm

loaded:
  workspace.md
  memory.md                12 entries
  sources                  3 files in /sources
  tools                    bash, read, write, patch, search, pdf, web_fetch
  mcp                      brave-search

[turn 1] Discovering source files
  ⚙ bash: find /sources -name "*.pdf"   risk: LOW · auto-approved
    → 3 results

  ⚙ pdf: read paper1.pdf pages 1-8
    → 4,200 tokens

[turn 2] Extracting claims
  ✎ write: /docs/synthesis-draft.md    risk: MEDIUM · approved
    → created

[turn 3] Processing remaining sources
  ⚙ pdf: read paper2.pdf   → 3,100 tokens
  ⚙ pdf: read paper3.pdf   → 2,800 tokens

[turn 4] Finalizing synthesis
  ⚙ bash: wc -l /docs/synthesis-draft.md   risk: LOW · auto-approved
    → 142 lines
  ✎ write: /docs/synthesis-draft.md   risk: MEDIUM · approved
    → updated +38 lines

━━━ Session complete ━━━
turns: 4
tokens: 12,400 input · 2,300 output
estimated cost: $0.19
output: /docs/synthesis-draft.md

Memory proposal:
  + Papers in /sources were synthesized on 2026-05-30.
  + Output saved to /docs/synthesis-draft.md.
  + Conflict noted: paper2 disputes paper1's sample-size claim.

Accept memory update? [y/N]
```

### 16.4 Pipeline Mode

Pipeline files are plain Markdown with TOML front matter. They are versionable, reviewable in pull requests, and runnable in CI or cron without a TTY.

```markdown
---
model: claude-sonnet-4-6
max_turns: 40
mode: yolo
---

# Weekly Competitor Brief

## Goal

Produce an updated competitor brief from recent sources and prior workspace notes.

## Steps

1. Search for competitor news published in the last 7 days.
2. Extract product announcements, pricing changes, leadership moves, and notable press.
3. Compare against `/docs/competitor-brief-prior.md`.
4. Tag net-new items with `[NEW]`.
5. Write the updated brief to `/docs/competitor-brief-{date}.md`.
6. Append a one-line digest to `/memory.md` under `## Weekly Digests`.
```

---

## 17. Configuration

Configuration is explicit, layered, and safe by default. Secrets are never stored in config files.

### 17.1 Configuration Hierarchy

Configuration is resolved in this order, from highest to lowest priority:

```
1. CLI flags
2. Environment variables
3. Workspace config:  .gestalt/config.toml
4. Global config:     ~/.config/gestalt/config.toml
5. Built-in defaults
```

### 17.2 `config.toml` Schema

```toml
[defaults]
provider = "anthropic"
model = "claude-sonnet-4-6"
mode = "confirm"
max_turns = 50

[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"

[providers.openai]
api_key_env = "OPENAI_API_KEY"
org_id_env = "OPENAI_ORG_ID"

[providers.ollama]
base_url = "http://localhost:11434"

[tools]
bash_timeout_secs = 60
max_output_tokens = 4000
sandbox_type = "none"   # none | bubblewrap | docker (bubblewrap/docker: Phase 3)

[context]
max_context_window = 120000
reserved_output_tokens = 8000
summary_threshold_tokens = 8000

[observe]
run_log_dir = ".gestalt/runs"
log_format = "jsonl"
token_alert_threshold = 100000
```

API keys and secrets must always come from environment variables. Configuration files define behavior, not credentials.

### 17.3 `workspace.md` Front Matter

A workspace may override session defaults in TOML front matter:

```markdown
---
model = "claude-opus-4-7"
mode = "confirm"
max_turns = 80
---

# Q3 Competitive Analysis

**Goal:** Produce a 15-page competitive landscape report for C-suite readers.

**Tone:** Direct, data-anchored, no filler.

**Source policy:** Prioritize primary sources — filings, earnings calls, product docs, official statements.

**Output standard:** Every material claim must cite a specific source.
```

`workspace.md` provides high-priority context, but system policy always wins. It cannot grant itself permissions that the policy layer would deny.

### 17.4 Config Validation

```bash
gestalt config validate        # Check for errors, unknown keys, missing credentials
gestalt config explain         # Describe what each active setting does
gestalt config effective       # Show the merged configuration after all layers apply
```

---

## 18. Governance & Permissions

The policy engine runs before any tool executes. The agent proposes actions; the policy layer decides whether those actions are allowed, need confirmation, or are denied outright. Policy evaluation is deterministic, logged, and replayable.

### 18.1 Declarative Policy File

```toml
# .gestalt/policies.toml

[paths]
allow_read  = [".", "sources/", "docs/", "src/"]
allow_write = ["docs/", "reports/", "src/", ".gestalt/"]
deny_write  = [".git/", "secrets/", ".env", "*.key"]

[tools.bash]
default      = "confirm"
yolo_allow   = ["cargo test", "cargo check", "cargo build",
                "ls", "grep", "rg", "find", "cat"]
always_confirm = ["rm", "sudo", "docker", "git push",
                  "git reset", "ssh", "curl", "wget"]
always_deny  = ["dd", "mkfs", "fdisk", "chmod 777"]

[network]
default        = "confirm"
allow_domains  = ["arxiv.org", "github.com", "crates.io", "docs.rs"]
deny_domains   = []

[mcp]
default     = "confirm"
yolo_allow  = ["brave-search"]
```

The policy file is conservative by default. Project teams loosen permissions only where they understand and accept the risk.

### 18.2 Risk Level Reference

|Level|Examples|Default behavior (`confirm` mode)|
|---|---|---|
|Low|Read-only commands, local search, allowed-path file reads|Auto-execute|
|Medium|File writes inside allowed paths, tests, builds|Confirm or show diff|
|High|Network access, shell commands with side effects, external services|Require explicit approval|
|Critical|Credential access, destructive commands, system mutation, external writes|Deny unless explicitly allowlisted|

Risk is determined by both the tool type and the specific input. `git status` and `rm -rf target/` are both bash commands; they receive very different classifications.

### 18.3 Every Decision Is Logged

Every policy decision — the proposed tool name, input summary, risk level, policy source, decision, and reason — is written to the trace as a first-class event. Users can audit every approval, every auto-execute, and every denial after the fact.

---

## 19. Verification

Verification checks whether generated outputs satisfy task-specific invariants. It runs after generation: the agent writes, then the verification layer checks the result.

### 19.1 Output Classes

|Output type|Verification examples|
|---|---|
|Code|`cargo test`, `cargo clippy`, build check|
|Research|Citation exists in trace, quote matches source chunk, claim has indexed support|
|Data|Script reruns without error, schema matches expected structure, row counts are stable|
|Architecture|ADR constraints respected, forbidden tools avoided, policy compliance confirmed|

### 19.2 Verification Results

Verification results are emitted as first-class trace events. They are:

- Visible in the session output (pass/fail/warning count).
- Logged to the JSONL trace for replay and regression testing.
- Queryable via `gestalt export` for reporting pipelines.

Verification failures can block completion (hard fail), warn without blocking (soft fail), or trigger a repair turn where the agent attempts to fix the identified issue before retrying.

---

## 20. Observability

Observability is built around three layers: JSONL run logs for replay and audit, cost reports for token and model usage, and structured tracing for runtime debugging.

### 20.1 Run Log Structure

Each session writes a JSONL trace to:

```
.gestalt/runs/<timestamp>-<session-id>/
├── trace.jsonl     # One event per line; source of truth for replay and cost
├── summary.md      # Human-readable session summary
├── cost.json       # Token usage and estimated spend
└── artifacts/      # Full tool outputs that exceeded the inline size limit
```

The trace is append-only. If a session fails mid-run, the partial trace is still valid and inspectable.

### 20.2 Cost Analysis

```
$ gestalt cost .gestalt/runs/

━━━ Cost Analysis: .gestalt/runs/ ━━━
sessions: 14
turns: 187
period: 2026-05-24 → 2026-05-31

tokens:
  input:  980,000
  output: 260,000
  total:  1,240,000

estimated cost: $3.24

by model:
  claude-sonnet-4-6   1,100,000 tokens   $2.87
  claude-opus-4-7       140,000 tokens   $0.37
```

### 20.3 Export Formats

```bash
gestalt export ./runs/latest --format markdown     # Human-readable session report
gestalt export ./runs/latest --format jsonl        # Raw event stream
gestalt export ./runs/latest --format sharegpt     # Fine-tuning data export
```

### 20.4 Replay Modes

|Mode|What it does|Suitable for|
|---|---|---|
|`display` (default)|Renders recorded events verbatim. No model calls, no tool execution.|Reviewing any prior session|
|`deterministic`|Re-runs local tool calls and diffs against recorded results.|Regression-testing tool behavior|
|`regression`|Re-runs full session and checks semantic output invariants.|End-to-end quality checks|

`gestalt replay` without a `--mode` flag defaults to `display`. Re-execution requires explicit opt-in.

---

## 21. Roadmap & Build Order

### Phase 1 — Core Loop & Local Substrates (v0.1)

Target: a usable, reliable single-agent loop with a minimal policy gate and the most critical tools.

**In scope:**

- `gestalt-core`: agent loop, provider trait, tool trait, event model, session types, minimal policy trait
- `gestalt-models`: Anthropic and OpenAI SSE stream adapters, model catalog
- `gestalt-tools`: `BashTool`, `ReadTool`, `WriteTool`, `PatchTool`, `SearchTool`, `WebFetchTool`
- `gestalt-policy`: minimal path/network/bash policy, confirm/yolo/deny routing, `policies.toml` parser
- `gestalt-exec`: `NoSandbox` (subprocess + timeout + output cap + env allowlist)
- `gestalt-trace`: JSONL writer and `gestalt replay --mode display`
- `gestalt-cli`: `run`, `replay`, `cost`, `config validate` commands; plain-stdout mode
- Integration tests with mock provider (no live API calls in CI)
- `cargo install gestalt-harness` works on Linux and macOS

**Not in scope for v0.1:** PDF tool, MCP client, pipeline mode, skills, vector index, TUI, WASM.

### Phase 2 — Knowledge Ingestion & Policy Maturity (v0.2)

- `gestalt-policy`: full `policies.toml` grammar, MCP tool-level permissions, skill permissions, approval UX
- `gestalt-docs`: PDF ingestion (`pdfium-render`), HTML extraction, Markdown chunking; source cache with content hashing
- `gestalt-mcp`: MCP client over stdio and HTTP SSE; capability negotiation; tool registration; trust boundary enforcement
- `gestalt-memory`: `memory.md` parser, memory proposal generation at session end, deduplication before proposal
- `gestalt-index`: lexical workspace search index (BM25 / ripgrep); source summary cache
- Additional providers: Mistral, Groq, Ollama
- Citation contract enforcement and `CitationVerifier`
- `gestalt skill` command, SKILL.md parser, three-phase disclosure, skill trust levels
- `gestalt replay --mode deterministic`

### Phase 3 — Autonomy, Scheduling & Embedding (v0.3)

- Pipeline mode: Markdown pipeline parser, sequential task execution, run diffs
- Session resumability: replay partial sessions, continue interrupted runs
- Sub-agent spawning: delegate bounded tasks to a child loop instance
- Sandbox implementations: `BubblewrapSandbox` (Linux), `DockerSandbox` (cross-platform)
- `gestalt export --format sharegpt` for fine-tuning data
- OpenTelemetry span export (optional feature flag)
- WASM build target for embedding in the Gestalt frontend
- Programmatic tool calling via `code_execution` for data-heavy multi-tool workflows
- `gestalt replay --mode regression`
- Community skill registry: `gestalt skill fetch <name>`
- Remote task execution stub (see architecture §18)

---

## 22. What gestalt-harness Is Not

|Not this|Why it matters|
|---|---|
|A heavy multi-agent planning framework|It is a single-agent loop executor. Multi-agent coordination is built above the harness library, not inside it.|
|A framework that owns your application|`gestalt-core` is a library. Your application calls it. The harness does not prescribe routing or data models beyond the agent loop interface.|
|An autonomous daemon|Every run operates within explicit turn limits and token budgets. The harness does not run in the background without clear triggers.|
|A shell wrapper replacement|The harness delegates system commands to bash. It does not reimplement `grep`, `find`, `git`, or `make`.|
|A cloud service|Single binary. No telemetry. No account required. Run logs stay on your disk.|
|Locked to Anthropic|Provider registration is open. Any provider implementing the `Provider` trait is a first-class citizen.|
|A replacement for Excel or SQL|The harness is a reasoning surface. It does not replace structured query engines.|
|Secure by magic|The policy engine and sandbox are explicit, configurable, and logged. Security is a property of your configuration, not an invisible background guarantee.|

---

## Appendix A: AGENTS.md

This file governs how AI agents and human contributors work in this repository.

```markdown
# AGENTS.md

## Code Standards

* **Compiler Lint Guard:** `#[deny(clippy::all, clippy::pedantic)]` must be present in all library crates,
  with targeted `#[allow]` for known false positives.
* **Panic Prevention:** No `unwrap()` in library code. `expect("descriptive message")` is allowed only
  in test code.
* **Error Handling:** No `panic!` on expected error paths in `gestalt-core` or `gestalt-tools`.
  Always return `Result<T, HarnessError>`.
* **Documentation:** All public API items must have rustdoc doc-comments.

---

## The Sacred Loop Contract

* **Size Restriction:** `gestalt-core/src/agent.rs` must stay under **200 lines**. If it grows beyond
  that, the addition belongs in middleware.
* **I/O Isolation:** `gestalt-core` must contain **zero** filesystem reads and **zero** HTTP calls.
* **Testability:** The agent loop must be fully verifiable via mock providers and mock tools with no
  live API keys required.

---

## Adding a Provider

1. Add the implementation to `gestalt-models/src/`.
2. Implement the `Provider` trait — specifically `stream()`, mapping to the unified `AgentEvent` stream.
3. Register in `registry.rs` via a factory closure (lazy, never eagerly instantiated).
4. Add to the `models.toml` generation script and the model catalog.
5. Add an integration test with a recorded HTTP cassette (no live API calls in CI).
6. Pass the provider normalization test matrix (see architecture §11.4).

---

## Adding a Tool

1. Add to `gestalt-tools/src/`.
2. Derive `schemars::JsonSchema` on the input struct — this is the tool's public contract.
3. Implement `risk()` — the policy engine depends on it before every execution.
4. Register in the default `ToolRegistry`.
5. Document timeout behavior and define what constitutes an error versus a valid empty result.
6. Write unit tests covering: happy path, schema validation errors, risk classification, and path
   traversal rejection.

---

## Git Rules (Parallel Agents)

* Never `git add -A`. Always use `git add <specific-files>`.
* Never use `git reset --hard` or `git checkout .`.
* Always include `fixes #N` in commit messages when closing an issue.
* Before committing, run `git status` and verify only your intended files are staged.

---

## Testing Requirements

* **New Tools:** Happy path, schema validation errors, risk classification, path traversal rejection.
* **New Providers:** Integration test with recorded HTTP cassette. No live API keys.
* **Loop Modifications:** Update the mock-provider integration test.
* **Pre-Commit:** If you create or modify a test file, run it locally until it fully passes before
  committing.
```

---

## Appendix B: Dependency Audit

Full list of allowed direct dependencies across all crates. `gestalt-core` has the strictest budget.

|Crate|Version|Role|
|---|---|---|
|`tokio`|1.x|Async runtime|
|`serde`|1.x|Serialization|
|`serde_json`|1.x|JSON support|
|`schemars`|0.8|Compile-time JSON Schema for tool contracts|
|`thiserror`|1.x|Typed error derivation|
|`futures`|0.3|Stream combinators|
|`async-trait`|0.1|Dynamic async trait objects|
|`reqwest`|0.12|HTTP client (TLS via `rustls`)|
|`tokio-stream`|0.1|Stream utilities|
|`eventsource-stream`|0.2|SSE parsing|
|`clap`|4.x|CLI argument parsing|
|`toml`|0.8|Config file parsing|
|`tracing`|0.1|Structured logging|
|`tracing-subscriber`|0.3|Log formatting|
|`tiktoken-rs`|0.5|Token counting (Anthropic / OpenAI tokenizers)|
|`pulldown-cmark`|0.11|Markdown parsing for context middleware|
|`chrono`|0.4|Timestamps in run logs (`gestalt-trace` only)|
|`uuid`|1.x|Session and event IDs (`gestalt-trace` only)|
|`dirs`|5.x|XDG config path resolution|
|`glob`|0.3|Path pattern matching for policies|
|`encoding_rs`|0.8|File encoding detection in `ReadTool`|
|`sha2`|0.10|Source content hashing in `gestalt-context`|
|`ratatui`|0.28|TUI — optional feature flag|
|`crossterm`|0.28|Terminal control — optional feature flag|
|`pdfium-render`|0.8|PDF text extraction — optional `pdf` feature|

Total: 25 direct deps across all crates. `gestalt-core` uses 7. All are actively maintained.

`OnceLock` is used for the provider registry (`std::sync::OnceLock`, stable since Rust 1.70). No `once_cell` dependency is needed.

---

## Appendix C: Engineering Efficiency Rules

Rules that prevent scope creep and keep the implementation fast to build and easy to reason about.

1. **One loop first.** Do not design multi-agent planning structures until the basic single loop behaves deterministically on real tasks. Composition is a Phase 3 concern.
    
2. **Bash first, not bash only.** Bash is the escape hatch for anything not worth a structured tool. Document ingestion and index building justify structured tools because they are significantly faster and more reliable than shell scripts.
    
3. **Events as the source of truth.** UI streams, debug reports, JSONL traces, test assertions, and cost analysis all consume the same `AgentEvent` pipeline. No separate logging path.
    
4. **Policy ships in v0.1.** Never deploy a loop with `BashTool`, `WriteTool`, or `WebFetchTool` and no policy gate. The minimal policy engine is not optional; it is the safety boundary that makes everything else safe to ship.
    
5. **Durable memory is human-editable.** Never write opaque binary models for long-term memory. Memory lives in `memory.md` — a versionable Markdown file the user reads, edits, and commits to git.
    
6. **Standard library over extra dependencies.** `std::sync::OnceLock` before `once_cell`. `std::collections::HashMap` before `indexmap`. Each external dependency is a maintenance commitment and a compile-time cost.
    
7. **The full turn before tool execution.** Never execute a tool on a partial streamed turn. Accumulate the complete assistant message first. This is not a performance optimization; it is required for correctness with providers that emit multiple tool calls per turn.
    

---

## Appendix D: Open Questions

1. **Vector index for Phase 2:** Should the workspace index run purely on lexical algorithms (BM25, ripgrep) through Phase 2, or should we include a lightweight local vector library? Candidates: `sqlite-vss`, `usearch`, or a pure-Rust HNSW implementation. Tradeoff: semantic accuracy vs. binary size and compile time.
    
2. **Citation verification completeness:** What is the minimum runtime requirement to make citations fully verifiable under local execution? Chunk byte offsets in the JSONL (current design) versus a content hash sufficient for matching? Does page-level granularity satisfy the research use case or do we need paragraph-level?
    
3. **Confirm mode UX without TUI:** In plain-stdout mode, how does the user review and edit proposed file writes before committing? Options: open `$EDITOR` on a diff; inline patch editing via stdin; a `--patch` flag that lists proposed changes and prompts per-item.
    
4. **Skill community registry governance:** Who owns the registry, how are skills versioned, and how is trust established? Options: a git-based registry (Homebrew tap model), a dedicated index service with a signing key model, or a curated official set with a community contribution process.
    
5. **Cross-platform execution:** v0.1 officially targets Linux and macOS with bash-compatible shells. Windows support requires a `ShellKind` abstraction (bash / PowerShell / cmd) and a decision on whether WSL is the recommended path or whether native Windows support is worth the engineering cost.
    
6. **Pipeline step types:** Plain Markdown pipeline steps are human-readable but their executable semantics are implicit. Should Phase 3 pipeline mode introduce a structured step YAML block inside Markdown (agent task / verify / write / human-approval) or infer step intent from prose?

---

## Appendix E: Success Metrics

|Metric|Definition|Target|
|---|---|---|
|**Loop Comprehension Time**|Time for a Rust developer unfamiliar with the project to read and understand `agent.rs` completely|< 30 minutes|
|**Time to First Value**|Time from `cargo install gestalt-harness` to a successful run synthesizing local Markdown files|< 5 minutes|
|**Trace Replay Success Rate**|Fraction of tool-free deterministic runs that reproduce identical outputs from JSONL replay|> 95%|
|**Citation Accuracy**|Fraction of agent-generated citations that verify against their source chunks|> 90% in knowledge mode|
|**Binary Size**|Stripped release binary, default features, Linux x86-64|< 10 MB|
|**Cold Start**|Time from invocation to first token streamed, interactive mode, no workspace|< 100ms|
|**Policy Coverage**|Fraction of tool calls in test suite that pass through a policy gate before execution|100%|
|**Core Compile Time**|Time to compile `gestalt-core` from scratch on a modern developer machine|< 30 seconds|

---

_gestalt-harness PRD v3.0 — Maintained alongside `gestalt-harness-architecture.md`_