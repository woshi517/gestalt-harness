---
title: Gestalt Policy and Approval v1
status: published
type: version-contract
target: v0.1
owners:
  - gestalt-runtime
---

# Gestalt v0.1: Policy & Approval Contract

This document specifies the v0.1 policy checking and user-in-the-loop approval contract. Runtime-backed tests prove approval responses against real policy evaluation and tool execution through the runtime-control boundary.

These specifications are constrained by the accepted [H0B Architectural Decisions](../plans/v0.1-hardening/h0b-architectural-decisions.md).

---

## 1. Policy Projections (`PolicyProjectionV1`)

Before executing any high-risk action (such as a tool call), the runtime evaluates it against active policies. The result of this evaluation is projected as a `PolicyProjectionV1` containing:

* **`tool_call_id`**: The target tool call ID.
* **`canonical_tool_id`**: The fully qualified name or URI of the tool.
* **`input_hash`**: A deterministic SHA-256 hash of the normalized tool input.
* **`risk_level`**: The classified risk level (`LOW`, `MEDIUM`, `HIGH`, `CRITICAL`).
* **`execution_backend`**: The execution locality/backend (`SANDBOX`, `LOCAL`, `REMOTE`). This is distinct from the core interaction policy mode (`CONFIRM`, `YOLO`, `HUMAN`, `DRY_RUN`, `REPLAY`).
* **`decision`**: The policy decision (`ALLOW`, `DENY`, `REQUIRES_APPROVAL`).
* **`reason`**: Descriptive rationale for the decision.
* **`matched_rule`**: The rule name or ID that triggered the decision.
* **`source`**: The file path or configuration source containing the rule.

---

## 2. Approval Challenge Model (`ApprovalProjectionV1`)

When a policy indicates that an action requires manual approval, a challenge is registered. The client polls or receives an `ApprovalProjectionV1` representing the challenge:

* **`approval_id`**: Unique identifier for this challenge instance.
* **`tool_call_id`**: Associated tool call ID.
* **`correlation_id`**: Links the challenge to the active logical session.
* **`summary`**: Human-readable summary of the action (e.g. "Execute command `rm -rf tmp`").
* **`editable_input_rules`**: A JSON schema specifying which fields of the tool input the user is permitted to edit, if any.
* **`original_hash`**: Hash of the input originally proposed by the agent.
* **`edited_hash`**: Hash of the input after modification by the user (populated in the projection if edited).
* **`expires_at`**: ISO 8601/RFC 3339 timestamp after which the challenge is invalid.
* **`is_cancelled`**: True if the associated execution was cancelled, rendering the challenge moot.
* **`session_grant_terms`**: Bounded session grant details if the user elects to permit similar future actions automatically.

### 2.1 Bounded Session Grants (`SessionGrantTermsV1`)
If a user approves an action with "Always Allow For Session", a bounded grant is created. The grant defines:
* The specific **tool name**.
* The exact normalized **input hash**. v0.1 grants do not authorize an editable input range.
* The **risk ceiling** (the grant will not cover higher risk invocations).
* The **matched rule** and **policy source** that produced the challenge.
* A fixed expiration window measured in conversational **turns**. A grant cannot silently broaden itself or persist across sessions.

---

## 3. Approval Response Validation (H1A-B06)

Clients respond to outstanding challenges using `RespondToApprovalRequestV1` with a decision of:
* **`Approve`**: Execute the action as originally proposed.
* **`Deny`**: Block execution (results in a policy rejection tool error).
* **`Edit(Value)`**: Execute the action with modified inputs.
* **`AlwaysAllowForSession`**: Approve the action and establish a bounded grant.

### 3.1 Hardened Validation Rules
To ensure secure execution, the following invariants are enforced:
1. **Duplicate or Late Responses**: If an approval response is received after the challenge has already been answered, expired, or cancelled, execution is blocked, and the response is rejected with `CONFLICT` or `EXPIRED_CURSOR`.
2. **Cancellation State**: If the run is cancelled, any pending approvals are marked `is_cancelled: true`. Any late response to a cancelled approval returns a `CONFLICT` error.
3. **Input Revalidation**: When the user edits the input via the `Edit` decision:
   - The edited input must conform to the `editable_input_rules` schema.
   - The edited input is re-evaluated by the policy engine as a completely new request.
   - Runtime-backed control accepts a schema-valid response into the waiting executor. The executor then re-evaluates the edited call under the active policy before execution. A denied or still-confirmed result becomes a policy/approval-denied tool result, and the tool does not execute.

The in-memory and mock control hosts validate DTO lifecycle and edit shape only; they do not claim to execute or re-evaluate tools.
