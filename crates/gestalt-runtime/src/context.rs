use crate::error::Result;
use gestalt_core::context::{ContextPipeline, TokenBudget};
use gestalt_core::message::Message;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[async_trait::async_trait]
pub trait ContextContributor: Send + Sync {
    fn name(&self) -> &str;
    async fn contribute(&self, workspace_root: &Path) -> Result<gestalt_core::message::Message>;
}

pub struct RuntimeContextPipeline {
    pub base: Arc<dyn ContextPipeline>,
    pub patch_store: Arc<Mutex<Vec<Message>>>,
}

impl ContextPipeline for RuntimeContextPipeline {
    fn process(&self, history: &[Message], budget: &TokenBudget) -> Vec<Message> {
        let mut messages = self.base.process(history, budget);
        let patches = self.patch_store.lock().unwrap().clone();
        if !patches.is_empty() {
            if let Some(pos) = messages
                .iter()
                .position(|m| matches!(m, Message::System { .. }))
            {
                for (i, msg) in patches.into_iter().enumerate() {
                    messages.insert(pos + 1 + i, msg);
                }
            } else {
                for (i, msg) in patches.into_iter().enumerate() {
                    messages.insert(i, msg);
                }
            }
        }
        messages
    }

    fn version(&self) -> &str {
        self.base.version()
    }

    fn build_packet(
        &self,
        history: &[Message],
        budget: &TokenBudget,
    ) -> gestalt_core::context::ContextPacket {
        use sha2::Digest;
        let mut packet = self.base.build_packet(history, budget);
        let patches = self.patch_store.lock().unwrap().clone();
        if !patches.is_empty() {
            let mut messages = packet.messages;
            if let Some(pos) = messages
                .iter()
                .position(|m| matches!(m, Message::System { .. }))
            {
                for (i, msg) in patches.into_iter().enumerate() {
                    messages.insert(pos + 1 + i, msg);
                }
            } else {
                for (i, msg) in patches.into_iter().enumerate() {
                    messages.insert(i, msg);
                }
            }

            let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
            let to_hash = format!("{serialized_messages}:{}", packet.pipeline_version);
            let mut hasher = sha2::Sha256::new();
            hasher.update(to_hash.as_bytes());
            let packet_hash = format!("{:x}", hasher.finalize());

            let message_hashes = messages
                .iter()
                .map(|msg| {
                    let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                    let mut hasher = sha2::Sha256::new();
                    hasher.update(msg_ser.as_bytes());
                    format!("{:x}", hasher.finalize())
                })
                .collect();

            packet.messages = messages;
            packet.packet_hash = packet_hash;
            packet.message_hashes = message_hashes;
        }
        packet
    }
}
