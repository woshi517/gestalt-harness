---
type: review
area: harness-engineering
title: v0.1 Harness Engineering Review Findings & Hardening
date: 2026-06-01
---

# v0.1 Harness Engineering Review Findings & Hardening

During the pre-release review of `gestalt-harness` v0.1, a harness-engineering audit identified several safety, context, verification, and observability gaps that undermined core execution invariants. This document records those findings, the corresponding invariants affected, and the hardening measures implemented.

## 1. Safety & Permission Gaps (Invariants: *No Unbounded Action*, *No Unaudited Permission*)

### Findings
- **Overly Broad Session Approvals:** A session-wide "always allow" grant stored only the tool name. Once a user approved a benign command like `bash echo hello`, the system would auto-approve subsequent dangerous calls like `bash rm -rf /` under the same tool.
- **Honesty in Sandboxing:** The `NoSandbox` execution backend was described as a sandbox but ran directly on the host with no mount, network, or process namespaces, making working directory check the only form of validation.
- **Interpreter Wrappers & Secret Paths:** The `bash` risk classifier was prefix-based, meaning wrapped commands (e.g. `python -c "..."` or shell metacharacters like `&&`, `||`, `|`, etc.) would bypass protections or default to low risk, exposing sensitive environment files (e.g., `.env`, private keys).

### Hardening Measures
1. **Bounded Session Grants:** Replaced the plain tool-name session grant check with a narrower `SessionGrant` that binds approval to a specific tool, input hash, risk ceiling, and matched policy rule. Future calls must re-run policy and only auto-approve if they are no riskier and match the same fingerprint.
2. **Explicit Sandbox Boundaries:** Replaced misleading sandbox descriptors with clear warnings identifying `NoSandbox` as unconfined host execution. Defaulted all non-allowlisted `bash` commands in `yolo` mode to require explicit human confirmation.
3. **Robust Risk Classification:** Upgraded the `bash` risk classifier to treat interpreter wrappers (`python`, `sh`, `bash`, `sudo`, `xargs`, etc.) and commands containing shell metacharacters as high-risk by default. Capped command inputs referencing sensitive paths (e.g. `.env`, `.key`, `.pem`, `secret`) as high-risk deny candidates.

## 2. Context & Verification Gaps (Invariants: *No Opaque Context*, *No Unverifiable Artifact*)

### Findings
- **Transient Context Packages:** Ccontext compiling produced only a raw message vector, discarding the rich metadata (pipeline version, omitted items, trust tags, and tokenizer stats) returned by the pipeline.
- **Lack of Output Spillover:** Truncated tool outputs (exceeding maximum token/byte caps) were dropped silently in memory, rendering them unavailable in trace files and audit replays.
- **Verification Placeholder:** Verification remained a simple event enum placeholder with no actual validator implementations or registry execution in the main loop.
- **Lack of Default/Overridable System Prompt:** v0.1 lacked a standard, non-forking way to configure the agent's identity, policy constraints, and output rules via configuration.

### Hardening Measures
1. **ContextPacket Integration:** Promoted `ContextPipeline` to output a deterministic `ContextPacket` capturing packet hash, tokenizer metadata, message hashes, context source refs, and omissions (retaining provenance for dropped items).
2. **Artifact Spillover:** Configured the executor to save the full content of truncated outputs to `artifacts/` under the run directory. Added artifact path, size, MIME type, and SHA-256 hashes to the `ToolResult` event.
3. **Verifier Registry Crate:** Created the `gestalt-verify` crate establishing the `Verifier` trait, `VerifierRegistry`, and five core verification filters (`CommandVerifier`, `FileExistsVerifier`, `NoSecretsVerifier`, `PatchAppliesVerifier`, and `MarkdownStructureVerifier`).
4. **Default Overridable System Prompt:** Implemented a standard default system prompt covering identity, environment, tool policy, and output rules. Allowed overriding this default prompt via `.gestalt/policies.toml` (`prompt.override` / `prompt.override_file`), enforcing trust tags on custom prompts.

## 3. Observability & Lifecycle Gaps (Invariants: *No Throwaway Trtraces*, *No Invisible Action*)

### Findings
- **Silent Trace Failures:** CLI silently swallowed trace write errors, meaning execution could continue even if "ground truth" trace logs were failing to persist.
- **Insufficient Event Schema:** Events lacked runtime details (e.g., risk levels, duration, directory contexts, matched rules) needed for full replayability.
- **Uncorrelated Workspace States:** Traces were not tied to the local workspace state, making it impossible to correlate execution events to a specific version or dirty state of files.

### Hardening Measures
1. **Durable Trace Sinks:** Configured `run_prompt` to monitor trace sink emission failures, logging errors immediately and aborting the session if a write failure threshold is reached.
2. **Richer Event Protocol:** Expanded the public JSONL trace schema with fields for risk levels, temperature, durations, directories, artifact references, and approval provenance.
3. **Workspace Snapshots:** Implemented `WorkspaceSnapshot` (git SHA, dirty flag, untracked count, and a deterministic SHA-256 content hash of all tracked files) captured at session start and updated on demand. The snapshot ID is embedded in every trace envelope and the run summary.
