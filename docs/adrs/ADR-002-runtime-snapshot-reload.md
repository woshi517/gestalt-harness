# ADR-002 Runtime Snapshot Reload

Status: Accepted

The runtime owns immutable generation snapshots. `ExtensionManager` publishes snapshots atomically through `RwLock<Arc<RuntimeExtensionSnapshot>>`; runs adopt the active snapshot at run/session entry.

Reload candidates are validated before publication. Dry-runs return the candidate generation and fingerprint without publishing. Failed required candidates must leave the active generation untouched.

*Note on Implementation:* In Phase 1 scaffolding, the `reload_extensions` command acts as a generation-incrementing placeholder that clones the active snapshot and updates the generation and fingerprint. Real candidate reconstruction (including rediscovery, instance resolution, component diffing, validation, launching, draining, and rollback) is deferred.

Deferred: automatic file watching, remote package registry integration, optimizing publication with `arc-swap`, and transactional hot reload (candidate reconstruction, diffing, and draining).

