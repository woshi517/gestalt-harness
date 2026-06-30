use chrono::{DateTime, Utc};
use gestalt_core::context::HistoryRange;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

pub type MessageMetadataRef = gestalt_core::context::ProjectionMessageMetadata;
pub type ProjectionManifest = gestalt_core::context::ProjectionManifest;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionCheckpoint {
    pub v: u32,
    pub checkpoint_id: String,
    pub history_range: HistoryRange,
    pub history_range_hash: String,
    pub policy_version: String,
    pub compactor_model: String,
    pub prompt_hash: String,
    pub created_at: DateTime<Utc>,
    pub goal: String,
    pub constraints: Vec<String>,
    pub completed_work: Vec<String>,
    pub in_progress_work: Vec<String>,
    pub blocked_items: Vec<String>,
    pub key_decisions: Vec<String>,
    pub next_steps: Vec<String>,
    pub critical_context: String,
    pub relevant_references: Vec<String>,
}

impl CompactionCheckpoint {
    pub fn render_markdown(&self) -> String {
        let mut md = String::new();
        let _ = write!(
            md,
            "### Session Checkpoint Summary (ID: {})\n\n**Goal:** {}\n\n",
            self.checkpoint_id, self.goal
        );
        render_list(&mut md, "Constraints", &self.constraints);
        render_list(&mut md, "Completed Work", &self.completed_work);
        render_list(&mut md, "In Progress Work", &self.in_progress_work);
        render_list(&mut md, "Blocked Items", &self.blocked_items);
        render_list(&mut md, "Key Decisions", &self.key_decisions);
        render_list(&mut md, "Next Steps", &self.next_steps);
        let _ = write!(md, "**Critical Context:**\n{}\n\n", self.critical_context);
        render_list(&mut md, "Relevant References", &self.relevant_references);
        md
    }
}

fn render_list(output: &mut String, heading: &str, items: &[String]) {
    let _ = writeln!(output, "**{heading}:**");
    for item in items {
        let _ = writeln!(output, "- {item}");
    }
    output.push('\n');
}

#[cfg(all(test, not(feature = "trace")))]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_render_should_include_summary_without_trace() {
        let checkpoint = CompactionCheckpoint {
            v: 1,
            checkpoint_id: "checkpoint-1".to_string(),
            history_range: HistoryRange { start: 0, end: 1 },
            history_range_hash: String::new(),
            policy_version: String::new(),
            compactor_model: String::new(),
            prompt_hash: String::new(),
            created_at: Utc::now(),
            goal: "Keep context".to_string(),
            constraints: Vec::new(),
            completed_work: Vec::new(),
            in_progress_work: Vec::new(),
            blocked_items: Vec::new(),
            key_decisions: Vec::new(),
            next_steps: Vec::new(),
            critical_context: String::new(),
            relevant_references: Vec::new(),
        };

        assert!(checkpoint
            .render_markdown()
            .starts_with("### Session Checkpoint Summary"));
    }
}
