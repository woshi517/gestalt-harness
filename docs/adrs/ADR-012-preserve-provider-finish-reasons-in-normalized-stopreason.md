# ADR-012: Preserve Provider Finish Reasons in Normalized StopReason

**Status:** Accepted

## Context
The original normalized `StopReason` enum was too small to represent common provider finish conditions like tool-use handoff, output truncation, and content filtering. Collapsing them into `EndTurn` or `ProviderError` would lose replay and audit fidelity.

## Decision
Extend normalized `StopReason` with `ToolUse`, `MaxOutput`, and `ContentFiltered` while keeping provider-native wire details out of `gestalt-core`. Providers map their finish reasons into these shared variants.

## Consequences
Replay, summaries, and diagnostics can distinguish normal turn completion from tool delegation, output truncation, and provider-side filtering. The loop still consumes a provider-agnostic contract.
