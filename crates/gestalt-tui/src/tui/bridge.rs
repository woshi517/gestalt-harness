use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tokio::sync::{mpsc, oneshot};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use gestalt_core::{
    approval::{ApprovalDecision, ApprovalRequest},
    error::HarnessError,
    event::AgentEvent,
};

/// Messages sent from the background agent execution thread to the TUI event loop.
pub enum TuiBridgeMessage {
    /// An event emitted by the agent loop (e.g. text token, tool call, checkpoint).
    AgentEvent(AgentEvent),
    /// A request for tool execution approval.
    ApprovalRequest {
        request: ApprovalRequest,
        response_tx: oneshot::Sender<ApprovalDecision>,
    },
    /// Notification that the run has completed, containing the path to the run directory or error.
    RunCompleted(Result<PathBuf, HarnessError>),
}

/// The receiver end of the TUI event bridge, managed by the TUI event loop.
pub struct TuiEventBridge {
    pub rx: mpsc::UnboundedReceiver<TuiBridgeMessage>,
}

impl TuiEventBridge {
    pub fn new(rx: mpsc::UnboundedReceiver<TuiBridgeMessage>) -> Self {
        Self { rx }
    }
}

// Global Diagnostics Log Ring Buffer
static DIAGNOSTICS_BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static DIAGNOSTICS_MAX_LINES: OnceLock<usize> = OnceLock::new();

/// Initializes the global diagnostics buffer with a maximum capacity.
pub fn init_diagnostics_buffer(max_lines: usize) {
    let _ = DIAGNOSTICS_MAX_LINES.set(max_lines);
    let _ = DIAGNOSTICS_BUFFER.set(Mutex::new(VecDeque::with_capacity(max_lines)));
}

/// Retrieves all currently buffered diagnostic logs.
pub fn get_diagnostics_logs() -> Vec<String> {
    if let Some(buf) = DIAGNOSTICS_BUFFER.get() {
        let lock = buf.lock().expect("Failed to lock diagnostics buffer");
        lock.iter().cloned().collect()
    } else {
        Vec::new()
    }
}

/// A tracing layer that intercepts standard logs and formats/buffers them in memory for TUI diagnostics.
pub struct TuiLogLayer;

impl<S> Layer<S> for TuiLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        struct MessageVisitor {
            msg: String,
        }

        impl tracing::field::Visit for MessageVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    let _ = write!(self.msg, "{:?}", value);
                } else {
                    let _ = write!(self.msg, " {}={:?}", field.name(), value);
                }
            }

            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "message" {
                    self.msg.push_str(value);
                } else {
                    let _ = write!(self.msg, " {}={}", field.name(), value);
                }
            }
        }

        let metadata = event.metadata();
        let level = metadata.level();
        let target = metadata.target();

        let mut visitor = MessageVisitor { msg: String::new() };
        event.record(&mut visitor);

        let log_line = format!("[{}] [{}] {}", level, target, visitor.msg);

        if let Some(buf) = DIAGNOSTICS_BUFFER.get() {
            if let Ok(mut lock) = buf.lock() {
                let max_lines = *DIAGNOSTICS_MAX_LINES.get().unwrap_or(&1000);
                lock.push_back(log_line);
                while lock.len() > max_lines {
                    lock.pop_front();
                }
            }
        }
    }
}
