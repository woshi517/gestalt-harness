# ADR-029 Runtime Snapshot Reload

Status: Accepted

The runtime owns immutable generation snapshots. `ExtensionManager` publishes snapshots atomically through `RwLock<Arc<RuntimeExtensionSnapshot>>`; assistant turns adopt a `RuntimeSnapshotLease` at run/turn entry and execute against the pinned snapshot for the duration of that turn.

Startup and reload both execute through `ExtensionActivationPipeline`. Reload candidates are validated before publication. Dry-runs return the candidate generation and fingerprint without publishing. Failed required candidates must leave the active generation untouched.

`ActivationCandidate` owns newly-started resources until commit. When a new generation is published, the previous generation is retired but remains callable while leases exist. Non-reused resources drain only after the final lease releases.

Deferred: automatic file watching, remote package registry integration, optimized publication via arc-swap, and richer reload reporting.
