---
title: "feat: Add workspace setup, status, and diagnostics commands"
type: feat
status: completed
date: 2026-06-02
origin: docs/plans/2026-06-02-001-feat-cli-tui-steering-wheel-plan.md
related:
  - docs/plans/2026-06-02-001-feat-cli-tui-steering-wheel-plan.md
---

# feat: Add workspace setup, status, and diagnostics commands

## Summary

Implement the workspace management and diagnostic command taxonomy (`init`, `status`, `workspace info`, `workspace snapshot`, and `workspace doctor`) in `gestalt-cli`. These commands enable operators to initialize a workspace, inspect configuration health, generate workspace snapshots, and diagnose provider credential status in either human-readable text or structured JSON envelopes.

---

## Problem Frame

In v0.1, managing the agent harness workspace is a manual task. Setting up a new workspace requires hand-crafting configuration files (`config.toml`, `policies.toml`) and documentation templates (`workspace.md`, `memory.md`). Inspecting the current workspace health, counting runs, and validating active defaults requires traversing the filesystem and checking environment variables manually. This lacks a structured control surface and is error-prone. Providing explicit setup, status, and diagnostic commands eliminates manual file management and improves workspace operability.

---

## Requirements

- **R1.** Scaffold a new workspace using conservative defaults with the `gestalt init` command.
- **R2.** Atomic execution for `gestalt init`: if any target files (`.gestalt/config.toml`, `policies.toml`, `workspace.md`, `memory.md`) exist, the command must fail without writing partial files, unless `--force` is supplied.
- **R3.** Implement `gestalt status` to provide a top-level summary of active settings, config health, recent runs, and credential warnings.
- **R4.** Implement `gestalt workspace` subcommands:
  - `info`: List paths to workspace configuration and data files.
  - `snapshot`: Capture and print the git and content metadata using `GitWorkspaceSnapshotter`.
  - `doctor`: Validate TOML formatting, verify required files exist, check provider auth environment variables, and verify directory writability.
- **R5.** Support versioned JSON envelopes (`schema_version: 1`, `kind: "workspace.<command>"`) when `--format json` is provided, returning text output as default.
- **R6.** Ensure commands do not require active network credentials for static validation or mock testing.

---

## Scope Boundaries

- Do not implement real sandbox environments or MCP registration checks during `workspace doctor`.
- Do not implement config mutation/editing commands (e.g. `config edit` is deferred).
- Do not perform live network model/provider reachability calls during standard `workspace doctor` or `status` validation (reachability tests belong to `providers doctor --live`).

### Deferred to Follow-Up Work

- Live reachability checks to provider API endpoints (handled separately under provider-specific commands with opt-in flags).
- Interactive wizard setup inside `gestalt init` to prompt for credentials or provider choices.

---

## Context & Research

### Relevant Code and Patterns

