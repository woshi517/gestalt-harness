# Contributing to gestalt-harness

Thank you for contributing to `gestalt-harness`! This guide covers coding standards, workflows, quality gates, and system invariants.

## Development Workflow

1. Fork the repository and create a feature branch.
2. Implement your changes.
3. Verify that all quality gates pass locally.
4. Open a Pull Request (PR) describing the change and its rationale.
5. Once CI checks pass and code is reviewed, it will be merged into `main`.

## Quality Gates

Before merging any PR, the following commands MUST pass without errors or warnings:

```bash
# Verify formatting
cargo fmt --all --check

# Lint the workspace
cargo clippy --workspace --all-targets -- -D warnings

# Run all tests
cargo test --workspace

# Audit dependencies (enforces ADR-001)
bash scripts/check-deps.sh
```

## Code Standards

- **Linting:** We enforce `clippy::all` as `deny` and `clippy::pedantic` as `warn` at the workspace level.
- **Safety:**
  - `#[deny(unsafe_code)]` is active for all workspace members.
  - Never use `unwrap()` in library code. Use `expect("meaningful panic explanation")` ONLY in test code.
  - Do not use `panic!` on expected error paths. All fallible operations must return a `Result<T, HarnessError>`.
- **Documentation:** All public API items (modules, structs, enums, traits, functions, methods) must have rustdoc comments (`///`).
- **Dependencies:**
  - Dependencies must strictly adhere to the crate-level budgets defined in [the architecture document](docs/gestalt-harness-architecture.md#43-dependency-budget-revised).
  - Adding any new dependency requires justification in the PR description.
  - All shared dependency versions must be pinned in the workspace `Cargo.toml`.

## System Invariants & Architecture Guardrails

1. **Inverted Dependency Direction (ADR-001):**
   - Concrete crates depend on `gestalt-core`, NEVER the reverse.
   - `gestalt-core` must have ZERO path dependencies on concrete implementation crates.
2. **Purity of Core:**
   - `gestalt-core` must contain **zero** file I/O operations and **zero** network (HTTP) calls.
   - The sacred loop implementation (`gestalt-core/src/agent.rs`) must remain lightweight (target: under 200 lines).
3. **Crate Boundaries:**
   - `gestalt-tools` depends on `gestalt-exec` for subprocess execution.
   - `gestalt-context` orchestrates `gestalt-docs`, `gestalt-index`, and `gestalt-memory`.
4. **Git Hygiene:**
   - Never stage all changes indiscriminately (avoid `git add -A` or `git add .`). Manually inspect and stage specific files.
   - Avoid destructive git actions (`git reset --hard` or `git checkout .`) unless absolutely necessary.
   - Reference corresponding issues in commit messages (e.g., `fixes #N`).

## Testing Requirements

- **Tools:** Every tool must include tests for the happy path, schema validation, input risk classification, and path traversal vulnerabilities.
- **Providers:** Network calls must be mocked. Provider tests must consume recorded JSONL HTTP cassettes located in `tests/fixtures/provider-streams/`. No live API keys are permitted in CI.
- **Sacred Loop:** If you modify `AgentLoop`, you must update the mock-provider integration tests to verify correct state transitions.

## Security & Permissions

- No tool may execute without producing a corresponding `PolicyDecision` event from the policy engine.
- All content fetched from external sources (e.g. via MCP or tools) must be explicitly tagged as `ContentTrust::Untrusted`.
- Provider API keys and other secrets must never be committed to repository config files, traces, or test fixtures.
