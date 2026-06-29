# ADR-013: Bounded Session Approval Grants instead of Tool-Name Keys

**Status:** Accepted

## Context
Storing session-wide tool approvals by tool name alone (`allowed_session_tools`) meant approving one benign tool call (e.g., `bash echo "hi"`) auto-approved any future high-risk tool call (e.g., `bash rm -rf /`).

## Decision
Bounded grants (`SessionGrant`) containing input hash, risk ceiling, matched rule, and Turn-based expiry. Re-evaluate policy on every turn, and only auto-approve if the request's risk is at or below the grant's risk ceiling and the input hash matches the grant.

## Consequences
Eliminates risk escalation. The user can confidently approve one call without opening a blanket authorization for that tool. Traces record explicit approval provenance.
