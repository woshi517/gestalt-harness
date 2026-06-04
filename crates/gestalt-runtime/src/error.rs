use thiserror::Error;
use gestalt_core::error::HarnessError;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("harness error: {0}")]
    Harness(#[from] HarnessError),

    #[error("builder error: {0}")]
    Builder(String),

    #[error("registry error: {0}")]
    Registry(String),

    #[error("extension error: {0}")]
    Extension(String),

    #[error("hook error: {0}")]
    Hook(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;
