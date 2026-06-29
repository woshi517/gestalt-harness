# Architecture Decision Records (ADRs)

This index lists the Architecture Decision Records (ADRs) accepted for `gestalt-harness`.
ADRs 001–022 are documented in the [architecture document](../gestalt-harness-architecture.md). ADRs 023+ are standalone files in this directory.

Accepted ADRs are authoritative for architectural decisions. Feature
specifications and implementation plans may propose changes, but they must
amend or supersede the affected ADR before implementation. See the
[documentation map](../README.md) for the complete authority hierarchy.

ADRs 001–022 should be extracted into standalone files as part of documentation
hardening. Until extraction is complete, the linked sections remain their
canonical records.

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-001](./ADR-001-inverted-crate-dependency-direction.md) | Inverted Crate Dependency Direction | Accepted |
| [ADR-002](./ADR-002-full-turn-accumulation-before-tool-execution.md) | Full Turn Accumulation Before Tool Execution | Accepted |
| [ADR-003](./ADR-003-toolexecutionresult-separate-from-tooloutput.md) | ToolExecutionResult Separate from ToolOutput | Accepted |
| [ADR-004](./ADR-004-policyrequest-struct-over-nameinput-pair.md) | PolicyRequest Struct Over name/input Pair | Accepted |
| [ADR-005](./ADR-005-approvalprovider-as-an-injectable-interface.md) | ApprovalProvider as Injectable Interface | Accepted |
| [ADR-006](./ADR-006-three-replay-modes-display-deterministic-regression.md) | Three Replay Modes | Accepted |
| [ADR-007](./ADR-007-eventenvelope-in-gestalt-trace-not-gestalt-core.md) | EventEnvelope in gestalt-trace, Not gestalt-core | Accepted |
| [ADR-008](./ADR-008-minimal-policy-ships-in-v01.md) | Minimal Policy Ships in v0.1 | Accepted |
| [ADR-009](./ADR-009-sandbox-deferred-to-v02.md) | Sandbox Deferred to v0.2+ | Accepted |
| [ADR-010](./ADR-010-contenttrust-tags-on-all-external-content.md) | ContentTrust Tags on All External Content | Accepted |
| [ADR-011](./ADR-011-credential-resolution-boundary-separate-from-provider-behavior-config.md) | Credential Resolution Boundary Separate from Provider Behavior Config | Accepted |
| [ADR-012](./ADR-012-preserve-provider-finish-reasons-in-normalized-stopreason.md) | Preserve Provider Finish Reasons in Normalized StopReason | Accepted |
| [ADR-013](./ADR-013-bounded-session-approval-grants-instead-of-tool-name-keys.md) | Bounded Session Approval Grants instead of Tool-Name Keys | Accepted |
| [ADR-014](./ADR-014-rich-contextpacket-and-contextbuilt-events.md) | Rich ContextPacket and ContextBuilt Events | Accepted |
| [ADR-015](./ADR-015-artifact-spillover-for-truncated-tool-output.md) | Artifact Spillover for Truncated Tool Output | Accepted |
| [ADR-016](./ADR-016-honest-host-execution-nosandbox-and-network-policy-enforcement.md) | Honest Host Execution (NoSandbox) and Network Policy Enforcement | Accepted |
| [ADR-017](./ADR-017-dedicated-verification-substrate-gestalt-verify.md) | Dedicated Verification Substrate (gestalt-verify) | Accepted |
| [ADR-018](./ADR-018-internal-lifecycle-hooks-for-extensibility.md) | Internal Lifecycle Hooks for Extensibility | Accepted |
| [ADR-019](./ADR-019-workspace-state-snapshotting-in-session-metadata.md) | Workspace State Snapshotting in Session Metadata | Accepted |
| [ADR-020](./ADR-020-trace-driven-regression-testing-via-golden-traces.md) | Trace-Driven Regression Testing via Golden Traces | Accepted |
| [ADR-021](./ADR-021-default-system-prompt-with-local-policy-overrides.md) | Default System Prompt with Local Policy Overrides | Accepted |
| [ADR-022](./ADR-022-persistent-session-lineage-resumability-and-graceful-cancel-safety.md) | Persistent Session Lineage, Resumability, and Graceful Cancel-Safety | Accepted |
| [ADR-023](./ADR-023-runtime-composition-layer.md) | Runtime Composition Layer | Accepted |
| [ADR-024](./ADR-024-process-extensions.md) | Process-Backed Extensions over Stdio JSON-RPC | Accepted |
| [ADR-025](./ADR-025-unified-gestalt-json-config.md) | Unified `gestalt.json` Configuration | Accepted |
| [ADR-026](./ADR-026-cache-aware-prompt-assembly.md) | Cache-Aware Prompt Assembly | Accepted |
| [ADR-027](./ADR-027-mcp-client-integration.md) | Model Context Protocol (MCP) Client Integration | Accepted |
| [ADR-028](./ADR-028-extension-package-components.md) | Extension Package Components | Accepted |
| [ADR-029](./ADR-029-runtime-snapshot-reload.md) | Runtime Snapshot Reload | Accepted |
| [ADR-030](./ADR-030-lifecycle-protocol-v2.md) | Lifecycle Protocol V2 | Accepted |
| [ADR-031](./ADR-031-v0-1-greenfield-compatibility-cutoff.md) | v0.1 Greenfield Compatibility Cutoff | Accepted |

ADR-023: Introduces the gestalt-runtime crate as the primary orchestration and composition shell above the pure kernel (gestalt-core).

ADR-024: Implements process-backed extensions executed in separate child processes over stdio using newline-delimited JSON-RPC 2.0 protocol.

ADR-025: Consolidates workspace-scoped and global harness configuration into a
single `gestalt.json` per scope. Its legacy TOML fallback and migration-seeding
clauses are superseded by ADR-031.

ADR-026: Introduces `PromptAssemblyStrategy` (Snapshot/Dynamic), `ContextStability` classification, and cache-aware context compilation that preserves provider prompt-cache hit rates by separating stable session context from turn-specific context.

ADR-027: Integrates Model Context Protocol (MCP) clients with standard stdio transport, lazy client lifecycle pooling via OnceCell, canonical collision-safe tool naming, secure host-side risk annotations, and event-bus notifications.

ADR-028: Separates runtime modules, packages, components, configured instances, process instances, runtime generations, and client/product descriptors. Client/product descriptors are inventory for embedding hosts and are not executed by the runtime.

ADR-029: Focuses on extension activation pipeline, generation snapshots, lease management, candidate validation, and transactional hot reload draining.

ADR-030: Replaces generic external hooks with typed capabilities: context providers, policy guards, turn routers, external verifiers, and event observers in Lifecycle Protocol V2.

ADR-031: Establishes stable v0.1 as a greenfield compatibility boundary,
removing legacy harness TOML, extension manifest/protocol V1, deprecated Rust
APIs, and pre-hardening persistence migrations. Known legacy harness config
files fail with `UNSUPPORTED_LEGACY_CONFIG` without parsing or migration.
