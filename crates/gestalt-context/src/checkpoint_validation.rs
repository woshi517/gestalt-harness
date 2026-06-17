use crate::{estimate_message_tokens, estimate_text_tokens};
use gestalt_core::context::HistoryRange;
use gestalt_core::message::{ContentBlock, ContentTrust, Message};
use gestalt_trace::CompactionCheckpoint;

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
    let source_range = &history[expected_range.start..expected_range.end];

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
        return Err(ValidationError::ConstraintViolation(
            "goal is empty".to_string(),
        ));
    }
    if checkpoint.critical_context.trim().is_empty() {
        return Err(ValidationError::ConstraintViolation(
            "critical_context is empty".to_string(),
        ));
    }
    if checkpoint
        .constraints
        .iter()
        .any(|item| item.trim().is_empty())
    {
        return Err(ValidationError::ConstraintViolation(
            "constraints contain an empty item".to_string(),
        ));
    }
    if checkpoint
        .relevant_references
        .iter()
        .any(|item| item.trim().is_empty())
    {
        return Err(ValidationError::ConstraintViolation(
            "relevant_references contain an empty item".to_string(),
        ));
    }

    let source_has_user_constraints = source_range
        .iter()
        .any(|message| matches!(message, Message::User { .. }));
    if source_has_user_constraints && checkpoint.constraints.is_empty() {
        return Err(ValidationError::ConstraintViolation(
            "user constraints were present in source history but checkpoint constraints are empty"
                .to_string(),
        ));
    }

    let source_has_references = source_range.iter().any(message_has_reference);
    if source_has_references && checkpoint.relevant_references.is_empty() {
        return Err(ValidationError::ConstraintViolation(
            "source history contains references but checkpoint relevant_references are empty"
                .to_string(),
        ));
    }

    let source_has_untrusted_content = source_range.iter().any(message_has_untrusted_content);
    if source_has_untrusted_content && checkpoint.relevant_references.is_empty() {
        return Err(ValidationError::ConstraintViolation(
            "untrusted source content requires relevant references in the checkpoint".to_string(),
        ));
    }

    // 4. summary size check
    let original_tokens = source_range
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

fn message_has_reference(message: &Message) -> bool {
    match message {
        Message::User { content, .. } | Message::Assistant { content } => content
            .iter()
            .any(|block| matches!(block, ContentBlock::Document { .. })),
        Message::ToolResult { artifact_refs, .. } => {
            artifact_refs.as_ref().is_some_and(|refs| !refs.is_empty())
        }
        Message::System { .. } => false,
    }
}

fn message_has_untrusted_content(message: &Message) -> bool {
    match message {
        Message::User { content, .. } | Message::Assistant { content } => {
            content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Document {
                        trust: ContentTrust::Untrusted,
                        ..
                    }
                )
            })
        }
        Message::ToolResult { .. } => true,
        Message::System { .. } => false,
    }
}
