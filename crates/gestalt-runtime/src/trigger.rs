#![forbid(unsafe_code)]
//! Host-level event & schedule `Trigger` primitives and `TriggerManager`.
//!
//! `gestalt-runtime` owns only the trait contract, lifecycle supervision, and
//! Turn/`FollowUp` dispatch plumbing; concrete trigger implementations (interval,
//! cron schedule, filesystem watcher, HTTP webhook) live in user space
//! (`gestalt-std` or external extensions).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::credentials::{CredentialStore, StreamRedactor};
use crate::provider::SharedHttpClient;
use crate::sandbox::WorkspacePathGuard;
use crate::{CancellationToken, RuntimeError};

/// Event emitted by a running [`TriggerHandler`] when its schedule or external
/// condition fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerEvent {
    /// Trigger instance identifier.
    pub trigger_id: String,
    /// Trigger paradigm/kind (`"interval"`, `"cron_schedule"`, `"fs_watch"`, `"webhook"`).
    pub kind: String,
    /// Prompt or structured input payload passed to the triggered Turn.
    pub prompt: Value,
    /// Optional target preset or loop ID (defaults to session's active preset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Optional extra decorators applied to the triggered Turn.
    #[serde(default)]
    pub extra_decorators: Vec<String>,
    /// Structured trigger metadata recorded on `custom:trigger:fired`.
    #[serde(default)]
    pub metadata: Value,
    /// Deterministic/wall-clock firing timestamp in milliseconds since Unix epoch.
    pub timestamp_ms: u64,
}

/// Configuration for a single active or paused trigger instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerInstanceConfig {
    /// Unique trigger instance identifier.
    pub id: String,
    /// Registered [`TriggerHandler::kind`] backing this instance.
    pub kind: String,
    /// Whether the trigger is enabled/running.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Prompt or JSON input dispatched when this trigger fires.
    pub prompt: Value,
    /// Optional target preset or loop ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Optional extra decorators applied to triggered Turns.
    #[serde(default)]
    pub extra_decorators: Vec<String>,
    /// Kind-specific trigger configuration JSON.
    #[serde(default)]
    pub config: Value,
}

fn default_true() -> bool {
    true
}

/// Public status snapshot for a supervised trigger instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerStatus {
    /// Trigger instance identifier.
    pub id: String,
    /// Trigger kind.
    pub kind: String,
    /// Human-readable handler description.
    pub description: String,
    /// `true` if enabled.
    pub enabled: bool,
    /// `true` if the background task is currently active.
    pub running: bool,
    /// Total number of firings observed.
    pub fire_count: u64,
    /// Timestamp (ms) of the most recent firing, if any.
    pub last_fired_ms: Option<u64>,
    /// Instance configuration JSON (with secrets scrubbed).
    pub config: Value,
}

#[derive(Debug, Default)]
struct ActivityInner {
    last_activity_ms: AtomicU64,
    turn_in_progress: AtomicBool,
    completed_turn_count: AtomicU64,
}

/// Read-only session activity snapshot shared with [`TriggerHandler`]s so
/// idle-aware and turn-count-aware triggers can schedule without polling.
#[derive(Debug, Clone, Default)]
pub struct ActivityTracker {
    inner: Arc<ActivityInner>,
}

impl ActivityTracker {
    /// Construct a fresh [`ActivityTracker`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Epoch timestamp (ms) of the most recent Turn start or completion.
    #[must_use]
    pub fn last_activity_ms(&self) -> u64 {
        self.inner.last_activity_ms.load(Ordering::Relaxed)
    }

    /// `true` while a foreground Turn is currently executing.
    #[must_use]
    pub fn turn_in_progress(&self) -> bool {
        self.inner.turn_in_progress.load(Ordering::Relaxed)
    }

