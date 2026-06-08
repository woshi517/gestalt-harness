# Architecture Decision Records (ADRs)

This index lists the Architecture Decision Records (ADRs) accepted for `gestalt-harness`.
ADRs 001–022 are documented in the [architecture document](../gestalt-harness-architecture.md). ADRs 023+ are standalone files in this directory.

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-001](../gestalt-harness-architecture.md#adr-001-inverted-crate-dependency-direction) | Inverted Crate Dependency Direction | Accepted |
| [ADR-002](../gestalt-harness-architecture.md#adr-002-full-turn-accumulation-before-tool-execution) | Full Turn Accumulation Before Tool Execution | Accepted |
| [ADR-003](../gestalt-harness-architecture.md#adr-003-toolexecutionresult-separate-from-tooloutput) | ToolExecutionResult Separate from ToolOutput | Accepted |
| [ADR-004](../gestalt-harness-architecture.md#adr-004-policyrequest-struct-over-nameinput-pair) | PolicyRequest Struct Over name/input Pair | Accepted |
| [ADR-005](../gestalt-harness-architecture.md#adr-005-approvalprovider-as-an-injectable-interface) | ApprovalProvider as Injectable Interface | Accepted |
| [ADR-006](../gestalt-harness-architecture.md#adr-006-three-replay-modes-display-deterministic-regression) | Three Replay Modes | Accepted |
| [ADR-007](../gestalt-harness-architecture.md#adr-007-eventenvelope-in-gestalt-trace-not-gestalt-core) | EventEnvelope in gestalt-trace, Not gestalt-core | Accepted |
| [ADR-008](../gestalt-harness-architecture.md#adr-008-minimal-policy-ships-in-v01) | Minimal Policy Ships in v0.1 | Accepted |
| [ADR-009](../gestalt-harness-architecture.md#adr-009-sandbox-deferred-to-v02) | Sandbox Deferred to v0.2+ | Accepted |
| [ADR-010](../gestalt-harness-architecture.md#adr-010-contenttrust-tags-on-all-external-content) | ContentTrust Tags on All External Content | Accepted |
| [ADR-011](../gestalt-harness-architecture.md#adr-011-credential-resolution-boundary-separate-from-provider-behavior-config) | Credential Resolution Boundary Separate from Provider Behavior Config | Accepted |
| [ADR-012](../gestalt-harness-architecture.md#adr-012-preserve-provider-finish-reasons-in-normalized-stopreason) | Preserve Provider Finish Reasons in Normalized StopReason | Accepted |
| [ADR-013](../gestalt-harness-architecture.md#adr-013-bounded-session-approval-grants-instead-of-tool-name-keys) | Bounded Session Approval Grants instead of Tool-Name Keys | Accepted |
| [ADR-014](../gestalt-harness-architecture.md#adr-014-rich-contextpacket-and-contextbuilt-events) | Rich ContextPacket and ContextBuilt Events | Accepted |
| [ADR-015](../gestalt-harness-architecture.md#adr-015-artifact-spillover-for-truncated-tool-output) | Artifact Spillover for Truncated Tool Output | Accepted |
| [ADR-016](../gestalt-harness-architecture.md#adr-016-honest-host-execution-nosandbox-and-network-policy-enforcement) | Honest Host Execution (NoSandbox) and Network Policy Enforcement | Accepted |
| [ADR-017](../gestalt-harness-architecture.md#adr-017-dedicated-verification-substrate-gestalt-verify) | Dedicated Verification Substrate (gestalt-verify) | Accepted |
| [ADR-018](../gestalt-harness-architecture.md#adr-018-internal-lifecycle-hooks-for-extensibility) | Internal Lifecycle Hooks for Extensibility | Accepted |
| [ADR-019](../gestalt-harness-architecture.md#adr-019-workspace-state-snapshotting-in-session-metadata) | Workspace State Snapshotting in Session Metadata | Accepted |
| [ADR-020](../gestalt-harness-architecture.md#adr-020-trace-driven-regression-testing-via-golden-traces) | Trace-Driven Regression Testing via Golden Traces | Accepted |
| [ADR-021](../gestalt-harness-architecture.md#adr-021-default-system-prompt-with-local-policy-overrides) | Default System Prompt with Local Policy Overrides | Accepted |
| [ADR-022](../gestalt-harness-architecture.md#adr-022-persistent-session-lineage-resumability-and-graceful-cancel-safety) | Persistent Session Lineage, Resumability, and Graceful Cancel-Safety | Accepted |
| [ADR-023](./ADR-023-runtime-composition-layer.md) | Runtime Composition Layer | Accepted |
| [ADR-024](./ADR-024-process-extensions.md) | Process-Backed Extensions over Stdio JSON-RPC | Accepted |
| [ADR-025](./ADR-025-unified-gestalt-json-config.md) | Unified `gestalt.json` Configuration | Accepted |
| [ADR-026](./ADR-026-cache-aware-prompt-assembly.md) | Cache-Aware Prompt Assembly | Accepted |

ADR-023: Introduces the gestalt-runtime crate as the primary orchestration and composition shell above the pure kernel (gestalt-core).

ADR-024: Implements process-backed extensions executed in separate child processes over stdio using newline-delimited JSON-RPC 2.0 protocol.

ADR-025: Consolidates workspace-scoped and global harness configuration into a single `gestalt.json` per scope, with JSON-first loading, legacy TOML fallback, and transparent migration seeding.

ADR-026: Introduces `PromptAssemblyStrategy` (Snapshot/Dynamic), `ContextStability` classification, and cache-aware context compilation that preserves provider prompt-cache hit rates by separating stable session context from turn-specific context.

