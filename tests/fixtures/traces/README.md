# Trace Regression Fixtures

This directory currently holds targeted regression fixtures, not the full H2A golden matrix yet.

Fixtures now in use:

- `confirm-bash-golden/`
- `deny-read-secret-golden/`
- `yolo-bash-allowlist-golden/`
- `minimal-run.jsonl`

The `*/` fixtures each contain:

- `input.json`: prompt, mock provider turns, tools, session config, execution mode, policy TOML, and workspace snapshot.
- `context.json`: expected `ContextPacket` build state.
- `expected.jsonl`: expected `EventEnvelope` sequence.

`GoldenTraceRunner` uses these to cover the current replay and policy regression cases. The full normalized EVT-005 matrix, including normalization declarations for every scenario, is still tracked in `docs/plans/v0.1-hardening/H2A-event-trace-replay-contracts.md`.
