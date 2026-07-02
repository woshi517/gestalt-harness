# Gestalt Documentation Inventory (H0A-F01)

This inventory lists every maintained Markdown file in the repository, assigning type, lifecycle status, owner, authority, and required hardening action.

| File Path | Type | Status | Target | Owner | Domain | Auth Rank | Canonical Replacement | Required Action |
|---|---|---|---|---|---|---|---|---|
| `docs/README.md` | Index | active | general | Docs Group | Metadata & Index | 1 (Index) | None | Harden authority rules and metadata specifications |
| `docs/gestalt-harness-prd.md` | PRD | active | general | Product | Philosophy & Scope | 6 | None | Simplify to vision/success metrics, remove code/ADRs |
| `docs/gestalt-harness-architecture.md` | Arch Overview | active | general | gestalt-runtime | System Architecture | 6 | Standalone ADRs | Extracted ADRs 001-022, reduce to high-level diagrams |
| `docs/gestalt-harnes-implementation-roadmap.md` | Plan | abandoned | historical | Project | Historical Roadmap | 7 | Smaller Phase Plans | Archive file, add redirect to phase plans |
| `docs/composition-hooks-guide.md` | Guide | active | v0.1 | gestalt-runtime | Lifecycle Extensibility | 4 | None | Update compiling hook examples to match V2 lifecycle |
| `docs/extension-development-guide.md` | Guide | active | v0.1 | gestalt-runtime | Extensions V2 | 4 | None | Ensure exclusively V2 content, remove V1 leftovers |
| `docs/extension-manifest-schema.md` | Reference | active | v0.1 | gestalt-runtime | Extensions V2 | 4 | None | Match schema properties exactly with V2 JSON schema |
| `docs/jsonrpc-extension-protocol.md` | Reference | active | v0.1 | gestalt-runtime | Extension Protocol | 4 | None | Remove references to JSON-RPC 1.0/V1 protocol |
| `docs/mcp-client-best-practices.md` | Guide | active | v0.1 | gestalt-runtime | MCP Integration | 4 | None | Ensure guide aligns with H0B event/tool decisions |
| `docs/permissions-model.md` | Reference | active | v0.1 | gestalt-runtime | Permissions | 4 | None | Detail V2 permissions checks (path, network, shell) |
| `docs/runtime-event-bus.md` | Reference | active | v0.1 | gestalt-runtime | Runtime events | 4 | None | Update with new H0B event decision schema |
| `docs/release-checklist.md` | Checklist | active | general | Release Mgmt | Release readiness | 4 | None | Harden G3 release-conformance checks |
| `docs/skill-specification.md` | Spec | active | v0.1 | gestalt-runtime | Skills | 3 | None | Keep skill trust layers and SKILL.md specs aligned |
| `docs/tui-design.md` | Spec | active | v0.1 | gestalt-tui | TUI Console | 3 | None | Reference TUI as client of gestalt-app service |
| `docs/feature-spec/config-extension.md` | Spec | superseded-in-part | v0.1 | gestalt-app | Configuration | 3 | `docs/v0.1/configuration.md`, `docs/v0.1/extensions.md` | Retain as explicitly superseded design history; do not use for released compatibility |
| `docs/feature-spec/context-projection-hardening.md` | Spec | active | v0.1 | gestalt-core | Context | 3 | None | Harden token-limit context trimming invariants |
| `docs/feature-spec/context-tool-compaction.md` | Spec | active | v0.1 | gestalt-core | Context Compaction | 3 | None | Specify compaction rules and checkpoints |
| `docs/feature-spec/crate-boundary.md` | Spec | implemented | general | Workspace | Workspace boundary | 3 | None | Update to reflect current five-crate architecture |
| `docs/feature-spec/model-schema-normalization.md` | Spec | active | v0.1 | gestalt-core | Providers | 3 | None | Align stream-event mapping with provider traits |
| `docs/feature-spec/product-neutral-extension-architecture.md` | Spec | proposed | post-v0.1 | gestalt-runtime | Extensions | 3 | `docs/v0.1/extensions.md` for v0.1 | Treat as long-term roadmap, not current v0.1 authority |
| `docs/feature-spec/v0.1-hardening.md` | Release Spec | active | v0.1 | Release Mgmt | Hardening Scope | 3 | None | The release-scope specification; update link mismatch |
| `docs/feature-spec/workspace-init.md` | Spec | active | v0.1 | gestalt-app | Workspace Init | 3 | None | Specify `gestalt.json` workspace initialization template |
| `docs/v0.1/README.md` | Version Index | active | v0.1 | Docs Group | v0.1 Contracts | 2 (Contract) | None | Keep publication states aligned with implementation gates |
| `docs/plans/v0.1-hardening/000-program-coordination-and-phase-gates.md` | Plan | active | v0.1 | Release Mgmt | Coordination | 5 | None | Update checklists and mark ready/complete |
| `docs/plans/v0.1-hardening/H0A-documentation-authority-and-contract-map.md` | Plan | completed | v0.1 | Docs Group | Documentation Map | 5 | None | Preserve as completed implementation evidence |
| `docs/plans/v0.1-hardening/H0B-api-spi-inventory-and-adr-decisions.md` | Plan | completed | v0.1 | Core/Runtime | API/SPI Classification | 5 | None | Preserve as completed implementation evidence |
| `docs/plans/v0.1-hardening/H0C-greenfield-removal-ledger-and-absence-checks.md` | Plan | completed | v0.1 | Runtime/App | removal tracking | 5 | None | Preserve as completed implementation evidence |
| `docs/plans/v0.1-hardening/pre-hardening-removal-ledger.md` | Tracker | active | v0.1 | Runtime/App | removal tracking | 5 | None | Create ledger and populate with legacy checks |
| `docs/plans/v0.1-hardening/api-spi-inventory.md` | Inventory | active | v0.1 | Core/Runtime | API Classification | 5 | None | Create inventory cataloging all public symbols |
| `docs/plans/v0.1-hardening/h0b-architectural-decisions.md` | Record | active | v0.1 | Core/Runtime | Decisions | 5 | None | Create document recording the six H0B decisions |
| `docs/plans/v0.1-hardening/coordination-tracker.md` | Tracker | active | v0.1 | Release Mgmt | Coordination | 5 | None | Create tracker for requirement map and status |
| `docs/adrs/README.md` | Index | active | general | Architecture | ADR authority | 1 (Index) | None | Maintain accepted/superseded decision index |
| `docs/adrs/ADR-001-inverted-crate-dependency-direction.md` | ADR | accepted | general | Architecture | Crate boundaries | 1 | None | Preserve |
| `docs/adrs/ADR-002-full-turn-accumulation-before-tool-execution.md` | ADR | accepted | v0.1 | gestalt-core | Execution | 1 | None | Preserve |
| `docs/adrs/ADR-003-toolexecutionresult-separate-from-tooloutput.md` | ADR | accepted | v0.1 | gestalt-core | Tools | 1 | None | Preserve |
| `docs/adrs/ADR-004-policyrequest-struct-over-nameinput-pair.md` | ADR | accepted | v0.1 | gestalt-core | Policy | 1 | None | Preserve |
| `docs/adrs/ADR-005-approvalprovider-as-an-injectable-interface.md` | ADR | accepted | v0.1 | gestalt-core | Approval | 1 | None | Preserve |
| `docs/adrs/ADR-006-three-replay-modes-display-deterministic-regression.md` | ADR | accepted | v0.1 | gestalt-runtime | Replay | 1 | None | Preserve |
| `docs/adrs/ADR-007-eventenvelope-in-gestalt-trace-not-gestalt-core.md` | ADR | superseded | historical | gestalt-runtime | Trace | 1 | ADR-031 and H0B event decision | Retain as decision history |
| `docs/adrs/ADR-008-minimal-policy-ships-in-v01.md` | ADR | accepted | v0.1 | gestalt-runtime | Policy | 1 | None | Preserve |
| `docs/adrs/ADR-009-sandbox-deferred-to-v02.md` | ADR | accepted | v0.1 | gestalt-runtime | Execution | 1 | None | Preserve |
| `docs/adrs/ADR-010-contenttrust-tags-on-all-external-content.md` | ADR | accepted | v0.1 | gestalt-core | Trust | 1 | None | Preserve |
| `docs/adrs/ADR-011-credential-resolution-boundary-separate-from-provider-behavior-config.md` | ADR | accepted | v0.1 | gestalt-runtime | Providers | 1 | None | Preserve |
| `docs/adrs/ADR-012-preserve-provider-finish-reasons-in-normalized-stopreason.md` | ADR | accepted | v0.1 | gestalt-core | Providers | 1 | None | Preserve |
| `docs/adrs/ADR-013-bounded-session-approval-grants-instead-of-tool-name-keys.md` | ADR | accepted | v0.1 | gestalt-core | Approval | 1 | None | Preserve |
| `docs/adrs/ADR-014-rich-contextpacket-and-contextbuilt-events.md` | ADR | accepted | v0.1 | gestalt-core | Context | 1 | None | Preserve |
| `docs/adrs/ADR-015-artifact-spillover-for-truncated-tool-output.md` | ADR | accepted | v0.1 | gestalt-runtime | Artifacts | 1 | None | Preserve |
| `docs/adrs/ADR-016-honest-host-execution-nosandbox-and-network-policy-enforcement.md` | ADR | accepted | v0.1 | gestalt-runtime | Execution | 1 | None | Preserve |
| `docs/adrs/ADR-017-dedicated-verification-substrate-gestalt-verify.md` | ADR | accepted | v0.1 | gestalt-runtime | Verification | 1 | None | Preserve |
| `docs/adrs/ADR-018-internal-lifecycle-hooks-for-extensibility.md` | ADR | accepted | v0.1 | gestalt-runtime | Hooks | 1 | None | Preserve |
| `docs/adrs/ADR-019-workspace-state-snapshotting-in-session-metadata.md` | ADR | accepted | v0.1 | gestalt-runtime | Replay | 1 | None | Preserve |
| `docs/adrs/ADR-020-trace-driven-regression-testing-via-golden-traces.md` | ADR | accepted | v0.1 | gestalt-runtime | Trace | 1 | None | Preserve |
| `docs/adrs/ADR-021-default-system-prompt-with-local-policy-overrides.md` | ADR | accepted | v0.1 | gestalt-runtime | Prompt | 1 | None | Preserve |
| `docs/adrs/ADR-022-persistent-session-lineage-resumability-and-graceful-cancel-safety.md` | ADR | accepted | v0.1 | gestalt-app | Sessions | 1 | None | Preserve |
| `docs/adrs/ADR-023-runtime-composition-layer.md` | ADR | accepted | v0.1 | gestalt-runtime | Composition | 1 | None | Preserve |
| `docs/adrs/ADR-024-process-backed-extensions-over-stdio-json-rpc.md` | ADR | superseded | historical | gestalt-runtime | Extensions | 1 | ADR-030 | Retain as redirect/history |
| `docs/adrs/ADR-024-process-extensions.md` | ADR | superseded | historical | gestalt-runtime | Extensions | 1 | ADR-028 and ADR-030 | Retain as decision history |
| `docs/adrs/ADR-025-unified-gestalt-json-config.md` | ADR | accepted | v0.1 | gestalt-app | Configuration | 1 | ADR-031 for removed compatibility clauses | Mark superseded clauses inline |
| `docs/adrs/ADR-026-cache-aware-prompt-assembly.md` | ADR | accepted | v0.1 | gestalt-runtime | Context | 1 | None | Preserve |
| `docs/adrs/ADR-027-mcp-client-integration.md` | ADR | accepted | v0.1 | gestalt-runtime | MCP | 1 | None | Preserve |
| `docs/adrs/ADR-028-extension-package-components.md` | ADR | accepted | v0.1 | gestalt-runtime | Extensions | 1 | None | Preserve |
| `docs/adrs/ADR-029-runtime-snapshot-reload.md` | ADR | accepted | v0.1 | gestalt-runtime | Generation | 1 | H0B-F06 clarification | Preserve |
| `docs/adrs/ADR-030-lifecycle-protocol-v2.md` | ADR | accepted | v0.1 | gestalt-runtime | Extensions | 1 | ADR-031 for V1 removal | Mark superseded clauses inline |
| `docs/adrs/ADR-031-v0-1-greenfield-compatibility-cutoff.md` | ADR | accepted | v0.1 | Release Mgmt | Compatibility | 1 | None | Preserve and enforce |
| `docs/audits/2026-06-29-001-runtime-feature-gating-implementation-audit.md` | Audit | historical | v0.1 | Release Mgmt | Feature gates | 7 | None | Preserve immutable evidence |
| `docs/audits/2026-07-02-001-v0.1-hardening.md` | Audit | active | v0.1 | Release Mgmt | v0.1 hardening | 7 | `docs/v0.1/README.md` | Record workstream requirements and execution evidence |
| `docs/migrations/extension-manifest-v1-to-v2.md` | Migration | historical | pre-v0.1 | gestalt-runtime | Extensions | 7 | ADR-031 | Retain only as unsupported pre-release history |
| `docs/solutions/2026-06-01-001-v0-1-harness-engineering-review.md` | Solution | historical | v0.1 | Release Mgmt | Engineering review | 7 | None | Preserve with enforcement links |
| `docs/v0.1/app-services.md` | Version Contract | published | v0.1 | gestalt-app | App services | 2 | None | Published and enforced by `report_contract_tests` |
| `docs/v0.1/cli-automation.md` | Version Contract | published | v0.1 | gestalt-cli | CLI | 2 | None | Published and test-backed |
| `docs/v0.1/configuration.md` | Version Contract | published | v0.1 | gestalt-app | Configuration | 2 | None | Published and test-backed |
| `docs/v0.1/conformance-matrix.md` | Conformance Matrix | published | v0.1 | Release Mgmt | Release evidence | 2 | None | Map every published contract ID to implementation and enforcing tests |
| `docs/v0.1/context-build-report.md` | Version Contract | published | v0.1 | gestalt-runtime | Context | 2 | None | Published and enforced by context report tests |
| `docs/v0.1/contract-inventory.md` | Inventory | active | v0.1 | Docs Group | Contract status | 2 | None | Keep deny-by-default classification aligned with the version index |
| `docs/v0.1/embedding-control.md` | Version Contract | published | v0.1 | gestalt-runtime | Runtime control | 2 | None | Published and test-backed |
| `docs/v0.1/extensions.md` | Version Contract | published | v0.1 | gestalt-runtime | Extensions | 2 | None | Published and enforced by extension V2 and activation tests |
| `docs/v0.1/migration.md` | Migration | active | v0.1 | Docs Group | v0.1 migration | 2 | None | Keep the greenfield compatibility cutoff concise and current |
| `docs/v0.1/policy-approval.md` | Version Contract | published | v0.1 | gestalt-runtime | Policy and approval | 2 | None | Published and test-backed |
| `docs/v0.1/runtime-api.md` | Version Contract | published | v0.1 | gestalt-runtime | Rust API boundary | 2 | None | Published and enforced by public API contract tests |
| `docs/v0.1/trace-events.md` | Version Contract | published | v0.1 | gestalt-runtime | Trace and client events | 2 | None | Published and enforced by trace contract tests |
| `docs/plans/2026-06-01-002-feat-v0-1-harness-engineering-primitives-plan.md` | Plan | completed | v0.1 | Project | Engineering primitives | 7 | Current hardening plans | Preserve as historical execution evidence |
| `docs/plans/2026-06-02-002-feat-workspace-commands-plan.md` | Plan | completed | v0.1 | gestalt-app | Workspace commands | 7 | Current app contracts | Preserve as historical execution evidence |
| `docs/plans/2026-06-22-001-feat-context-projection-hardening-plan.md` | Plan | completed | v0.1 | gestalt-runtime | Context | 7 | H2B | Preserve as historical execution evidence |
| `docs/plans/2026-06-23-001-ref-extension-foundation.md` | Plan | completed | v0.1 | gestalt-runtime | Extensions | 7 | H4A/H4B | Preserve as historical execution evidence |
| `docs/plans/2026-06-27-001-feat-model-schema-normalization.md` | Plan | completed | v0.1 | gestalt-runtime | Providers | 7 | H0B inventory | Preserve as historical execution evidence |
| `docs/plans/2026-06-28-001-fix-review-findings-plan.md` | Plan | completed | v0.1 | Project | Review findings | 7 | Current phase plans | Preserve as historical execution evidence |
| `docs/plans/2026-06-28-crate-boundary.md` | Plan | completed | v0.1 | Workspace | Crate boundaries | 7 | Architecture overview | Preserve as historical execution evidence |
| `docs/plans/2026-06-29-001-ref-runtime-feature-gating-and-legacy-removal.md` | Plan | completed | v0.1 | gestalt-runtime | Feature gates | 7 | H4A | Preserve as historical execution evidence |
| `docs/plans/v0.1-hardening/H1A-runtime-control-dtos-and-semantics.md` | Plan | completed | v0.1 | gestalt-runtime | Runtime control | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md` | Plan | completed | v0.1 | gestalt-app | Host boundary | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H2A-event-trace-replay-contracts.md` | Plan | completed | v0.1 | gestalt-runtime | Trace | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H2B-context-diagnostics-and-determinism.md` | Plan | completed | v0.1 | gestalt-runtime | Context | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H3A-config-schema-layering-and-cleanup.md` | Plan | completed | v0.1 | gestalt-app | Configuration | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H3B-cli-automation-contract-and-snapshots.md` | Plan | completed | v0.1 | gestalt-cli | CLI | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H4A-extension-v2-only-cleanup.md` | Plan | completed | v0.1 | gestalt-runtime | Extension cleanup | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H4B-extension-activation-trust-and-generation.md` | Plan | completed | v0.1 | gestalt-runtime | Extension activation | 5 | None | Preserve completion evidence |
| `docs/plans/v0.1-hardening/H5-release-conformance-docs-freeze-and-cutover.md` | Plan | active | v0.1 | Release Mgmt | Release | 5 | None | Run after all domain gates |
| `docs/plans/v0.1-hardening/documentation-inventory.md` | Inventory | active | v0.1 | Docs Group | Documentation | 5 | None | Enforced by `scripts/check-hardening-docs.sh` |
