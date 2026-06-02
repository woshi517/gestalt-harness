# Trace Regression Fixtures

This directory contains regression trace fixtures. Each fixture is structured as a directory with:
- `input.json`: Describes the prompt, mock provider turns, available tools, session config, execution mode, policy TOML, and workspace snapshot.
- `context.json`: Defines the expected ContextPacket build state.
- `expected.jsonl`: The sequence of `EventEnvelope` logs produced by running the agent loop.

These are used by `GoldenTraceRunner` to assert deterministic execution, policy evaluation, and event sequences.
