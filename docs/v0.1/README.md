# Gestalt v0.1 Stable Versioned Contracts

This index maps the stable v0.1 contracts across Gestalt's domain boundaries. No contract is considered published until its corresponding implementation passes all conformance tests.

---

## 1. Embedding and Runtime Control (RUNTIME-001–RUNTIME-006)
* **Status:** 🟢 **Published**
* **Specification:** [embedding-control.md](./embedding-control.md)
* **Implementation Plan:** [H1A](../plans/v0.1-hardening/H1A-runtime-control-dtos-and-semantics.md), [H1B](../plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md)
* **Rust API:** `gestalt_runtime::control::{LocalControlHost, MockControlHost}` and `gestalt_runtime::control::contract::RuntimeControlV1`
* **Enforcing Tests:** [control_conformance.rs](../../crates/gestalt-runtime/tests/control_conformance.rs)

## 2. App Services (APP-001–APP-002)
* **Status:** 🟢 **Published**
* **Proposed Specification:** [feature-spec/v0.1-hardening.md](../feature-spec/v0.1-hardening.md#99-app-001-keep-gestalt-app-product-neutral)
* **Implementation Plan:** [H1B](../plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md)
* **Rust API:** `gestalt_app::reports::{ServiceReportV1, AppDiagnosticV1, AppErrorProjectionV1}` and `gestalt_app::runtime_factory::build_app_runtime_with_report`
* **Enforcing Tests:** [report_contract_tests.rs](../../crates/gestalt-app/tests/report_contract_tests.rs)

## 3. Context Diagnostics (CTX-001–CTX-004)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Contract:** [context-build-report.md](./context-build-report.md)
* **Implementation Plan:** [H2B](../plans/v0.1-hardening/H2B-context-diagnostics-and-determinism.md)
* **Rust API Target:** `gestalt_runtime::ContextBuildReportV1`
* **Enforcing Tests:** [context_report_contract_tests.rs](../../crates/gestalt-runtime/tests/context_report_contract_tests.rs)

## 4. Configuration (CFG-001–CFG-005)
* **Status:** 🟢 **Published**
* **Specification:** [configuration.md](./configuration.md)
* **Implementation Plan:** [H3A](../plans/v0.1-hardening/H3A-config-schema-layering-and-cleanup.md)
* **JSON Schema:** [gestalt.schema.json](../schemas/gestalt.schema.json)
* **Enforcing Tests:** [config_schema_tests.rs](../../crates/gestalt-app/tests/config_schema_tests.rs), [config_tests.rs](../../crates/gestalt-app/tests/config_tests.rs)

## 5. Trace & Run Manifests (EVT-001–EVT-005)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/v0.1-hardening.md](../feature-spec/v0.1-hardening.md#34-events-remain-the-ground-truth)
* **Implementation Plan:** [H2A](../plans/v0.1-hardening/H2A-event-trace-replay-contracts.md)
* **Rust API Target:** `gestalt_runtime::{EventEnvelope, ClientEventRecordV1}`
* **Enforcing Tests:** [trace_contract_tests.rs](../../crates/gestalt-runtime/tests/trace_contract_tests.rs), [sessions_tests.rs](../../crates/gestalt-app/tests/sessions_tests.rs)

## 6. Policy & Approval (POL-001–POL-004)
* **Status:** 🟢 **Published**
* **Specification:** [policy-approval.md](./policy-approval.md)
* **Implementation Plan:** [H1A](../plans/v0.1-hardening/H1A-runtime-control-dtos-and-semantics.md)
* **Rust API Target:** `gestalt_runtime::control::contract::ApprovalControlV1`
* **Enforcing Tests:** [control_conformance.rs](../../crates/gestalt-runtime/tests/control_conformance.rs)

## 7. Extension Packages & Components (V2-only) (EXT-001–EXT-005)
* **Status:** 🔴 **Unpublished (Under Development)**
* **Proposed Specification:** [feature-spec/product-neutral-extension-architecture.md](../feature-spec/product-neutral-extension-architecture.md)
* **Implementation Plan:** [H4A](../plans/v0.1-hardening/H4A-extension-v2-only-cleanup.md), [H4B](../plans/v0.1-hardening/H4B-extension-activation-trust-and-generation.md)
* **Enforcing Tests:** `crates/gestalt-runtime/tests/extension_manifest_v2_tests.rs`, `crates/gestalt-runtime/tests/lifecycle_protocol_v2_tests.rs`, `crates/gestalt-runtime/tests/runtime_builder_tests.rs`

## 8. Stable CLI Automation (CLI-001–CLI-004)
* **Status:** 🟢 **Published**
* **Specification:** [cli-automation.md](./cli-automation.md)
* **Implementation Plan:** [H3B](../plans/v0.1-hardening/H3B-cli-automation-contract-and-snapshots.md)
* **CLI Command Entry Points:** See the stable command matrix.
* **Enforcing Tests:** [main_cli_contract_tests.rs](../../crates/gestalt-cli/tests/main_cli_contract_tests.rs), [h3b_snapshot_tests.rs](../../crates/gestalt-cli/tests/h3b_snapshot_tests.rs)