- Clap Parser & Subcommand Dispatch: [crates/gestalt-cli/src/main.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-cli/src/main.rs#L45-L54)
- Config precedence, validation, and directories: [crates/gestalt-cli/src/config.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-cli/src/config.rs#L165-L224)
- CLI Output report formatting and `CliReport` trait: [crates/gestalt-cli/src/output.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-cli/src/output.rs#L199-L203)
- `GitWorkspaceSnapshotter` implementation: [crates/gestalt-core/src/snapshot.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-core/src/snapshot.rs#L23-L81)
- Policy file loading and validation: [crates/gestalt-policy/src/lib.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-policy/src/lib.rs#L38-L47)
- Auth resolution logic: [crates/gestalt-cli/src/auth.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-cli/src/auth.rs#L6-L44)

---

## Key Technical Decisions

- **Atomicity via Pre-flight checks:** Before `init` writes any workspace files, it will scan for the presence of any of the four target files. If any exists, it will abort with a single, clear error listing the existing files, preventing partial or corrupt initializations.
- **Fast Run Listing:** Run directory count is computed by checking for direct subdirectories inside `.gestalt/runs` that contain a `trace.jsonl` file. This avoids slow operations like reading or parsing the logs themselves.
- **Diagnostics Isolation:** `workspace doctor` will trap configuration loading errors and parse failures, returning them as validation findings in the output report rather than panicking or exiting before analysis is complete.

---

## Open Questions

### Resolved During Planning

- **Template Storage:** Target workspace templates will be stored as static strings compiled inside the CLI binary to keep initialization completely local and self-contained.
- **Run Counting:** Runs will be scanned directly from the filesystem directory tree, verifying the presence of `trace.jsonl` to differentiate valid runs from empty folders.

### Deferred to Implementation

- None.

---

## Implementation Units

### U1. Define Output Reports for Workspace Commands

**Goal:** Create structured data payloads (serializable structs) and text rendering methods for all workspace-related subcommands in the CLI.

**Requirements:** R5

**Dependencies:** None

**Files:**
- Modify: `crates/gestalt-cli/src/output.rs`
- Create: `crates/gestalt-cli/tests/workspace_contract_tests.rs`

**Approach:**
- Add `WorkspaceInitReport`, `WorkspaceStatusReport`, `WorkspaceInfoReport`, `WorkspaceSnapshotReport`, and `WorkspaceDoctorReport` structs to `crates/gestalt-cli/src/output.rs`.
- Implement the `CliReport` trait for each report struct.
- In `kind()`, return `workspace.init`, `workspace.status`, `workspace.info`, `workspace.snapshot`, and `workspace.doctor`.
- Implement human-friendly text formatting in `render_text()` for each struct.

**Patterns to follow:**
- `ProvidersListReport` and `ConfigValidateReport` in `crates/gestalt-cli/src/output.rs`.

**Test scenarios:**
- Happy path: `WorkspaceInitReport` outputs clean list of created files under text mode, and valid JSON structure containing absolute workspace path under JSON mode.
- Happy path: `WorkspaceStatusReport` serializes active configuration details and auth statuses into the versioned envelope.
- Happy path: `WorkspaceSnapshotReport` encapsulates the full `WorkspaceSnapshot` struct correctly.

**Verification:**
- `cargo test -p gestalt-harness --test workspace_contract_tests` passes.

---

### U2. Implement gestalt init Command

**Goal:** Provide a command to safely scaffold `.gestalt/config.toml`, `policies.toml`, `workspace.md`, and `memory.md` with default contents.

**Requirements:** R1, R2, R5

**Dependencies:** U1

**Files:**
- Create: `crates/gestalt-cli/src/workspace.rs`
- Modify: `crates/gestalt-cli/src/lib.rs`
- Test: `crates/gestalt-cli/tests/workspace_cli_tests.rs`

**Approach:**
- Export `workspace` module in `crates/gestalt-cli/src/lib.rs`.
- In `workspace.rs`, implement `pub fn init_workspace(root: &Path, force: bool) -> Result<WorkspaceInitReport, HarnessError>`.
- Define default string templates for `config.toml`, `policies.toml`, `workspace.md`, and `memory.md`.
- Check if any target files already exist in `.gestalt/`. If any do and `force` is false, return an error listing existing files.
- Create `.gestalt/` directory if missing, write all files, and return the report.

**Patterns to follow:**
- Config path generation in `crates/gestalt-cli/src/config.rs`.

**Test scenarios:**
- Happy path: Running `init` on a fresh temp dir writes all files successfully.
- Edge case: Running `init` when `.gestalt/config.toml` already exists fails and writes no other files (e.g. `policies.toml` is not created).
- Edge case: Running `init --force` overwrites all existing config files successfully.

**Verification:**
- Temporary workspace initialized by `init` is readable and passes `load_effective_config` checks.

---

### U3. Implement gestalt status Command

**Goal:** Report workspace details, active configuration default options, runs stats, and authentication warnings.

**Requirements:** R3, R5, R6

**Dependencies:** U1, U2

**Files:**
- Modify: `crates/gestalt-cli/src/workspace.rs`
- Modify: `crates/gestalt-cli/src/config.rs`
- Test: `crates/gestalt-cli/tests/workspace_cli_tests.rs`

**Approach:**
- In `workspace.rs`, implement `pub fn status_workspace(overrides: &CliOverrides) -> Result<WorkspaceStatusReport, HarnessError>`.
- Attempt to load config via `load_effective_config(overrides)`.
- If successful, extract active defaults (provider, model, mode, max turns) and scan `config.run_log_dir()` (counting entries with `trace.jsonl`).
- Check auth variable presence for registered providers (e.g., anthropic, openai) using `resolve_auth`.
- If config load fails, capture the error, set `config_valid` to false, and populate warnings without crashing the execution.

**Patterns to follow:**
- Auth resolution logic in `crates/gestalt-cli/src/auth.rs`.
- Run directory creation in `crates/gestalt-trace/src/lib.rs`.

**Test scenarios:**
- Happy path: `status` outputs config validation success, default provider/model details, active mode, run count, and auth env var statuses.
- Edge case: `status` run on a directory without a `.gestalt/` folder reports config is invalid but completes execution with warnings.
- Edge case: Malformed TOML syntax in `config.toml` is captured and reported in warnings cleanly.

**Verification:**
- Status outputs correctly format workspace root and active options.

---

### U4. Implement gestalt workspace commands (info, snapshot, doctor)

**Goal:** Add deeper workspace information, metadata snapshotting, and validation checks.

**Requirements:** R4, R5, R6

**Dependencies:** U1, U2, U3

**Files:**
- Modify: `crates/gestalt-cli/src/workspace.rs`
- Test: `crates/gestalt-cli/tests/workspace_cli_tests.rs`

**Approach:**
- In `workspace.rs`, implement:
  - `info_workspace`: Returns absolute file paths of the active workspace files.
  - `snapshot_workspace`: Asynchronously runs `GitWorkspaceSnapshotter::capture(&config.workspace_root)` and returns the snapshot structure.
  - `doctor_workspace`: Evaluates config load, policies load (via `PolicyConfig::from_file`), checks file presence, checks auth statuses, and tests folder writability by writing and deleting a temporary test file.

**Patterns to follow:**
- `GitWorkspaceSnapshotter::capture` call in `crates/gestalt-cli/src/run.rs`.
- `PolicyConfig::from_file` call in `crates/gestalt-cli/src/run.rs`.

**Test scenarios:**
- Happy path: `workspace info` lists the expected four files under `.gestalt/`.
- Happy path: `workspace snapshot` returns valid git SHA/dirty/untracked metadata when run in the repository directory.
- Happy path: `workspace doctor` passes on minimal workspace fixture and correctly highlights permissions or missing files in a mocked broken directory.

**Verification:**
- `workspace` subcommand targets can be called and return correct info, snapshots, and doctor diagnostics.

---

### U5. Dispatch Commands in main.rs

**Goal:** Register the new subcommands in the clap command tree and wire them to the handler functions.

**Requirements:** R1, R2, R3, R4

**Dependencies:** U2, U3, U4

**Files:**
- Modify: `crates/gestalt-cli/src/main.rs`
- Test: `crates/gestalt-cli/tests/workspace_cli_tests.rs`

**Approach:**
- Modify the `Command` enum to include `Init { #[arg(long)] force: bool }`, `Status`, and `Workspace(WorkspaceCommand)`.
- Implement `WorkspaceCommand` argument group with subcommands: `Info`, `Snapshot`, `Doctor`.
- Wire subcommands in `main()` to the corresponding `workspace::*` handlers.
- Route results through `handle_result`.

**Patterns to follow:**
- Clap CLI parsing structure in `crates/gestalt-cli/src/main.rs`.

**Test scenarios:**
- Happy path: Running `gestalt status` dispatches status logic.
- Happy path: Running `gestalt workspace snapshot --format json` produces valid JSON payload containing snapshot metadata.
- Error path: Unrecognized subcommands exit with standard clap diagnostics and code 2.

**Verification:**
- Compilation of the CLI binary succeeds and all integration test cases pass.

---

## System-Wide Impact

- **Interaction graph:** Modifies CLI dispatch root (`main.rs`). The new commands do not affect `run`, `replay`, or `cost` execution paths.
- **Error propagation:** CLI-specific workspace errors are mapped to `HarnessError` variants (e.g. `ConfigError`), which are serialized and exited via `handle_result`.
- **State lifecycle risks:** `init` creates files on disk. Pre-flight check ensures existing configs are not partially overwritten or corrupted on failure.
- **Unchanged invariants:** Does not modify the underlying event schema or provider execution cycles in `gestalt-core`.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Overwriting existing config during `init` | Pre-flight check ensures any existing workspace file blocks creation unless `--force` is set. |
| Incomplete snapshot capturing in non-git directories | `GitWorkspaceSnapshotter` automatically falls back to full filesystem traversal when not in a git repo. |
| Slow diagnostics scans | We count runs by looking at files/folders directly instead of reading individual trace logs. |

---

## Documentation / Operational Notes

- **Command references:** Add new commands (`init`, `status`, `workspace info/snapshot/doctor`) to `README.md` and standard references.
- **Test environments:** Test suites will use temporary directories for file creation, keeping test state isolated.

---

## Sources & References

- Master Plan: [docs/plans/2026-06-02-001-feat-cli-tui-steering-wheel-plan.md](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/docs/plans/2026-06-02-001-feat-cli-tui-steering-wheel-plan.md)
- Snapshot API: [crates/gestalt-core/src/snapshot.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-core/src/snapshot.rs)
- Output Reports API: [crates/gestalt-cli/src/output.rs](file:///home/woshi/Code/Noentic/gestalt/gestalt-harness/crates/gestalt-cli/src/output.rs)
