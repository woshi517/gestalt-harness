use std::path::Path;
use std::sync::{Arc, Mutex};
use gestalt_core::context::{ContextPipeline, TokenBudget};
use gestalt_core::message::Message;
use crate::error::Result;

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
            if let Some(pos) = messages.iter().position(|m| matches!(m, Message::System { .. })) {
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
}
