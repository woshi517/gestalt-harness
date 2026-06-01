# Release Checklist

Use this checklist before tagging or publishing `gestalt-harness` v0.1.

## Packaging and Install

- Confirm `crates/gestalt-cli/Cargo.toml` declares package name `gestalt-harness`.
- Confirm the installed executable name remains `gestalt`.
- Run `cargo install --path crates/gestalt-cli` or `cargo install --locked --path crates/gestalt-cli`, or run `bash scripts/install-smoke.sh`.
- Verify the installed binary starts and `gestalt --workspace tests/fixtures/workspaces/minimal config validate` succeeds without provider credentials.

## Quality Gates

- Run `cargo fmt --all --check`.
- Run `cargo clippy --workspace --all-targets -- -D warnings`.
- Run `cargo test --workspace`.
- Run `bash scripts/check-deps.sh`.
- Run `bash scripts/check-binary-size.sh`.

## CI Evidence

- Confirm GitHub Actions CI passes on Linux and macOS.
- Confirm CI does not require live provider API keys.
- Confirm the Linux binary-size audit stays below the 10 MiB threshold.
- Capture the install-smoke output for release evidence.

## Docs and User Guidance

- Review `README.md` for platform support, install guidance, philosophy, and architecture overview.
- Review `CHANGELOG.md` for the `0.1.0` entry.
- Confirm the README, changelog, and release checklist all mention the `gestalt-harness` package and `gestalt` binary names.
- Confirm documented v0.1 non-goals still match product scope.

## Sanity Checks

- Replay at least one known trace with `gestalt replay`.
- Run `gestalt cost` on a recorded trace.
- Confirm trace output remains JSONL and auditable.
- Confirm default builds do not enable optional `tui`, `mcp`, `otel`, or PDF-related features unless explicitly requested.
