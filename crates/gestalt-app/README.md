# Gestalt App Crate (`gestalt-app`)

The `gestalt-app` crate owns reusable product-level services built on top of `gestalt-runtime`. It provides config resolution, workspace models, model and provider services, credential verification, and run/session persistence.

---

## Ownership Boundary

- **What it owns:** Config validation, workspace initialization, credentials parsing, run persistence databases/stores, and report/diagnostic serialization.
- **What it does NOT own:** UI rendering (held by `gestalt-cli` and `gestalt-tui`), core LLM state loop (`gestalt-core`), or execution permissions evaluation (`gestalt-runtime`).

---

## Core Entry Points

- `WorkspaceResolver` / `WorkspaceContextLoader` — Resolves and loads the current workspace configuration.
- `reports::RunReport` — Serializes runs and cost estimates into human-readable data.

---

## Construction Example

Construct and load a workspace configuration:

```rust
use std::path::PathBuf;
use gestalt_app::workspace::{WorkspaceContextConfig, WorkspaceContextLoader};

let config = WorkspaceContextConfig {
    workspace_root: PathBuf::from("./my-workspace"),
    selected_profile: Some("default".to_string()),
};

let workspace = WorkspaceContextLoader::new()
    .load(&config)
    .expect("Failed to load workspace context");
```

---

## Error Handling & Cancellation

- Expected validation errors return `HarnessError::Config` or `HarnessError::Workspace`.
- Cancellation is propagated via the standard `CancelToken` to abort network preflight tests and credential verification checks.

---

## Feature Gates

- `providers`: Enables model caching and provider connections.
- `trace`: Enables run history queries and reports.
- `verify`: Enables task verifiers.
