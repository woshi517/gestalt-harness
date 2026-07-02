# Gestalt v0.1 Stable Versioned Contracts

This index maps the stable v0.1 contracts across Gestalt's domain boundaries. No contract is considered published until its corresponding implementation passes all conformance tests.

The complete current classification is in the
[contract inventory](./contract-inventory.md). The deliberate Rust boundary is
documented in [runtime-api.md](./runtime-api.md).

---

## 1. Embedding and Runtime Control (RUNTIME-001–RUNTIME-006)
* **Status:** 🔴 **Unpublished (Remaining Event/Conformance Gates)**
* **Specification:** [embedding-control.md](./embedding-control.md)
* **Implementation Plan:** [H1A](../plans/v0.1-hardening/H1A-runtime-control-dtos-and-semantics.md), [H1B](../plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md)
* **Rust API Target:** `gestalt_runtime::api::v1::{RuntimeBackedControlHost, RuntimeControlV1}`
* **Enforcing Tests:** [control_conformance.rs](../../crates/gestalt-runtime/tests/control_conformance.rs), [runtime_control_real_run.rs](../../crates/gestalt-runtime/tests/runtime_control_real_run.rs)
* **Test Support:** `InMemoryControlHost` and `MockControlHost` are conformance-only implementations.

## 2. App Services (APP-001–APP-002)
* **Status:** 🔴 **Unpublished (Diagnostics Integration Required)**
* **Proposed Specification:** [feature-spec/v0.1-hardening.md](../feature-spec/v0.1-hardening.md#99-app-001-keep-gestalt-app-product-neutral)
* **Implementation Plan:** [H1B](../plans/v0.1-hardening/H1B-runtime-host-app-boundary-and-conformance.md)
* **Rust API:** `gestalt_app::reports::{ServiceReportV1, AppDiagnosticV1, AppErrorProjectionV1}` and `gestalt_app::runtime_factory::build_app_runtime_with_report`
* **Enforcing Tests:** [report_contract_tests.rs](../../crates/gestalt-app/tests/report_contract_tests.rs)

## 3. Context Diagnostics (CTX-001–CTX-004)
* **Status:** 🟢 **Published**
* **Contract:** [context-build-report.md](./context-build-report.md)
* **Rust API:** `gestalt_runtime::api::v1::{ContextBuildReportV1, ContextCaptureMode}`
* **Enforcing Tests:** [context_report_contract_tests.rs](../../crates/gestalt-runtime/tests/context_report_contract_tests.rs)

## 4. Configuration (CFG-001–CFG-005)
* **Status:** 🟢 **Published**
* **Specification:** [configuration.md](./configuration.md)
* **Implementation Plan:** [H3A](../plans/v0.1-hardening/H3A-config-schema-layering-and-cleanup.md)
* **JSON Schema:** [gestalt.schema.json](../schemas/gestalt.schema.json)
* **Enforcing Tests:** [config_schema_tests.rs](../../crates/gestalt-app/tests/config_schema_tests.rs), [config_tests.rs](../../crates/gestalt-app/tests/config_tests.rs)

## 5. Trace & Run Manifests (EVT-001–EVT-005)
* **Status:** 🟢 **Published**
* **Specification:** [trace-events.md](./trace-events.md)
* **Client API:** `gestalt_runtime::api::v1::{ClientEventPayloadV1, ClientEventRecordV1, project_client_event_line}`
* **Internal Trace API:** `gestalt_runtime::unstable::{EventEnvelope, TraceEventV1}`
* **Enforcing Tests:** [trace_contract_tests.rs](../../crates/gestalt-runtime/tests/trace_contract_tests.rs), [sessions_tests.rs](../../crates/gestalt-app/tests/sessions_tests.rs)

## 6. Policy & Approval (POL-001–POL-004)
* **Status:** 🟢 **Published**
* **Specification:** [policy-approval.md](./policy-approval.md)
* **Implementation Plan:** [H1A](../plans/v0.1-hardening/H1A-runtime-control-dtos-and-semantics.md)
* **Rust API:** `gestalt_runtime::api::v1::{ApprovalControlV1, PolicyProjectionV1, ApprovalProjectionV1}`
* **Enforcing Tests:** [control_conformance.rs](../../crates/gestalt-runtime/tests/control_conformance.rs), [runtime_control_real_run.rs](../../crates/gestalt-runtime/tests/runtime_control_real_run.rs), and core session-grant tests

## 7. Extension Packages & Components (V2-only) (EXT-001–EXT-005)
* **Status:** 🟢 **Published**
* **Specification:** [extensions.md](./extensions.md)
* **Implementation Plan:** [H4A](../plans/v0.1-hardening/H4A-extension-v2-only-cleanup.md), [H4B](../plans/v0.1-hardening/H4B-extension-activation-trust-and-generation.md)
* **Enforcing Tests:** `extension_manifest_v2_tests`, `extension_instance_config_tests`, `runtime_permissions_tests`, `extension_manager_tests`, `extension_reload_tests`, and `runtime_cli_tests`

## 8. Stable CLI Automation (CLI-001–CLI-004)
* **Status:** 🟢 **Published**
* **Specification:** [cli-automation.md](./cli-automation.md)
* **Implementation Plan:** [H3B](../plans/v0.1-hardening/H3B-cli-automation-contract-and-snapshots.md)
* **CLI Command Entry Points:** See the stable command matrix.
* **Enforcing Tests:** [cli_automation_contract_tests.rs](../../crates/gestalt-cli/tests/cli_automation_contract_tests.rs), [main_cli_contract_tests.rs](../../crates/gestalt-cli/tests/main_cli_contract_tests.rs), [h3b_snapshot_tests.rs](../../crates/gestalt-cli/tests/h3b_snapshot_tests.rs)
