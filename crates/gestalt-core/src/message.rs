use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool_failure::ToolErrorReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentTrust {
    Trusted,
    Untrusted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: Vec<ContentBlock>,
    },
    Assistant {
        content: Vec<ContentBlock>,
    },
    /// Tool result message committed to the assistant history.
    ///
    /// `content` is the rendered string the model sees. `is_error`
    /// is the boolean signal; `failure` is the optional structured
    /// `ToolErrorReport` so the model and downstream tools can act
    /// on the failure class and any `repair_guidance` without having
    /// to re-parse the rendered content.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<ToolErrorReport>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    Image {
        source: ImageSource,
    },
    Document {
        source: DocumentSource,
        title: Option<String>,
        trust: ContentTrust,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSource {
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSource {
    pub media_type: String,
    pub data: String,
}
