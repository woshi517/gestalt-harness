use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Result, RuntimeError};
use crate::process_extension::ProcessExtensionBroker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionProcessState {
    Starting,
    Ready,
    Draining,
    Stopping,
    Stopped,
    Failed,
}

pub struct ExtensionProcessInstance {
    pub component_id: String,
    state: Arc<Mutex<ExtensionProcessState>>,
    in_flight: Arc<AtomicUsize>,
    broker: Option<Arc<ProcessExtensionBroker>>,
}

impl std::fmt::Debug for ExtensionProcessInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionProcessInstance")
            .field("component_id", &self.component_id)
            .field("state", &self.state())
            .field("in_flight_calls", &self.in_flight_calls())
            .finish_non_exhaustive()
    }
}

impl ExtensionProcessInstance {
    pub fn new(component_id: String) -> Self {
        Self {
            component_id,
            state: Arc::new(Mutex::new(ExtensionProcessState::Starting)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            broker: None,
        }
    }

    pub fn with_broker(component_id: String, broker: Arc<ProcessExtensionBroker>) -> Self {
        Self {
            component_id,
            state: Arc::new(Mutex::new(ExtensionProcessState::Starting)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            broker: Some(broker),
        }
    }

    pub fn state(&self) -> ExtensionProcessState {
        self.state
            .lock()
            .map_or(ExtensionProcessState::Failed, |state| *state)
    }

    pub fn transition_to(&self, state: ExtensionProcessState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = state;
        }
    }

    pub fn begin_call(&self) -> Result<InFlightCallGuard> {
        match self.state() {
            ExtensionProcessState::Draining
            | ExtensionProcessState::Stopping
            | ExtensionProcessState::Stopped
            | ExtensionProcessState::Failed => Err(RuntimeError::Extension(format!(
                "Extension process '{}' is not accepting new calls in state {:?}",
                self.component_id,
                self.state()
            ))),
            ExtensionProcessState::Starting | ExtensionProcessState::Ready => {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                Ok(InFlightCallGuard {
                    in_flight: self.in_flight.clone(),
                })
            }
        }
    }

    pub fn in_flight_calls(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub fn broker(&self) -> Option<Arc<ProcessExtensionBroker>> {
        self.broker.clone()
    }

    pub async fn shutdown(&self) {
        if let Some(broker) = &self.broker {
            broker.shutdown().await;
        }
        self.transition_to(ExtensionProcessState::Stopped);
    }
}

pub struct InFlightCallGuard {
    in_flight: Arc<AtomicUsize>,
}

impl Drop for InFlightCallGuard {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}
