use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use gestalt_core::{
    approval::{ApprovalDecision, ApprovalProvider, ApprovalRequest},
    error::HarnessError,
};

use crate::tui::bridge::TuiBridgeMessage;

/// Non-blocking approval provider that delegates requests to the TUI event loop.
pub struct TuiApprovalProvider {
    bridge_tx: mpsc::UnboundedSender<TuiBridgeMessage>,
}

impl TuiApprovalProvider {
    pub fn new(bridge_tx: mpsc::UnboundedSender<TuiBridgeMessage>) -> Self {
        Self { bridge_tx }
    }
}

#[async_trait]
impl ApprovalProvider for TuiApprovalProvider {
    async fn approve(&self, request: ApprovalRequest) -> Result<ApprovalDecision, HarnessError> {
        let (response_tx, response_rx) = oneshot::channel();

        // Forward the request to the TUI bridge
        if self
            .bridge_tx
            .send(TuiBridgeMessage::ApprovalRequest {
                request,
                response_tx,
            })
            .is_err()
        {
            // If the bridge is closed, the UI is terminating. Abort.
            return Err(HarnessError::Cancelled);
        }

        // Await the user's decision from the UI loop
        match response_rx.await {
            Ok(decision) => Ok(decision),
            Err(_) => {
                // The oneshot channel was dropped, meaning the UI cancelled or closed the prompt.
                Err(HarnessError::Cancelled)
            }
        }
    }
}
