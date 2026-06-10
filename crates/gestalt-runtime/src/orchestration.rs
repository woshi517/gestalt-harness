use crate::artifact_store::ArtifactStore;
use crate::builder::AgentRuntimeBuilder;
use crate::error::{Result, RuntimeError};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::runtime::AgentRuntime;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub struct OrchestrationTask {
    pub prompt: String,
    pub input_artifacts: Vec<String>,
}

pub struct OrchestrationResult {
    pub output: String,
    pub output_artifacts: Vec<String>,
}

#[async_trait::async_trait]
pub trait Orchestrator: Send + Sync {
    async fn execute(
        &self,
        handle: Arc<dyn AgentRuntimeHandle>,
        task: OrchestrationTask,
    ) -> Result<OrchestrationResult>;
}

#[async_trait::async_trait]
pub trait AgentRuntimeHandle: Send + Sync {
    async fn spawn_session(
        &self,
        session_id: &str,
        config_override: Option<crate::config::RuntimeConfig>,
    ) -> Result<String>;
    async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult>;
    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>>;
    fn artifact_store(&self) -> Arc<dyn ArtifactStore>;
    async fn create_artifact(&self, session_id: &str, name: &str, content: &[u8])
        -> Result<String>;
    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>>;
    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>>;
    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck>;
}

pub struct DefaultAgentRuntimeHandle {
    builder: AgentRuntimeBuilder,
    runtimes: Arc<Mutex<HashMap<String, Arc<AgentRuntime>>>>,
    artifact_store: Arc<dyn ArtifactStore>,
    event_bus: RuntimeEventBus,
}

impl DefaultAgentRuntimeHandle {
    pub fn new(builder: AgentRuntimeBuilder, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        let event_bus = builder.event_bus.clone();
        Self {
            builder,
            runtimes: Arc::new(Mutex::new(HashMap::new())),
            artifact_store,
            event_bus,
        }
    }
}

#[async_trait::async_trait]
impl AgentRuntimeHandle for DefaultAgentRuntimeHandle {
    async fn spawn_session(
        &self,
        session_id: &str,
        config_override: Option<crate::config::RuntimeConfig>,
    ) -> Result<String> {
        let mut runtimes = self.runtimes.lock().unwrap();
        if runtimes.contains_key(session_id) {
            return Err(RuntimeError::Orchestration(format!(
                "Session already exists: {}",
                session_id
            )));
        }

        let mut session_builder = self.builder.clone();
        if let Some(config) = config_override {
            session_builder = session_builder.config(config);
        }

        let runtime = session_builder.build()?;
        runtimes.insert(session_id.to_string(), Arc::new(runtime));

        self.event_bus.publish(RuntimeEvent::SessionSpawned {
            session_id: session_id.to_string(),
        });

        Ok(session_id.to_string())
    }

    async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult> {
        let runtime = {
            let runtimes = self.runtimes.lock().unwrap();
            runtimes.get(session_id).cloned().ok_or_else(|| {
                RuntimeError::Orchestration(format!("Session not found: {}", session_id))
            })?
        };

        let input = crate::runtime::UserInput {
            prompt: prompt.to_string(),
            session_id: Some(session_id.to_string()),
            cancel_token: gestalt_core::cancel::CancelToken::new(),
            event_tx: None,
            artifact_dir: None,
        };

        let result = runtime.run_prompt(input).await?;
        Ok(result)
    }

    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck> {
        let runtime = {
            let runtimes = self.runtimes.lock().unwrap();
            runtimes.get(session_id).cloned().ok_or_else(|| {
                RuntimeError::Orchestration(format!("Session not found: {}", session_id))
            })?
        };

        runtime
            .enqueue_message(
                session_id.to_string(),
                content.to_string(),
                source,
                idempotency_key,
            )
            .await
    }

    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>> {
        self.event_bus.subscribe()
    }

    fn artifact_store(&self) -> Arc<dyn ArtifactStore> {
        self.artifact_store.clone()
    }

    async fn create_artifact(
        &self,
        session_id: &str,
        name: &str,
        content: &[u8],
    ) -> Result<String> {
        let uri = self
            .artifact_store
            .put_artifact(session_id, name, content)?;
        self.event_bus.publish(RuntimeEvent::ArtifactRouted {
            session_id: session_id.to_string(),
            path: uri.clone(),
            size_bytes: content.len(),
        });
        Ok(uri)
    }

    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>> {
        self.artifact_store.get_artifact(session_id, name)
    }

    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>> {
        self.artifact_store.list_artifacts(session_id)
    }
}
