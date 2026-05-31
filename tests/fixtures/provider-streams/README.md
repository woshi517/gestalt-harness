# Provider Stream Fixtures

Each file in this directory is a recorded SSE stream from a provider API response.

## Naming Convention

`<provider>-<scenario>.jsonl`

Examples:
- `anthropic-text-only.jsonl`
- `anthropic-single-tool-call.jsonl`
- `openai-multiple-tool-calls.jsonl`

## Format

Each line is a JSON object representing one SSE event in the provider's native format.
Provider adapters consume these in integration tests instead of making live API calls.

## Adding New Fixtures

1. Record the raw SSE stream from the provider API.
2. Save as `.jsonl` with one SSE event per line.
3. **Redact all API keys, tokens, and secrets before committing.**
4. Add a corresponding test in `gestalt-models/tests/`.
