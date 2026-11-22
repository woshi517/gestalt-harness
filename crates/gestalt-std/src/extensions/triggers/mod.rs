#![forbid(unsafe_code)]
//! Standard user-space `TriggerHandler` implementations (`interval`,
//! `cron_schedule`, `fs_watch`, `webhook`).

pub mod cron;
pub mod fs_watch;
pub mod interval;
pub mod webhook;

use std::sync::Arc;

use gestalt_runtime::RuntimeRegistry;

pub use cron::{CronSchedule, CronScheduleTriggerHandler};
pub use fs_watch::FsWatchTriggerHandler;
pub use interval::{IntervalTriggerHandler, parse_duration_ms};
pub use webhook::WebhookTriggerHandler;

/// Register all four `gestalt-std` user-space [`gestalt_runtime::TriggerHandler`]
/// implementations into [`RuntimeRegistry::trigger_handlers`].
pub fn register_std_triggers(runtime: &mut RuntimeRegistry) {
    let handlers: [Arc<dyn gestalt_runtime::TriggerHandler>; 4] = [
        Arc::new(IntervalTriggerHandler),
        Arc::new(CronScheduleTriggerHandler),
        Arc::new(FsWatchTriggerHandler),
        Arc::new(WebhookTriggerHandler),
    ];
    for h in handlers {
        runtime.trigger_handlers.insert(h.kind().to_string(), h);
    }
}
