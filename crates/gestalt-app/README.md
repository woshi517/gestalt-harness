# Gestalt App Crate (`gestalt-app`)

The `gestalt-app` crate owns reusable product-level services built on top of `gestalt-runtime`. It provides config resolution, workspace models, model and provider services, credential verification, and run/session persistence.

---

## Ownership Boundary

- **What it owns:** Config validation, workspace initialization, credentials parsing, run persistence databases/stores, and report/diagnostic serialization.
- **What it does NOT own:** UI rendering (held by `gestalt-cli` and `gestalt-tui`), core LLM state loop (`gestalt-core`), or execution permissions evaluation (`gestalt-runtime`).

---

## Core Entry Points

- `workspace::init_workspace` / `workspace::info_workspace` — Resolve, initialize, and inspect the current workspace configuration.
- `reports::ServiceReportV1` / `AppDiagnosticV1` — Return typed values,
  ordered diagnostics, and stable error projections without presentation I/O.
- `runtime_factory::build_app_runtime_with_report` — Constructs a runtime and
  returns expected failures and warnings as structured data.

---

## Construction Example

Construct and initialize a workspace:

```rust
use std::path::Path;
use gestalt_app::workspace::init_workspace;

let report = init_workspace(Path::new("./my-workspace"), false)
    .expect("Failed to initialize workspace");
```

---

## Error Handling & Cancellation

- Reusable service boundaries return `ServiceReportV1`; expected construction
  failures are projected as `AppErrorProjectionV1`.
- Cancellation is propagated via the standard `CancelToken` to abort network preflight tests and credential verification checks.

---

## Feature Gates

- `providers`: Enables model caching and provider connections.
- `trace`: Enables run history queries and reports.
- `verify`: Enables task verifiers.
