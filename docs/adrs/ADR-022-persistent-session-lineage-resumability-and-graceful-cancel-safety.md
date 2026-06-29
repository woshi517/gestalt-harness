# ADR-022: Persistent Session Lineage, Resumability, and Graceful Cancel-Safety

**Status:** Accepted

## Context
The harness previously ran one-shot in-memory sessions that were tightly coupled to individual run directories, without persistent lineage or a graceful interruption mechanism. Process termination (e.g., Ctrl+C) risked orphaned tool subprocesses, unflushed trace events, and incomplete run metadata. Additionally, resuming an interrupted run or branching from previous checkpoints was unsupported, and parallel tool tracking relied on a single boolean which could lead to tracking ambiguity.

## Decision
Introduce stable logical session IDs separate from physical run IDs (with suffix and manifest-based UUID resolution in the CLI). Checkpoint assistant-only turns on completion, flush traces durably upon interruption, and introduce a `CancellationToken` propagating cancel-safety through all agent phases (approval, policy, hooks, tool executors). Convert blocking stdin prompts into interruptible, asynchronous readers. Replace single-boolean parallel tool tracking with a tool ID `HashSet`. Verify execution environment compatibility via compatibility fingerprints stored in manifests and checked during preflight, and ensure lineage-aware prune and delete operations block or cascade descendant runs.

## Consequences
Clear separation between logical sessions and physical runs. Resuming and branching from committed checkpoints are fully safe, deterministic, and prevent duplicate execution of side-effects. Graceful cancel-safety ensures zero orphaned processes or trace loss.
