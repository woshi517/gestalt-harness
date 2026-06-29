# ADR-009: Sandbox Deferred to v0.2+

**Status:** Accepted

## Context
No infrastructure access for bubblewrap or Docker. Implementing a stub now would create dead code and an incomplete safety promise.

## Decision
v0.1 uses `NoSandbox` (direct subprocess with working-dir restriction, timeout, output cap, env allowlist). The `ExecutionSandbox` trait is defined so v0.2 can drop in `BubblewrapSandbox` or `DockerSandbox` without changing the tool layer.

## Consequences
v0.1 has a weaker execution boundary. The interface contract is stable and ready for real sandbox implementations.
