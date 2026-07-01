# CLI Golden Fixtures

Stable v0.1 automation commands and their semantic/snapshot evidence are listed
in [`docs/v0.1/cli-automation.md`](../../../docs/v0.1/cli-automation.md).
Only timestamps, generated IDs, hashes, and temporary paths may be normalized.
Secrets, ANSI escapes, and unexpected absolute paths must fail contract tests.

Contains legacy replay/cost golden files for validating CLI commands. The
stable H3B JSON envelope snapshots live in
`crates/gestalt-cli/tests/snapshots/` and are exercised by
`crates/gestalt-cli/tests/h3b_snapshot_tests.rs`.