    /// Total number of completed Turns observed on this [`RuntimeHost`].
    #[must_use]
    pub fn completed_turn_count(&self) -> u64 {
        self.inner.completed_turn_count.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_turn_started(&self, now_ms: u64) {
        self.inner.last_activity_ms.store(now_ms, Ordering::Relaxed);
        self.inner.turn_in_progress.store(true, Ordering::Relaxed);
    }

    pub(crate) fn mark_turn_ended(&self, now_ms: u64) {
        self.inner.last_activity_ms.store(now_ms, Ordering::Relaxed);
        self.inner.turn_in_progress.store(false, Ordering::Relaxed);
        self.inner
            .completed_turn_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Host capabilities supplied to [`TriggerHandler::start`].
#[derive(Clone)]
pub struct TriggerContext {
    /// Canonical workspace root path.
    pub workspace_root: PathBuf,
    /// In-process path guard enforcing workspace confinement for `fs_watch` triggers.
    pub path_guard: WorkspacePathGuard,
    /// Host credential store for webhook bearer tokens or cloud schedule auth.
    pub credentials: CredentialStore,
    /// Shared HTTP client for cloud schedule synchronization.
    pub http: SharedHttpClient,
    /// Secret redactor applied to incoming webhook/external trigger payloads.
    pub redactor: StreamRedactor,
    /// Read-only session activity & idle-state tracker.
    pub activity: ActivityTracker,
}

/// Async host contract for user-space event and schedule triggers.
pub trait TriggerHandler: Send + Sync {
    /// Unique trigger kind identifier (e.g., `"interval"`, `"cron_schedule"`).
    fn kind(&self) -> &str;

    /// Short human-readable description.
    fn description(&self) -> &str;

    /// Validate `config` before starting or persisting a trigger instance.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if required fields are missing or invalid.
    fn validate(&self, config: &Value) -> Result<(), RuntimeError>;

    /// Spawn the background trigger task emitting [`TriggerEvent`]s onto `event_tx`
    /// until `cancel` is triggered.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if configuration validation or resource binding fails.
    fn start(
        &self,
        instance: TriggerInstanceConfig,
        ctx: TriggerContext,
        event_tx: mpsc::UnboundedSender<TriggerEvent>,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<()>, RuntimeError>;
}

struct ManagedTrigger {
    instance: TriggerInstanceConfig,
    description: String,
    cancel: Option<CancellationToken>,
    handle: Option<JoinHandle<()>>,
    fire_count: u64,
    last_fired_ms: Option<u64>,
}

/// Host supervisor managing active background trigger tasks and forwarding
/// [`TriggerEvent`]s to surfaces and [`crate::RuntimeHost`].
pub struct TriggerManager {
    event_tx: mpsc::UnboundedSender<TriggerEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<TriggerEvent>>,
    triggers: BTreeMap<String, ManagedTrigger>,
}

impl Default for TriggerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TriggerManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

impl TriggerManager {
    /// Construct an empty [`TriggerManager`] with a fresh event channel.
    #[must_use]
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            event_tx,
            event_rx: Some(event_rx),
            triggers: BTreeMap::new(),
        }
    }

    /// Take the single [`mpsc::UnboundedReceiver<TriggerEvent>`] for surface
    /// `tokio::select!` multiplexing.
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<TriggerEvent>> {
        self.event_rx.take()
    }

    /// Clone the [`mpsc::UnboundedSender<TriggerEvent>`] (e.g., for external RPC
    /// `trigger/fire` notifications).
    #[must_use]
    pub fn event_sender(&self) -> mpsc::UnboundedSender<TriggerEvent> {
        self.event_tx.clone()
    }

    /// Validate and register (and start if `instance.enabled`) a trigger instance.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `handler.validate` or `handler.start` fails.
    pub fn start_trigger(
        &mut self,
        handler: &Arc<dyn TriggerHandler>,
        instance: TriggerInstanceConfig,
        ctx: TriggerContext,
    ) -> Result<(), RuntimeError> {
        handler.validate(&instance.config)?;
        let id = instance.id.clone();
        let _ = self.remove_trigger(&id);

        let (cancel, handle) = if instance.enabled {
            let cancel = CancellationToken::new();
            let handle =
                handler.start(instance.clone(), ctx, self.event_tx.clone(), cancel.clone())?;
            (Some(cancel), Some(handle))
        } else {
            (None, None)
        };

        self.triggers.insert(
            id,
            ManagedTrigger {
                instance,
                description: handler.description().to_string(),
                cancel,
                handle,
                fire_count: 0,
                last_fired_ms: None,
            },
        );
        Ok(())
    }

    /// Pause a running trigger by cancelling its background task without removing
    /// its configuration. Returns `false` if `id` is unknown.
    pub fn pause_trigger(&mut self, id: &str) -> bool {
        let Some(entry) = self.triggers.get_mut(id) else {
            return false;
        };
        entry.instance.enabled = false;
        if let Some(cancel) = entry.cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = entry.handle.take() {
            handle.abort();
        }
        true
    }

    /// Resume a paused trigger instance.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `id` is unknown or restarting the handler fails.
    pub fn resume_trigger(
        &mut self,
        id: &str,
        handler: &Arc<dyn TriggerHandler>,
        ctx: TriggerContext,
    ) -> Result<(), RuntimeError> {
        let Some(entry) = self.triggers.get_mut(id) else {
            return Err(RuntimeError::ArtifactError(format!(
                "unknown trigger '{id}'"
            )));
        };
        if let Some(cancel) = entry.cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = entry.handle.take() {
            handle.abort();
        }
        entry.instance.enabled = true;
        let cancel = CancellationToken::new();
        let handle = handler.start(
            entry.instance.clone(),
            ctx,
            self.event_tx.clone(),
            cancel.clone(),
        )?;
        entry.cancel = Some(cancel);
        entry.handle = Some(handle);
        Ok(())
    }

    /// Look up the `kind` of a registered trigger instance.
    #[must_use]
    pub fn trigger_kind(&self, id: &str) -> Option<&str> {
        self.triggers.get(id).map(|t| t.instance.kind.as_str())
    }

    /// Stop and remove a trigger instance. Returns `false` if `id` is unknown.
    pub fn remove_trigger(&mut self, id: &str) -> bool {
        let Some(mut entry) = self.triggers.remove(id) else {
            return false;
        };
        if let Some(cancel) = entry.cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = entry.handle.take() {
            handle.abort();
        }
        true
    }

    /// Manually fire a registered trigger immediately and emit its [`TriggerEvent`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ArtifactError`] if `id` is unknown.
    pub fn fire_now(&mut self, id: &str, timestamp_ms: u64) -> Result<TriggerEvent, RuntimeError> {
        let Some(entry) = self.triggers.get_mut(id) else {
            return Err(RuntimeError::ArtifactError(format!(
                "unknown trigger '{id}'"
            )));
        };
        entry.fire_count += 1;
        entry.last_fired_ms = Some(timestamp_ms);
        let ev = TriggerEvent {
            trigger_id: entry.instance.id.clone(),
            kind: entry.instance.kind.clone(),
            prompt: entry.instance.prompt.clone(),
            preset: entry.instance.preset.clone(),
            extra_decorators: entry.instance.extra_decorators.clone(),
            metadata: json!({ "manual_fire": true }),
            timestamp_ms,
        };
        let _ = self.event_tx.send(ev.clone());
        Ok(ev)
    }

    /// Record that `trigger_id` fired at `timestamp_ms`.
    pub fn record_fired(&mut self, trigger_id: &str, timestamp_ms: u64) {
        if let Some(entry) = self.triggers.get_mut(trigger_id) {
            entry.fire_count += 1;
            entry.last_fired_ms = Some(timestamp_ms);
        }
    }

    /// Return status snapshots for all registered triggers.
    #[must_use]
    pub fn list_status(&self) -> Vec<TriggerStatus> {
        self.triggers
            .values()
            .map(|t| TriggerStatus {
                id: t.instance.id.clone(),
                kind: t.instance.kind.clone(),
                description: t.description.clone(),
                enabled: t.instance.enabled,
                running: t.handle.as_ref().is_some_and(|h| !h.is_finished()),
                fire_count: t.fire_count,
                last_fired_ms: t.last_fired_ms,
                config: t.instance.config.clone(),
            })
            .collect()
    }

    /// Cancel and abort all supervised trigger tasks.
    pub fn shutdown_all(&mut self) {
        for (_, mut entry) in std::mem::take(&mut self.triggers) {
            if let Some(cancel) = entry.cancel.take() {
                cancel.cancel();
            }
            if let Some(handle) = entry.handle.take() {
                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::WorkspacePathGuard;
    use std::collections::BTreeSet;
    use std::time::Duration;

    struct OneTickHandler;
    impl TriggerHandler for OneTickHandler {
        fn kind(&self) -> &str {
            "mock_tick"
        }
        fn description(&self) -> &str {
            "Mock tick trigger"
        }
        fn validate(&self, _config: &Value) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn start(
            &self,
            instance: TriggerInstanceConfig,
            _ctx: TriggerContext,
            event_tx: mpsc::UnboundedSender<TriggerEvent>,
            cancel: CancellationToken,
        ) -> Result<JoinHandle<()>, RuntimeError> {
            Ok(tokio::spawn(async move {
                tokio::select! {
                    () = cancel.cancelled() => {}
                    () = tokio::time::sleep(Duration::from_millis(15)) => {
                        let _ = event_tx.send(TriggerEvent {
                            trigger_id: instance.id,
                            kind: instance.kind,
                            prompt: instance.prompt,
                            preset: instance.preset,
                            extra_decorators: instance.extra_decorators,
                            metadata: json!({ "tick": 1 }),
                            timestamp_ms: 123,
                        });
                    }
                }
            }))
        }
    }

    #[tokio::test]
    async fn trigger_manager_lifecycle_pause_resume_fire_and_shutdown() {
        let dir = std::env::temp_dir().join(format!("gestalt_trig_mgr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let canon = std::fs::canonicalize(&dir).expect("canon");

        let ctx = TriggerContext {
            workspace_root: canon.clone(),
            path_guard: WorkspacePathGuard::new(&canon, &BTreeSet::new()).expect("guard"),
            credentials: CredentialStore::default(),
            http: SharedHttpClient::new(5).expect("http"),
            redactor: StreamRedactor::default(),
            activity: ActivityTracker::default(),
        };

        let mut mgr = TriggerManager::new();
        let mut rx = mgr.take_event_receiver().expect("rx");
        let handler: Arc<dyn TriggerHandler> = Arc::new(OneTickHandler);

        mgr.start_trigger(
            &handler,
            TriggerInstanceConfig {
                id: "t1".into(),
                kind: "mock_tick".into(),
                enabled: true,
                prompt: json!("check status"),
                preset: None,
                extra_decorators: Vec::new(),
                config: json!({}),
            },
            ctx.clone(),
        )
        .expect("start trigger");

        let ev = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        assert_eq!(ev.trigger_id, "t1");
        assert_eq!(ev.prompt, json!("check status"));

        assert!(mgr.pause_trigger("t1"));
        assert!(!mgr.list_status()[0].enabled);
        let manual = mgr.fire_now("t1", 999).expect("fire_now");
        assert_eq!(manual.timestamp_ms, 999);
        assert_eq!(mgr.list_status()[0].fire_count, 1);

        mgr.resume_trigger("t1", &handler, ctx).expect("resume");
        assert!(mgr.list_status()[0].enabled);

        assert!(mgr.remove_trigger("t1"));
        assert!(mgr.list_status().is_empty());
        mgr.shutdown_all();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
