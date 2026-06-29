# ADR-017: Dedicated Verification Substrate (gestalt-verify)

**Status:** Accepted

## Context
Verifying task outcomes (e.g., verifying a file exists, confirming a test passes, checking for secrets, or validating a diff) was handled as ad-hoc event placeholders or left entirely to model self-reflection, leading to brittle success criteria.

## Decision
Introduce a dedicated `gestalt-verify` crate. Define `Verifier`, `VerifierRegistry`, and `VerifyContext` abstractions. Ship core verifiers (Command, FileExists, NoSecrets, PatchApplies, MarkdownStructure) and emit structured `VerificationResult` events after writes.

## Consequences
Success verification becomes independent of the model's output interpretation. The loop gains structured, testable assertions that run deterministically against the workspace.
