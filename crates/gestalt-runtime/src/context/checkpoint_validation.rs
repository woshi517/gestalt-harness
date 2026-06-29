use crate::context::CompactionCheckpoint;
use crate::{estimate_message_tokens, estimate_text_tokens};
use gestalt_core::context::HistoryRange;
use gestalt_core::message::{ContentBlock, ContentTrust, Message};

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

    let protected_anchors = extract_protected_anchors(source_range);
    if !protected_anchors.is_empty() {
        let checkpoint_text = render_checkpoint_text(checkpoint);
        let matched = protected_anchors
            .iter()
            .filter(|anchor| checkpoint_mentions_anchor(&checkpoint_text, anchor))
            .count();
        let required_matches = protected_anchors.len().min(2);

        if matched < required_matches {
            return Err(ValidationError::ConstraintViolation(
                "checkpoint omitted explicit protected context from the compacted range"
                    .to_string(),
            ));
        }
    }

    let reference_anchors = extract_reference_anchors(source_range);
    if !reference_anchors.is_empty() {
        let references_text = normalize_text(&checkpoint.relevant_references.join(" "));
        if !reference_anchors
            .iter()
            .any(|anchor| checkpoint_mentions_anchor(&references_text, anchor))
        {
            return Err(ValidationError::ConstraintViolation(
                "checkpoint references do not preserve any source reference anchors".to_string(),
            ));
        }
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

fn render_checkpoint_text(checkpoint: &CompactionCheckpoint) -> String {
    normalize_text(
        &[
            checkpoint.goal.as_str(),
            checkpoint.critical_context.as_str(),
            &checkpoint.constraints.join(" "),
            &checkpoint.key_decisions.join(" "),
            &checkpoint.next_steps.join(" "),
        ]
        .join(" "),
    )
}

fn extract_protected_anchors(history: &[Message]) -> Vec<String> {
    let mut anchors = Vec::new();

    for message in history {
        let blocks = match message {
            Message::User { content, .. } => content,
            _ => continue,
        };

        for block in blocks {
            let text = match block {
                ContentBlock::Text { text } => text,
                ContentBlock::Document { source, .. } => &source.data,
                _ => continue,
            };

            for line in text.lines() {
                let trimmed = line.trim().trim_start_matches([
                    '-', '*', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', ')', ' ',
                ]);
                if !looks_protected(trimmed) {
                    continue;
                }

                anchors.push(trimmed.to_string());
            }
        }
    }

    anchors.sort_by_key(|anchor| std::cmp::Reverse(anchor.len()));
    anchors.dedup();
    anchors.truncate(4);
    anchors
}

fn extract_reference_anchors(history: &[Message]) -> Vec<String> {
    let mut anchors = Vec::new();

    for message in history {
        match message {
            Message::User { content, .. } | Message::Assistant { content } => {
                for block in content {
                    if let ContentBlock::Document { title, source, .. } = block {
                        if let Some(title) = title {
                            anchors.push(title.clone());
                        }
                        anchors.push(source.data.lines().next().unwrap_or_default().to_string());
                    }
                }
            }
            Message::ToolResult { artifact_refs, .. } => {
                if let Some(refs) = artifact_refs {
                    anchors.extend(refs.iter().cloned());
                }
            }
            Message::System { .. } => {}
        }
    }

    anchors.retain(|anchor| !anchor.trim().is_empty());
    anchors.truncate(4);
    anchors
}

fn looks_protected(line: &str) -> bool {
    let normalized = normalize_text(line);
    if normalized.len() < 16 {
        return false;
    }

    [
        "must",
        "should",
        "do not",
        "dont",
        "never",
        "always",
        "required",
        "important",
        "keep",
        "preserve",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn checkpoint_mentions_anchor(checkpoint_text: &str, anchor: &str) -> bool {
    let anchor_words = significant_words(anchor);
    if anchor_words.is_empty() {
        return false;
    }

    anchor_words
        .iter()
        .filter(|word| checkpoint_text.contains(word.as_str()))
        .count()
        >= std::cmp::min(2, anchor_words.len())
}

fn significant_words(text: &str) -> Vec<String> {
    normalize_text(text)
        .split_whitespace()
        .filter(|word| word.len() >= 4)
        .filter(|word| {
            !matches!(
                *word,
                "that"
                    | "this"
                    | "with"
                    | "from"
                    | "have"
                    | "will"
                    | "your"
                    | "into"
                    | "then"
                    | "they"
                    | "them"
                    | "when"
                    | "were"
            )
        })
        .map(ToString::to_string)
        .collect()
}

fn normalize_text(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect()
}
