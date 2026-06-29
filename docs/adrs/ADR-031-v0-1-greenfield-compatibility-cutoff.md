# ADR-031: v0.1 Greenfield Compatibility Cutoff

**Status:** Accepted

## Context

Gestalt has not released a stable v0.1 contract. During pre-release development,
the repository accumulated compatibility paths for earlier experimental
designs, including:

- split TOML harness configuration;
- deprecated `gestalt.json` aliases;
- extension manifest and process protocol V1;
- deprecated Rust builders, traits, aliases, and provider constructors;
- pre-hardening persistence migration and recovery branches.

Retaining these paths would make the first stable release responsible for
unreleased formats, enlarge the supported surface, and obscure the canonical
v0.1 architecture.

## Decision

Pre-hardening v0.1 is treated as greenfield. Compatibility obligations begin
with the stable v0.1 release.

### Canonical version identifiers

The cutoff does not mean that all identifiers named `v1` are removed:

- `gestalt.json` schema version 1 is the first supported stable configuration
  schema.
- Extension package manifest version 2 is the only supported extension
  manifest.
- Lifecycle Protocol V2 is the only supported Gestalt lifecycle protocol.
- Trace, run, context, and client contracts use the versions selected by their
  stable v0.1 specifications.

### Legacy harness configuration

The following files are unsupported:

```text
<workspace>/.gestalt/config.toml
<workspace>/.gestalt/policies.toml
<global-config>/gestalt/config.toml
```

If a known legacy harness configuration file is encountered during discovery,
loading, mutation, or diagnostics, Gestalt returns
`UNSUPPORTED_LEGACY_CONFIG`. The error identifies the legacy path and the
supported `gestalt.json` path.

Legacy files are not parsed, merged, migrated, seeded, renamed, or deleted.
Detection exists only to fail safely.

`gestalt.extension.toml` remains the canonical TOML package-manifest filename;
it must declare `manifest_version = 2`.

### Removed compatibility

Before stable v0.1:

1. Deprecated configuration aliases are removed from Rust models and JSON
   Schema.
2. Manifest/protocol V1 parsing, conversion, activation, and adapters are
   removed.
3. Deprecated Rust APIs and compatibility bridges are removed.
4. Pre-hardening persisted-format migration branches are removed.
5. Tests and active documentation stop accepting or advertising removed
   behavior.
6. Historical migration material may be archived but is not an active support
   path.

## Superseded Decisions

This ADR supersedes:

- ADR-025's legacy TOML fallback, transparent migration seeding, legacy config
  aliases, and compatibility-window decisions;
- ADR-030's Protocol V1 compatibility-through-adapters decision;
- ADR-024's `GestaltExtension` compatibility abstraction where superseded by
  `RuntimeModule` and manifest-V2 components.

The remaining decisions in ADR-024, ADR-025, and ADR-030 stay accepted.

## Consequences

### Positive

- Stable v0.1 begins with one canonical path per contract.
- Config loading and extension activation become smaller and easier to audit.
- Deprecated public APIs do not become accidental compatibility obligations.
- Tests can enforce absence instead of preserving migration behavior.

### Negative

- Pre-release users must manually rewrite old configuration and extension
  manifests.
- Old pre-hardening traces and checkpoints are not migrated.
- Encountering an old harness TOML file blocks configuration until the user
  removes it and creates `gestalt.json`.

### Required enforcement

The v0.1 hardening plan maintains a removal ledger and absence tests covering
production code, public exports, schemas, fixtures, tests, CLI/TUI diagnostics,
active documentation, and superseded ADR clauses.
