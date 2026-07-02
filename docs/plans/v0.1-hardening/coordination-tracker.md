# v0.1 Hardening Program Coordination Tracker

This tracker monitors the status, requirement mapping, gate results, and shared fixtures for the contract-hardening program. It is the authoritative record for the exit criteria of [000-program-coordination-and-phase-gates.md](./000-program-coordination-and-phase-gates.md).

---

## 1. Requirement Ownership Map (PC-F01)

| Requirement ID | Description | Primary Owner | Supporting Plans |
|---|---|---|---|
| **DOC-001–008** | Documentation Index, Contract Map, PRD/Arch Simplification, Crate READMEs | [H0A](./H0A-documentation-authority-and-contract-map.md) | 000 |
| **CLEAN-001–007** | Greenfield Cutoff, Legacy Config, API, Manifest, and Persistence Removal | [H0C](./H0C-greenfield-removal-ledger-and-absence-checks.md) | H3A, H4A, H5 |
| **CORE-001–002** | Core execution invariants, canonical identity, lineage | [H1A](./H1A-runtime-control-dtos-and-semantics.md) | H2B |
| **CORE-003–004** | Crate trait classification, failure/panic contracts | [H0B](./H0B-api-spi-inventory-and-adr-decisions.md) | H1A, H4B |
| **RUNTIME-001–005** | DTO/Model split, Session/Message, Subscription, Artifact access, Errors | [H1A](./H1A-runtime-control-dtos-and-semantics.md) | H0B, H2B |
| **RUNTIME-006** | Host boundary implementation | [H1B](./H1B-runtime-host-app-boundary-and-conformance.md) | H1A |
| **APP-001–002** | gestalt-app neutrality, structured diagnostics | [H1B](./H1B-runtime-host-app-boundary-and-conformance.md) | H0B |
| **CTX-001–004** | Context projections, diagnostic DTOs, captured determinism, compression | [H2B](./H2B-context-diagnostics-and-determinism.md) | H0B, H2A |
| **CFG-001–005** | gestalt.json schema validation, global/workspace layering, prompt overrides | [H3A](./H3A-config-schema-layering-and-cleanup.md) | H0B, H0C |
| **CLI-001–004** | Stable command subset, success/error envelope, automated script checks | [H3B](./H3B-cli-automation-contract-and-snapshots.md) | H0B |
| **EXT-001–005** | Extension V2 boundaries, validation, launch, generation, token/network lease | [H4B](./H4B-extension-activation-trust-and-generation.md) | H0B, H4A |
| **EVT-001–005** | Lossless traces, schema ownership, typed client projections, replay contracts | [H2A](./H2A-event-trace-replay-contracts.md) | H0B |
| **POL-001–004** | Policy Engine, action matrix, session-approval, tool sandbox boundary | [H1A](./H1A-runtime-control-dtos-and-semantics.md) | H0B, H4B |
| **PROV-001–002** | Normalized stream, credential resolver | [H0B](./H0B-api-spi-inventory-and-adr-decisions.md) | H1A |
| **TOOL-001–003** | Tool input schema serialization, risk-classification, cancellation | [H0B](./H0B-api-spi-inventory-and-adr-decisions.md) | H1A |
| **Sections 18, 21, 22** | Conformance check freeze, cutover execution | [H5](./H5-release-conformance-docs-freeze-and-cutover.md) | All |

---

## 2. Plan Dependency & Status Record (PC-F02)

