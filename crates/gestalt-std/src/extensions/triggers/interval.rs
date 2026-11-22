#![forbid(unsafe_code)]
//! Local interval (`/loop`) trigger handler.

use std::time::Duration;

use gestalt_runtime::{
    CancellationToken, RuntimeError, TriggerContext, TriggerEvent, TriggerHandler,
    TriggerInstanceConfig, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

/// Parse a human duration string (`"500ms"`, `"5s"`, `"2m"`, `"1h"`) or
/// plain integer milliseconds into `u64` milliseconds.
///
/// # Errors
///
/// Returns [`RuntimeError::ArtifactError`] on empty, zero, or malformed strings.
pub fn parse_duration_ms(raw: &str) -> Result<u64, RuntimeError> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err(RuntimeError::ArtifactError(
            "duration cannot be empty".to_string(),
        ));
    }
    let (num_str, mult) = if let Some(n) = trimmed.strip_suffix("ms") {
        (n, 1_u64)
    } else if let Some(n) = trimmed.strip_suffix('s') {
        (n, 1_000_u64)
    } else if let Some(n) = trimmed.strip_suffix('m') {
        (n, 60_000_u64)
    } else if let Some(n) = trimmed.strip_suffix('h') {
        (n, 3_600_000_u64)
    } else {
        (trimmed.as_str(), 1_u64)
    };
    let val: u64 = num_str.trim().parse().map_err(|_| {
        RuntimeError::ArtifactError(format!(
            "invalid duration '{raw}' (expected e.g. 5s, 2m, 500ms)"
        ))
    })?;
    let ms = val.saturating_mul(mult);
    if ms == 0 {
        return Err(RuntimeError::ArtifactError(
            "interval duration must be > 0 ms".to_string(),
        ));
    }
    Ok(ms)
}

fn extract_interval_ms(config: &Value) -> Result<u64, RuntimeError> {
    if let Some(ms) = config.get("interval_ms").and_then(Value::as_u64) {
        if ms == 0 {
            return Err(RuntimeError::ArtifactError(
                "interval_ms must be > 0".to_string(),
            ));
        }
        return Ok(ms);
    }
    if let Some(raw) = config.get("interval").and_then(Value::as_str) {
        return parse_duration_ms(raw);
    }
    Err(RuntimeError::ArtifactError(
        "interval trigger requires 'interval_ms' or 'interval'".to_string(),
    ))
}

/// Local interval (`/loop`) [`TriggerHandler`].
#[derive(Debug, Clone, Default)]
pub struct IntervalTriggerHandler;

impl TriggerHandler for IntervalTriggerHandler {
    fn kind(&self) -> &str {
        "interval"
    }

    fn description(&self) -> &str {
        "Local fixed-interval timer trigger (/loop)"
    }

    fn validate(&self, config: &Value) -> Result<(), RuntimeError> {
        let _ = extract_interval_ms(config)?;
        Ok(())
    }

    fn start(
        &self,
        instance: TriggerInstanceConfig,
        _ctx: TriggerContext,
        event_tx: mpsc::UnboundedSender<TriggerEvent>,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<()>, RuntimeError> {
        let interval_ms = extract_interval_ms(&instance.config)?;
        let max_ticks = instance.config.get("max_ticks").and_then(Value::as_u64);
        let fire_immediately = instance
            .config
            .get("fire_immediately")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Ok(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms.max(10)));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            if !fire_immediately {
                ticker.tick().await;
            }
            let mut ticks = 0_u64;
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        ticks += 1;
                        let ev = TriggerEvent {
                            trigger_id: instance.id.clone(),
                            kind: instance.kind.clone(),
                            prompt: instance.prompt.clone(),
                            preset: instance.preset.clone(),
                            extra_decorators: instance.extra_decorators.clone(),
                            metadata: json!({
                                "tick": ticks,
                                "interval_ms": interval_ms,
                            }),
                            timestamp_ms: current_epoch_ms(),
                        };
                        if event_tx.send(ev).is_err() {
                            break;
                        }
                        if max_ticks.is_some_and(|max| ticks >= max) {
                            break;
                        }
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_runtime::sandbox::WorkspacePathGuard;
    use gestalt_runtime::{CredentialStore, SharedHttpClient, StreamRedactor};
    use std::collections::BTreeSet;

    #[test]
    fn duration_parser_supports_units_and_rejects_zero() {
        assert_eq!(parse_duration_ms("500ms").expect("ms"), 500);
        assert_eq!(parse_duration_ms("5s").expect("s"), 5_000);
        assert_eq!(parse_duration_ms("2m").expect("m"), 120_000);
        assert_eq!(parse_duration_ms("1h").expect("h"), 3_600_000);
        assert!(parse_duration_ms("0s").is_err());
        assert!(parse_duration_ms("bad").is_err());
    }

    #[tokio::test]
    async fn interval_trigger_fires_and_respects_max_ticks() {
        let dir = std::env::temp_dir();
        let ctx = TriggerContext {
            workspace_root: dir.clone(),
            path_guard: WorkspacePathGuard::new(&dir, &BTreeSet::new()).expect("guard"),
            credentials: CredentialStore::default(),
            http: SharedHttpClient::new(5).expect("http"),
            redactor: StreamRedactor::default(),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let handle = IntervalTriggerHandler
            .start(
                TriggerInstanceConfig {
                    id: "loop_1".into(),
                    kind: "interval".into(),
                    enabled: true,
                    prompt: json!("ping"),
                    preset: None,
                    extra_decorators: Vec::new(),
                    config: json!({ "interval_ms": 25, "max_ticks": 2 }),
                },
                ctx,
                tx,
                cancel,
            )
            .expect("start");

        let e1 = tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .expect("t1")
            .expect("e1");
        let e2 = tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .expect("t2")
            .expect("e2");
        assert_eq!(e1.metadata["tick"], 1);
        assert_eq!(e2.metadata["tick"], 2);
        let _ = handle.await;
    }
}
