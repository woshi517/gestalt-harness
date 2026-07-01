use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::api::v1::RuntimeBackedControlHost;
use gestalt_runtime::api::v1::{
    AgentRuntimeBuilder, ContextMessageAssembler, InMemoryArtifactStore, RuntimeConfig,
};
use gestalt_runtime::api::v1::{
    ContinueSessionRequestV1, InspectRunRequestV1, RunQueryV1, RunStatusV1, SessionControlV1,
    StartSessionRequestV1,
};

struct ExampleProvider;

#[async_trait::async_trait]
impl Provider for ExampleProvider {
    fn id(&self) -> &str {
        "example"
    }

    fn display_name(&self) -> &str {
        "Example"
    }

    fn default_model(&self) -> &str {
        "example-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAPABILITIES: ProviderCapabilities = ProviderCapabilities {
            supports_tools: false,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: true,
            supports_strict_schema: false,
        };
        &CAPABILITIES
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::HarnessError> {
        Ok(0)
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::HarnessError> {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(AgentEvent::Text {
                delta: "hello from the runtime".to_string(),
            }),
            Ok(AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }),
        ])))
    }
}

struct EmptyTools;

impl ToolCatalog for EmptyTools {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::Tool>> {
        None
    }
}

struct AllowPolicy;

#[async_trait::async_trait]
impl PolicyEngine for AllowPolicy {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = RuntimeBackedControlHost::new(
        AgentRuntimeBuilder::new()
            .provider(Arc::new(ExampleProvider))
            .tools(Arc::new(EmptyTools))
            .assembler(Arc::new(ContextMessageAssembler::new("embedding-example")))
            .policy(Arc::new(AllowPolicy))
            .approval(Arc::new(AutoApprovalProvider))
            .config(RuntimeConfig::default()),
        Arc::new(InMemoryArtifactStore::new()),
    )?;
    let started = host
        .start_session(StartSessionRequestV1 {
            session_id: None,
            idempotency_key: None,
            config_override: None,
        })
        .await?;
    host.continue_session(ContinueSessionRequestV1 {
        session_id: started.session_id.clone(),
        run_id: started.run_id.clone(),
        message: "hello".to_string(),
        idempotency_key: None,
    })
    .await?;

    loop {
        let status = host
            .inspect_run(InspectRunRequestV1 {
                session_id: started.session_id.clone(),
                run_id: started.run_id.clone(),
            })
            .await?
            .status;
        if status == RunStatusV1::Completed {
            break;
        }
        tokio::task::yield_now().await;
    }

    Ok(())
}
