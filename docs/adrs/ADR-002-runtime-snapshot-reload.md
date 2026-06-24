# ADR-002 Runtime Snapshot Reload

Status: Accepted

The runtime owns immutable generation snapshots. `ExtensionManager` publishes snapshots atomically through `RwLock<Arc<RuntimeExtensionSnapshot>>`; runs adopt the active snapshot at run/session entry.

Reload candidates are validated before publication. Dry-runs return the candidate generation and fingerprint without publishing. Failed required candidates must leave the active generation untouched.

Deferred: automatic file watching, remote package registry integration, and optimizing publication with `arc-swap`.
