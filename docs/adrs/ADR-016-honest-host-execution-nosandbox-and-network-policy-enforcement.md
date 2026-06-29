# ADR-016: Honest Host Execution (NoSandbox) and Network Policy Enforcement

**Status:** Accepted

## Context
The `NoSandbox` executor ran processes directly on the host but was presented as a security boundary. It lacked mount/network confinement, did not enforce network policies, failed to clean up child process descendants on timeout, and classified bash commands weakly.

## Decision
Document `NoSandbox` as host execution and default bash to confirm unless on a small read-only allowlist. Enforce network policies in `NoSandbox` (failing closed when network access is disallowed). Run commands in their own process groups, killing the group on timeout. Treat shell operators and interpreter wrappers as high-risk.

## Consequences
A clear security model that does not promise false sandboxing. Improved stability through process-group cleanup. Hardened policy checks block command injection and unauthorized network access.
