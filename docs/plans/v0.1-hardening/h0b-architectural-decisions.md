---
title: H0B Architectural Decisions
status: accepted
type: decision-record
target: v0.1
owners:
  - gestalt-core
  - gestalt-runtime
  - gestalt-cli
---

# H0B Architectural Decisions

These accepted decisions resolve the architecture gates in the
[v0.1 hardening specification](../../feature-spec/v0.1-hardening.md). The stable
surface is separately enumerated in the [API/SPI inventory](./api-spi-inventory.md).

## Runtime Control

### Decision

- Callers may supply logical session IDs and idempotency keys; hosts assign
  physical run and turn IDs. A collision with different normalized input is
  `CONFLICT`.
- `start`, `continue`, `resume`, and `branch` preserve explicit parent run/turn
  lineage. Resume uses the latest valid checkpoint; branch creates a distinct
  logical session.
- A submission acknowledgement means accepted into the steering queue, not
  model or tool completion. Completion is observed through ordered events and
  run queries.
- Repeating an idempotency key with identical normalized input returns the
  original result. Reuse with different input returns `CONFLICT` without a
  second side effect.
- Each session has one active writer. The in-process steering queue is FIFO and
  bounded to 64 pending messages. Capacity returns `QUEUE_FULL`; callers are
  never blocked indefinitely.
- Cancellation is idempotent. A terminal run returns `cancelled: false`; a
  winning cancellation interrupts provider, approval, and tool waits while
  retaining committed events.
- Event cursors are opaque and ordered within one logical session. Lagged or
  expired cursors return their stable error and the newest safe cursor when
  available.
- Artifact access uses logical IDs and relative display paths. Each read is a
  bounded range; v0.1 implementations reject chunks above 1 MiB before
  allocation.

### Rejected

- Raw runtime models and broadcast receivers at the client boundary.
- Multiple active writers or an unbounded queue.
- An acknowledgement that ambiguously means both queued and completed.

### Consequences and evidence

The DTO contract is `gestalt_runtime::control::contract`; behavior is enforced
by `control_conformance`. The concrete session-owning host remains unpublished
until H1B passes the same suite for local and mock hosts.

## Event and Trace Representation

### Decision

One producing path yields three representations:

1. internal `AgentEvent` for execution;
2. persisted `EventEnvelope` carrying trace schema version, session/run/turn
   identity, sequence, timestamp, redaction state, generation/snapshot identity,
   and a typed `TraceEvent`;
3. client `EventEnvelopeV1` carrying client schema version, stable kind, typed
   payload, and public metadata.

Trace, client envelope, and each payload family own independent version
locations. The trace module owns persisted schemas; the control contract owns
client schemas; the producing domain owns payload meaning.

Readers ignore unknown additive fields. Persisted readers skip and diagnose an
unknown event kind without advancing an invalid sequence; malformed JSON or an
unsupported envelope version fails deterministically. Client projections may
map unknown kinds to an explicit unknown/omitted result but never deserialize
raw Rust enums as a compatibility mechanism.

### Rejected

- Serializing raw `AgentEvent` as the stable persisted or client contract.
- One version number shared by internal, persisted, and client forms.
- Silently accepting malformed or unsupported envelope versions.

### Consequences and evidence

H2A owns golden fixtures, unknown-field/kind tests, replay ordering, and removal
of pre-hardening readers. No trace contract is published before those pass.

## CLI Automation

### Decision

The stable candidate command subset is:

| Command | Kind |
|---|---|
| `config validate` | `config.validate` |
| `config explain` | `config.explain` |
| `workspace info` | `workspace.info` |
| `providers list`, `providers inspect` | `providers.list`, `providers.inspect` |
| `profiles list`, `profiles inspect` | `profiles.list`, `profiles.inspect` |
| `models list`, `models inspect` | `models.list`, `models.inspect` |
| `policy explain` | `policy.explain` |
| `tools list`, `tools inspect` | `tools.list`, `tools.inspect` |

All other commands are experimental for v0.1 unless this record is amended.
Selected commands become published only after H3B adds a normalized success and
failure snapshot for each feature combination.

The one JSON envelope is:

```json
{
  "schema_version": 1,
  "status": "success",
  "kind": "config.validate",
  "data": {},
  "error": null,
  "warnings": []
}
```

Failures set `status: "error"`, `data: null`, and emit the envelope to stderr.
This deliberately replaces the pre-freeze `{schema_version,kind,data}` draft;
the development line has no compatibility promise, so no second envelope or
adapter remains.

### Rejected

- Per-command JSON shapes.
- Stabilizing every command because it already exists.
- Supporting both the minimal draft and richer envelope.

### Consequences and evidence

`main_cli_contract_tests` enforces common output behavior. H3B owns command
snapshots, exits, optional-feature behavior, and final publication.

## Generation Lease and Pinning

### Decision

The pinning unit is one assistant turn. A session-owning host acquires a
`RuntimeSnapshotLease` before context/model/tool work and releases it after the
turn's terminal event and persistence cleanup. `run_prompt` may execute multiple
turns; each turn reacquires the then-current generation.

Reload publishes atomically. Existing leases retain the old generation; the
next turn sees the new generation. Failed candidates do not change the active
generation. Trace events and runtime inspection expose generation ID and
fingerprint.

This is the behavior accepted by
[ADR-029](../../adrs/ADR-029-runtime-snapshot-reload.md).

### Rejected

- Process- or session-lifetime pinning.
- Switching generations midway through a turn.
- Publishing a candidate before required validation succeeds.

### Consequences and evidence

H4B owns lease isolation, multi-turn adoption, failed-reload, draining, trace,
and inspection tests.

## Activation Failure Policy

### Decision

Host construction succeeds with the host plus an activation report, or fails
with a structured report. Each diagnostic contains stage
(`discovery`, `validation`, `launch`, `initialize`), package/component/instance
identity, criticality, stable code, redacted cause, and safe diagnostics.

| Criticality | Any stage failure |
|---|---|
| required security | fail closed; no usable host |
| required general | fail closed; no usable host |
| optional | reject component, report warning, continue |

No required security path can select fail-open. Expected discovery, validation,
launch, and initialization failures return reports and do not panic.

### Rejected

- Fail-open required components.
- Panics for expected activation failures.
- A success result that discards optional-component diagnostics.

### Consequences and evidence

H4B owns `runtime_builder_tests`, lifecycle protocol tests, trust tests, and the
tool-origin policy/approval/trace matrix.

## Affected Plans and Compatibility

| Decision | Owners | Compatibility impact |
|---|---|---|
| runtime control | H1A, H1B | raw control types remain experimental; stable DTOs are v1 |
| event/trace | H2A, H2B | pre-hardening readers are removed, not migrated |
| CLI | H3B | the minimal development envelope is replaced without an adapter |
| generation | H4B | ADR-029 turn lease is authoritative |
| activation | H4B | required failures fail closed; optional failures remain observable |

Any change to these decisions requires an accepted amendment and updated
conformance evidence before dependent plan status changes.
