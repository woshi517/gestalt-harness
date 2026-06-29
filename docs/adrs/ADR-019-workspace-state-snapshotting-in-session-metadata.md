# ADR-019: Workspace State Snapshotting in Session Metadata

**Status:** Accepted

## Context
Traces captured what the agent did, but not the state of the codebase it acted upon. This made reproducing bugs, verifying code generation, and running offline replays highly dependent on the host environment's mutable state.

## Decision
Capture a `WorkspaceSnapshot` (git commit SHA, dirty flag, untracked file count, and a SHA-256 content hash of tracked files) at session start and refresh. Store the snapshot ID in the trace envelope and run summaries.

## Consequences
Runs are tied to a specific workspace state, making replays reproducible. Allows tools to verify if a workspace was modified before or during a run.
