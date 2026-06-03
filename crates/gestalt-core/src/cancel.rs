use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// A lightweight, cloneable token to coordinate graceful cancellation.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<CancellationToken>);

impl CancelToken {
    /// Creates a new uncancelled token.
    pub fn new() -> Self {
        Self(Arc::new(CancellationToken::new()))
    }

    /// Triggers cancellation. All child tokens and futures awaiting this token will be notified.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Returns `true` if the token has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Awaits the cancellation event.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }

    /// Creates a child token that is cancelled when this token is cancelled,
    /// but can also be cancelled independently.
    pub fn child_token(&self) -> Self {
        Self(Arc::new(self.0.child_token()))
    }
}
