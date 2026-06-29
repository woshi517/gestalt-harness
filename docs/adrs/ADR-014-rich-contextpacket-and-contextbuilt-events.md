# ADR-014: Rich ContextPacket and ContextBuilt Events

**Status:** Accepted

## Context
The context compilation pipeline returned only a list of messages, discarding metadata such as tokenizer ID, trust boundary tags, omissions, and sources. This prevented offline context analysis, replication, or validation of context-trimming policies.

## Decision
Return a structured `ContextPacket` from the pipeline. Compute a stable, deterministic SHA-256 hash over the compiled messages and parameters. Emit a `ContextBuilt` event containing the packet hash, source lists, and omissions.

## Consequences
Context becomes a deterministic, inspectable, and replay-ready artifact. Observability tools can track exactly what content (and which trust tier) was sent to the model per turn.
