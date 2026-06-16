use crate::message::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn sha256_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hash_message(message: &Message) -> String {
    serde_json::to_string(message)
        .map(|serialized| sha256_hash(&serialized))
        .unwrap_or_default()
}

fn hash_messages(messages: &[Message]) -> Vec<String> {
    messages.iter().map(hash_message).collect()
}

fn hash_message_list(messages: &[Message], created_turn: usize) -> String {
    let serialized = serde_json::to_string(messages).unwrap_or_default();
    sha256_hash(&format!("{serialized}:{created_turn}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptAssemblyStrategy {
    #[default]
    Dynamic,
    Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptSegmentKind {
    #[default]
    Conversation,
    Snapshot,
    Dynamic,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextStability {
    SessionStatic,
    ActivationStatic,
    #[default]
    TurnDynamic,
    Ephemeral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSegment {
    pub kind: PromptSegmentKind,
    pub stability: ContextStability,
    pub hash: String,
    pub message_count: usize,
}

impl PromptSegment {
    pub fn from_messages(
        kind: PromptSegmentKind,
        stability: ContextStability,
        messages: &[Message],
    ) -> Self {
        let message_hashes = hash_messages(messages);
        let hash = sha256_hash(
            &serde_json::to_string(&serde_json::json!({
                "kind": kind,
                "stability": stability,
                "message_hashes": message_hashes,
            }))
            .unwrap_or_default(),
        );

        Self {
            kind,
            stability,
            hash,
            message_count: messages.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSnapshot {
    pub snapshot_hash: String,
    pub prefix_hash: String,
    pub created_turn: usize,
    pub messages: Vec<Message>,
    pub message_hashes: Vec<String>,
}

impl PromptSnapshot {
    pub fn new(messages: Vec<Message>, created_turn: usize) -> Self {
        let prefix_hash = sha256_hash(&serde_json::to_string(&messages).unwrap_or_default());
        let message_hashes = hash_messages(&messages);
        let snapshot_hash = hash_message_list(&messages, created_turn);

        Self {
            snapshot_hash,
            prefix_hash,
            created_turn,
            messages,
            message_hashes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptCachePlan {
    pub strategy: PromptAssemblyStrategy,
    pub snapshot_hash: String,
    pub prefix_hash: String,
    pub prefix_message_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<PromptSegment>,
}

impl PromptCachePlan {
    pub fn new(strategy: PromptAssemblyStrategy, snapshot: &PromptSnapshot) -> Self {
        Self {
            strategy,
            snapshot_hash: snapshot.snapshot_hash.clone(),
            prefix_hash: snapshot.prefix_hash.clone(),
            prefix_message_count: snapshot.messages.len(),
            segments: Vec::new(),
        }
    }

    pub fn with_segments(mut self, segments: Vec<PromptSegment>) -> Self {
        self.segments = segments;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPacket {
    pub messages: Vec<Message>,
    pub packet_hash: String,
    pub pipeline_version: String,
    pub tokenizer_id: String,
    pub token_estimate: usize,
    pub sources: Vec<ContextSourceRef>,
    pub omissions: Vec<ContextOmission>,
    pub message_hashes: Vec<String>,
    #[serde(default)]
    pub prompt_assembly_strategy: PromptAssemblyStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_prefix_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<PromptSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_plan: Option<PromptCachePlan>,
    #[serde(default)]
    pub prompt_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSourceRef {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub token_estimate: usize,
    pub included: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextOmission {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub reason: String,
    pub token_estimate: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

pub trait ContextPipeline: Send + Sync {
    fn process(&self, history: &[Message], budget: &TokenBudget) -> Vec<Message>;

    fn version(&self) -> &str;

    fn build_packet(&self, history: &[Message], budget: &TokenBudget) -> ContextPacket {
        let messages = self.process(history, budget);
        let version = self.version().to_string();
        let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
        let to_hash = format!("{serialized_messages}:{version}");
        let packet_hash = sha256_hash(&to_hash);

        let message_hashes = messages
            .iter()
            .map(|msg| {
                let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                sha256_hash(&msg_ser)
            })
            .collect();

        ContextPacket {
            messages,
            packet_hash,
            pipeline_version: version,
            tokenizer_id: "default".to_string(),
            token_estimate: 0,
            sources: Vec::new(),
            omissions: Vec::new(),
            message_hashes,
            prompt_assembly_strategy: PromptAssemblyStrategy::Dynamic,
            snapshot_hash: None,
            cache_prefix_hash: None,
            segments: Vec::new(),
            cache_plan: None,
            prompt_source: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct TokenBudget {
    pub model_limit: usize,
    pub reserved_output: usize,
    pub used_system: usize,
    pub used_history: usize,
    pub used_sources: usize,
    pub used_tools: usize,
    pub used_memory: usize,
    pub minimum_turn_budget: usize,
}

impl TokenBudget {
    pub fn available_total(&self) -> usize {
        self.model_limit
            .saturating_sub(self.reserved_output)
            .saturating_sub(self.used_system)
            .saturating_sub(self.used_history)
            .saturating_sub(self.used_sources)
            .saturating_sub(self.used_tools)
            .saturating_sub(self.used_memory)
    }

    pub fn exhausted(&self) -> bool {
        self.available_total() < self.minimum_turn_budget
    }

    pub fn record_usage(&mut self, input_tokens: usize, _output_tokens: usize) {
        let non_history = self
            .used_system
            .saturating_add(self.used_sources)
            .saturating_add(self.used_tools)
            .saturating_add(self.used_memory);
        self.used_history = input_tokens.saturating_sub(non_history);
    }
}
