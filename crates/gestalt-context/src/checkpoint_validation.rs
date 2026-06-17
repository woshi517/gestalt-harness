use gestalt_core::context::HistoryRange;
use gestalt_core::message::Message;
use gestalt_trace::CompactionCheckpoint;
use crate::{estimate_message_tokens, estimate_text_tokens};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    RangeMismatch,
    HashMismatch,
    ConstraintViolation(String),
    SummaryTooLarge,
}

pub fn validate_checkpoint(
    checkpoint: &CompactionCheckpoint,
    history: &[Message],
    expected_range: HistoryRange,
    expected_range_hash: &str,
) -> Result<(), ValidationError> {
    // 1. covered-range check
    if checkpoint.history_range != expected_range {
        return Err(ValidationError::RangeMismatch);
    }

    // 2. covered-range hash check
    if checkpoint.history_range_hash != expected_range_hash {
        return Err(ValidationError::HashMismatch);
    }

    // 3. protected constraints check
    if checkpoint.goal.trim().is_empty() {
        return Err(ValidationError::ConstraintViolation("goal is empty".to_string()));
    }
    if checkpoint.critical_context.trim().is_empty() {
        return Err(ValidationError::ConstraintViolation("critical_context is empty".to_string()));
    }

    // 4. summary size check
    let original_tokens = history[expected_range.start..expected_range.end]
        .iter()
        .map(estimate_message_tokens)
        .sum::<usize>();

    let rendered = checkpoint.render_markdown();
    let checkpoint_tokens = estimate_text_tokens(&rendered) + 4;

    if checkpoint_tokens >= original_tokens {
        return Err(ValidationError::SummaryTooLarge);
    }

    Ok(())
}
