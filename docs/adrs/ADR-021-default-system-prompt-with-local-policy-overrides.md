# ADR-021: Default System Prompt with Local Policy Overrides

**Status:** Accepted

## Context
The harness lacked a default system prompt for provider routing. Having to define a full prompt from scratch in every client workspace or fork core to update instructions was inefficient.

## Decision
Inject a sane, built-in system prompt (covering identity, environment, tool policy, and output formatting) as the first system message. Support local overrides from `gestalt.json` `prompt.override` / `prompt.override_file` or custom files.

## Consequences
Immediate out-of-the-box utility for CLI agents. Users can customize instructions per workspace without altering the core framework code.
