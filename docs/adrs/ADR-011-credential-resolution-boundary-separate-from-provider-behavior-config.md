# ADR-011: Credential Resolution Boundary Separate from Provider Behavior Config

**Status:** Accepted

## Context
The original provider/auth design anticipated keychain, vault, and session-backed credentials, but v0.1 needed safe shipping behavior immediately. Provider configs also needed to remain portable, reviewable, and secret-free.

## Decision
Provider configuration stores only behavioral settings plus auth selectors such as `api_key_env` and optional `auth_ref`. Concrete adapters receive a `CredentialResolver` boundary and never accept inline secrets. The resolver chain implements environment-backed, OS keychain (keyring), dynamic session, and interactive fallback credential resolution.

## Consequences
Secrets stay out of config and traces. Provider behavior remains deterministic under config precedence. Future keychain/vault/session support can be added without changing provider constructors or the core loop.
