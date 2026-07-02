---
title: "Migrating to Gestalt v0.1"
status: active
type: migration
target: v0.1
owners:
  - gestalt-core
  - gestalt-runtime
  - gestalt-app
  - gestalt-cli
---

# Migrating to Gestalt v0.1

Gestalt v0.1 is a greenfield compatibility cutoff:

- Legacy configuration formats and aliases are removed. Use
  [`gestalt.json` version 1](./configuration.md); unsupported files and fields
  fail validation.
- Extension manifests and lifecycle protocols are
  [V2 only](./extensions.md). There is no V1 fallback or migration path at
  runtime.
- APIs outside the documented [`gestalt_runtime::api::v1`
  boundary](./runtime-api.md) are experimental, moved under `unstable`, or
  internalized. Rust visibility alone is not a compatibility promise.
- Product-facing [client events are separate from persisted trace
  events](./trace-events.md). Clients must consume `ClientEventRecordV1`, not
  deserialize trace envelopes.

Pre-release `secret:` references are also unsupported. Configure credentials
through the supported environment and provider configuration described in the
[configuration contract](./configuration.md).
