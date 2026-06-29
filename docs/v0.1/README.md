# Gestalt v0.1 Stable Versioned Contracts

This index maps the stable v0.1 contracts across Gestalt's domain boundaries. No contract is considered published until its corresponding implementation passes all conformance tests.

---

## 1. Embedding and Runtime Control (RUNTIME-001–RUNTIME-006)
* **Status:** 🟢 **Published**
* **Specification:** [embedding-control.md](./embedding-control.md)
* **Implementation Plan:** [H1A](../plans/v0.1-hardening/H1A-runtime-control-dtos-and-semantics.md), [H1B](../plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md)
* **Rust API Target:** `gestalt_runtime::control::contract::RuntimeControlV1`
* **Enforcing Tests:** [control_conformance.rs](../../crates/gestalt-runtime/tests/control_conformance.rs)

## 2. App Services (APP-001–APP-002)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/v0.1-hardening.md](../feature-spec/v0.1-hardening.md#99-app-001-keep-gestalt-app-product-neutral)
* **Implementation Plan:** [H1B](../plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md)
* **Rust API Target:** `gestalt_app::workspace::{init_workspace, info_workspace}`
* **Enforcing Tests:** `crates/gestalt-app/tests/report_contract_tests.rs` (pending)

## 3. Context Diagnostics (CTX-001–CTX-004)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/context-projection-hardening.md](../feature-spec/context-projection-hardening.md)
* **Implementation Plan:** [H2B](./H2B-context-diagnostics-and-determinism.md)
* **Rust API Target:** `gestalt_core::context::ContextPacket`
* **Enforcing Tests:** `crates/gestalt-core/tests/context_tests.rs` (pending)

## 4. Trace & Run Manifests (EVT-001–EVT-005)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/v0.1-hardening.md](../feature-spec/v0.1-hardening.md#34-events-remain-the-ground-truth)
* **Implementation Plan:** [H2A](./H2A-event-trace-replay-contracts.md)
* **Rust API Target:** `gestalt_runtime::EventEnvelope`
* **Enforcing Tests:** `crates/gestalt-app/tests/sessions_tests.rs` (pending)

## 5. Configuration (CFG-001–CFG-005)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/config-extension.md](../feature-spec/config-extension.md)
* **Implementation Plan:** [H3A](./H3A-config-schema-layering-and-cleanup.md)
* **JSON Schema:** `docs/schemas/gestalt.schema.json`
* **Enforcing Tests:** `crates/gestalt-app/tests/config_tests.rs` (pending)

## 6. Policy & Approval (POL-001–POL-004)
* **Status:** 🟢 **Published**
* **Specification:** [policy-approval.md](./policy-approval.md)
* **Implementation Plan:** [H1A](./H1A-runtime-control-dtos-and-semantics.md)
* **Rust API Target:** `gestalt_runtime::control::contract::ApprovalControlV1`
* **Enforcing Tests:** [control_conformance.rs](../../crates/gestalt-runtime/tests/control_conformance.rs)

## 7. Extensions V2 Compatibility (EXT-001–EXT-005)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/product-neutral-extension-architecture.md](../feature-spec/product-neutral-extension-architecture.md)
* **Implementation Plan:** [H4A](./H4A-extension-v2-only-cleanup.md), [H4B](./H4B-extension-activation-trust-and-generation.md)
* **JSON Schema:** `docs/schemas/extension-manifest.schema.json`
* **Enforcing Tests:** `crates/gestalt-runtime/tests/extension_tests.rs` (pending)

## 8. Stable CLI Automation (CLI-001–CLI-004)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/v0.1-hardening.md](../feature-spec/v0.1-hardening.md#7-contract-and-stability-model)
* **Implementation Plan:** [H3B](./H3B-cli-automation-contract-and-snapshots.md)
* **CLI Command Entry Points:** `gestalt run --json`, `gestalt session inspect --json`
* **Enforcing Tests:** `crates/gestalt-cli/tests/cli_json_tests.rs` (pending)