| Plan ID | Title | Status | Predecessors | Evidence Link |
|---|---|---|---|---|
| **000** | Coordination | `complete` | None | [000-program-coordination-and-phase-gates.md](./000-program-coordination-and-phase-gates.md) |
| **H0A** | Documentation Map | `complete` | 000 | [H0A-documentation-authority-and-contract-map.md](./H0A-documentation-authority-and-contract-map.md) |
| **H0B** | API/SPI Inventory & ADR Decisions | `complete` | 000 | [H0B-api-spi-inventory-and-adr-decisions.md](./H0B-api-spi-inventory-and-adr-decisions.md) |
| **H0C** | Removal Ledger & Rejection Tests | `complete` | 000 | [H0C-greenfield-removal-ledger-and-absence-checks.md](./H0C-greenfield-removal-ledger-and-absence-checks.md) |
| **H1A** | Runtime Control DTOs | `complete` | H0B, H0C | [H1A-runtime-control-dtos-and-semantics.md](./H1A-runtime-control-dtos-and-semantics.md) |
| **H1B** | Host Boundary & Conformance | `complete` | H1A | [control_conformance.rs](../../../crates/gestalt-runtime/tests/control_conformance.rs) |
| **H2A** | Event Trace & Replay | `in_progress` | H0B, H0C | [trace_contract_tests.rs](../../../crates/gestalt-runtime/tests/trace_contract_tests.rs); canonical record/client projection split and full EVT-005 matrix still pending |
| **H2B** | Context Diagnostics | `blocked` | H0B, H2A | Blocked on H2A canonical event/version split and replay fixture matrix |
| **H3A** | Config Layering & Schema | `complete` | H0C | [configuration.md](../../v0.1/configuration.md) |
| **H3B** | CLI Automation & Snapshots | `complete` | H0B, H3A | H0B-F05 inventory accepted; machine-readable usage/error envelopes verified; `h3b_snapshot_tests` covers the stable command matrix |
| **H4A** | Extension V2 Cleanup | `in_progress` | H0C | V2 cleanup is implemented, but the phase still needs matrix/absence verification and release-scope confirmation; see [H4A](./H4A-extension-v2-only-cleanup.md) |
| **H4B** | Activation, Trust & Generation | `in_progress` | H0B, H4A | Hash-pinned trust and typed host failures are in place, but activation/reporting/generation criteria are still being verified; see [H4B](./H4B-extension-activation-trust-and-generation.md) |
| **H5** | Release Conformance Freeze | `blocked` | All prior | *Blocked on Step 5* |

---

## 3. Phase Gate Logs (PC-F03)

### Gate G0: Scope and Ownership Freeze
* **Status:** `PASSED`
* **Date:** 2026-06-29
* **Reviewer:** Antigravity AI
* **Evidence:**
  - Every spec requirement mapped to a phase primary owner.
  - Standalone ADRs extracted from architecture monolith: [ADRs](../../adrs/) (ADR-001 to ADR-022 standalone files created).
  - Removal ledger created: [pre-hardening-removal-ledger.md](./pre-hardening-removal-ledger.md).
  - API/SPI inventory created: [api-spi-inventory.md](./api-spi-inventory.md).
  - Architectural decisions documented: [h0b-architectural-decisions.md](./h0b-architectural-decisions.md).
* **Failures:** None.
* **Affected Downstream Plans:** Downstream Step 3 plans unblocked for G1 preparation.

### Gate G1: Contract Test Compliance
* **Status:** `NOT STARTED`
* **Target Date:** Step 3 Completion
* **Evidence Required:** Unit/integration tests asserting exact contract schema compliance.

### Gate G2: Fixture Reconciliation
* **Status:** `NOT STARTED`
* **Target Date:** Step 4 Completion
* **Evidence Required:** Conformance testing verifying that local mock fixtures and shared fixtures do not diverge.

### Gate G3: Release Conformance
* **Status:** `NOT STARTED`
* **Target Date:** Step 5 Completion
* **Evidence Required:** Final absence ledger run showing zero matches for pre-hardening legacy config, manifest V1, and deprecated API references.

---

## 4. Shared Fixture Registry (PC-F04)

All reusable test fixtures under `/tests/fixtures/` must be registered here.

| Fixture Path | Owning Plan | Schema / Version | Normalization Rules | Consuming Suites |
|---|---|---|---|---|
| `tests/fixtures/config/v1/full_valid.json` | H3A | `gestalt.json` v1 | None (static fields) | `config_tests.rs`, CLI smoke |
| `tests/fixtures/extension/v2/manifest.json` | H4B | Extension manifest v2 | None (static fields) | `extension_tests.rs` |
| `tests/fixtures/traces/golden_run.jsonl` | H2A | Trace envelope v1 | Normalize timestamps to `1970-01-01T00:00:00Z` and IDs | `golden_trace_tests.rs` |

---

## 5. Documentation Convention (PC-F05)

Every active repository document must follow these rules:

1. **Metadata block (YAML frontmatter)**:
   ```yaml
   ---
   title: "Short descriptive title"
   status: "proposed | active | accepted | superseded | completed"
   type: "adr | feature-spec | version-contract | plan | guide | reference"
   target: "v0.1 | v0.2 | general"
   owners:
     - "crate-or-domain-owner"
   ---
   ```
2. **Authority hierarchy**: Defined in the root-level [docs/README.md](../../README.md). No plan can override specifications or ADRs.
3. **Publication**: Version-contract documentation goes into `docs/v0.1/` only after implementation and tests pass.
4. **Link Integrity**: Use repository-relative links. If moving a file, leave a redirect snippet at the old path.
