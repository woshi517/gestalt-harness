# ADR-015: Artifact Spillover for Truncated Tool Output

**Status:** Accepted

## Context
Capping model context required truncating large tool outputs. Simply dropping bytes at the trace boundary meant full output was lost for debugging or compliance, even though the system claimed full output was saved.

## Decision
Save full truncated tool outputs to `.gestalt/runs/<id>/artifacts/`. Emit `ArtifactCreated` events, and include path, size, MIME type, and SHA-256 hashes in `ToolResult` metadata.

## Consequences
Replay tools and human operators can inspect full outputs. Verification steps can run on the raw files even if the model only received a truncated summary.
