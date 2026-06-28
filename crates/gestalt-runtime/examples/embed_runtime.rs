use gestalt_core::{
    approval::AutoApprovalProvider,
    event::{AgentEvent, StopReason},
    message::Message,
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{Tool, ToolCatalog, ToolContext, ToolSchema},
};
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};
use serde_json::Value;
use std::sync::Arc;

struct ExampleProvider;

#[async_trait::async_trait]
impl Provider for ExampleProvider {
    fn id(&self) -> &str {
        "example"
    }

    fn display_name(&self) -> &str {
        "Example Provider"
    }

    fn default_model(&self) -> &str {
        "example-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: true,
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
        &CAP
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
        let events = vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }];
        Ok(Box::pin(futures::stream::iter(
            events.into_iter().map(Ok::<_, gestalt_core::HarnessError>),
        )))
    }
}

struct ExampleTool;

#[async_trait::async_trait]
impl Tool for ExampleTool {
    fn name(&self) -> &str {
        "example"
    }

    fn description(&self) -> &str {
        "Example tool"
    }

    fn schema(&self) -> ToolSchema {
        serde_json::json!({
            "name": "example",
            "input_schema": {"type": "object"}
        })
    }

    fn risk(&self, _input: &Value) -> gestalt_core::tool::RiskLevel {
        gestalt_core::tool::RiskLevel::Low
    }

    async fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Result<gestalt_core::tool::ToolOutput, gestalt_core::error::ToolError> {
        Ok(gestalt_core::tool::ToolOutput::Text {
            content: "ok".to_string(),
        })
    }
}

struct ExampleTools;

impl ToolCatalog for ExampleTools {
    fn schemas(&self) -> Vec<ToolSchema> {
        vec![ExampleTool.schema()]
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        (name == "example").then(|| Arc::new(ExampleTool) as Arc<dyn Tool>)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(ExampleProvider))
        .tools(Arc::new(ExampleTools))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "example",
        )))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()?;

    let _ = runtime.inspect();
    Ok(())
}
