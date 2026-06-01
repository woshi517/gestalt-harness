use crate::message::Message;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn sha256_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSourceRef {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub token_estimate: usize,
    pub included: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextOmission {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub reason: String,
    pub token_estimate: usize,
}

pub trait ContextPipeline: Send + Sync {
    fn process(&self, history: &[Message], budget: &TokenBudget) -> Vec<Message>;

    fn version(&self) -> &str;

    fn build_packet(&self, history: &[Message], budget: &TokenBudget) -> ContextPacket {
        let messages = self.process(history, budget);
        let version = self.version().to_string();
        let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
        let to_hash = format!("{}:{}" , serialized_messages, version);
        let packet_hash = sha256_hash(&to_hash);
        
        let message_hashes = messages.iter().map(|msg| {
            let msg_ser = serde_json::to_string(msg).unwrap_or_default();
            sha256_hash(&msg_ser)
        }).collect();

        ContextPacket {
            messages,
            packet_hash,
            pipeline_version: version,
            tokenizer_id: "default".to_string(),
            token_estimate: 0,
            sources: Vec::new(),
            omissions: Vec::new(),
            message_hashes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
