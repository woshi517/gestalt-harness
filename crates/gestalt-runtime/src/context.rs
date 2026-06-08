use crate::error::Result;
use gestalt_core::context::{
    ContextPipeline, PromptAssemblyStrategy, PromptCachePlan, PromptSegment, PromptSegmentKind,
    PromptSnapshot, TokenBudget,
};
use gestalt_core::message::Message;
use gestalt_core::ContextStability;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ContextPatch {
    pub message: Message,
    pub stability: ContextStability,
}

impl ContextPatch {
    pub fn new(message: Message, stability: ContextStability) -> Self {
        Self { message, stability }
    }
}

#[async_trait::async_trait]
pub trait ContextContributor: Send + Sync {
    fn name(&self) -> &str;
    fn stability(&self) -> ContextStability;
    async fn contribute(&self, workspace_root: &Path) -> Result<gestalt_core::message::Message>;
}

pub struct RuntimeContextPipeline {
    pub base: Arc<dyn ContextPipeline>,
    pub patch_store: Arc<Mutex<Vec<ContextPatch>>>,
}

impl ContextPipeline for RuntimeContextPipeline {
    fn process(&self, history: &[Message], budget: &TokenBudget) -> Vec<Message> {
        let messages = self.base.process(history, budget);
        let patches = self.patch_store.lock().unwrap().clone();
        Self::compose_messages(messages, &patches)
    }

    fn version(&self) -> &str {
        self.base.version()
    }

    fn build_packet(
        &self,
        history: &[Message],
        budget: &TokenBudget,
    ) -> gestalt_core::context::ContextPacket {
        let mut packet = self.base.build_packet(history, budget);
        let patches = self.patch_store.lock().unwrap().clone();
        if !patches.is_empty() {
            let (messages, stable_prefix_len) =
                Self::compose_messages_with_prefix(packet.messages.clone(), &patches);
            packet = Self::rebuild_packet(packet, messages, stable_prefix_len);
        }
        packet
    }
}

impl RuntimeContextPipeline {
    fn compose_messages(base_messages: Vec<Message>, patches: &[ContextPatch]) -> Vec<Message> {
        Self::compose_messages_with_prefix(base_messages, patches).0
    }

    fn compose_messages_with_prefix(
        base_messages: Vec<Message>,
        patches: &[ContextPatch],
    ) -> (Vec<Message>, usize) {
        let stable_prefix_len = base_messages
            .iter()
            .take_while(|message| !is_budget_notice(message) && matches!(message, Message::System { .. }))
            .count();

        let (stable_patches, unstable_patches): (Vec<_>, Vec<_>) = patches
            .iter()
            .cloned()
            .partition(|patch| is_stable(patch.stability));
        let stable_patch_count = stable_patches.len();

        let mut messages = Vec::with_capacity(
            base_messages.len() + stable_patch_count + unstable_patches.len(),
        );
        messages.extend(base_messages[..stable_prefix_len].iter().cloned());
        messages.extend(stable_patches.into_iter().map(|patch| patch.message));
        messages.extend(unstable_patches.into_iter().map(|patch| patch.message));
        messages.extend(base_messages[stable_prefix_len..].iter().cloned());

        (messages, stable_prefix_len + stable_patch_count)
    }

    fn rebuild_packet(
        mut packet: gestalt_core::context::ContextPacket,
        messages: Vec<Message>,
        stable_prefix_len: usize,
    ) -> gestalt_core::context::ContextPacket {
        use sha2::Digest;

        let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
        let to_hash = format!("{serialized_messages}:{}", packet.pipeline_version);
        let mut hasher = sha2::Sha256::new();
        hasher.update(to_hash.as_bytes());
        packet.packet_hash = format!("{:x}", hasher.finalize());

        packet.message_hashes = messages
            .iter()
            .map(|msg| {
                let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                let mut hasher = sha2::Sha256::new();
                hasher.update(msg_ser.as_bytes());
                format!("{:x}", hasher.finalize())
            })
            .collect();

        packet.messages = messages;

        if packet.cache_plan.is_some() {
            let stable_messages = packet.messages[..stable_prefix_len].to_vec();
            let snapshot = PromptSnapshot::new(stable_messages.clone(), 0);
            let mut segments = vec![PromptSegment::from_messages(
                PromptSegmentKind::Snapshot,
                ContextStability::SessionStatic,
                &stable_messages,
            )];

            let tail = &packet.messages[stable_prefix_len..];
            let (conversation_messages, ephemeral_messages) = split_tail_messages(tail);

            if !conversation_messages.is_empty() {
                segments.push(PromptSegment::from_messages(
                    PromptSegmentKind::Conversation,
                    ContextStability::TurnDynamic,
                    conversation_messages,
                ));
            }

            if !ephemeral_messages.is_empty() {
                segments.push(PromptSegment::from_messages(
                    PromptSegmentKind::Ephemeral,
                    ContextStability::Ephemeral,
                    ephemeral_messages,
                ));
            }

            let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot)
                .with_segments(segments.clone());
            packet.snapshot_hash = Some(snapshot.snapshot_hash);
            packet.cache_prefix_hash = Some(snapshot.prefix_hash);
            packet.segments = segments;
            packet.cache_plan = Some(plan);
        }

        packet
    }
}

fn is_stable(stability: ContextStability) -> bool {
    matches!(
        stability,
        ContextStability::SessionStatic | ContextStability::ActivationStatic
    )
}

fn is_budget_notice(message: &Message) -> bool {
    matches!(message, Message::System { content } if content.starts_with("context budget exhausted or truncated;"))
}

fn split_tail_messages(messages: &[Message]) -> (&[Message], &[Message]) {
    if matches!(messages.last(), Some(Message::System { content }) if content.starts_with("context budget exhausted or truncated;")) {
        messages.split_at(messages.len().saturating_sub(1))
    } else {
        (messages, &[])
    }
}
