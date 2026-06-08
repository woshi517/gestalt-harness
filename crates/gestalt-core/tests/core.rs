use gestalt_core::{
    error::ToolError,
    policy::PolicyRequest,
    provider::{ProviderRequest, ProviderToolSchema},
    session::ExecutionMode,
    tool::{RiskLevel, Tool, ToolContext, ToolOutput, ToolSchema},
    tool_descriptor::{
        AnnotationSource, CanonicalToolId, ProviderToolFormat, ToolAnnotation, ToolAnnotations,
        ToolDescriptor, ToolNamespace,
    },
};
use serde_json::json;
use std::sync::Arc;

struct DummyTool;

#[async_trait::async_trait]
impl Tool for DummyTool {
    fn name(&self) -> &str {
        "dummy_tool"
    }

    fn description(&self) -> &str {
        "A dummy tool for testing"
    }

    fn schema(&self) -> ToolSchema {
        json!({
            "name": "dummy_tool",
            "description": "A dummy tool for testing",
            "input_schema": {
                "type": "object",
                "properties": {
                    "param": { "type": "string" }
                },
                "required": ["param"]
            }
        })
    }

    fn risk(&self, _input: &serde_json::Value) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text {
            content: "ok".to_string(),
        })
    }
}

#[test]
fn test_default_descriptor_derivation() {
    let tool = DummyTool;
    let desc = tool.descriptor();

    assert_eq!(desc.id.name, "dummy_tool");
    assert_eq!(desc.id.namespace, ToolNamespace::BuiltIn);
    assert_eq!(desc.description, "A dummy tool for testing");
    assert_eq!(desc.risk, RiskLevel::Low);
    assert_eq!(desc.response_contract.format, ProviderToolFormat::Text);
}

#[test]
fn test_canonical_id_parsing() {
    let builtin_id: CanonicalToolId = "builtin:read".parse().unwrap();
    assert_eq!(builtin_id.namespace, ToolNamespace::BuiltIn);
    assert_eq!(builtin_id.name, "read");
    assert_eq!(builtin_id.to_string(), "builtin:read");

    let ext_id: CanonicalToolId = "extension:mock-ext:convert_pdf".parse().unwrap();
    assert_eq!(
        ext_id.namespace,
        ToolNamespace::Extension("mock-ext".to_string())
    );
    assert_eq!(ext_id.name, "convert_pdf");
    assert_eq!(ext_id.to_string(), "extension:mock-ext:convert_pdf");

    let mcp_id: CanonicalToolId = "mcp:brave-search:web_search".parse().unwrap();
    assert_eq!(
        mcp_id.namespace,
        ToolNamespace::Mcp("brave-search".to_string())
    );
    assert_eq!(mcp_id.name, "web_search");
    assert_eq!(mcp_id.to_string(), "mcp:brave-search:web_search");

    let invalid = "invalid_format".parse::<CanonicalToolId>();
    assert!(invalid.is_err());
}

#[test]
fn test_descriptor_serialization_stability() {
    let tool = DummyTool;
    let desc = tool.descriptor();
    let serialized1 = serde_json::to_string(&desc).unwrap();
    let serialized2 = serde_json::to_string(&desc).unwrap();
    assert_eq!(serialized1, serialized2);

    let deserialized: ToolDescriptor = serde_json::from_str(&serialized1).unwrap();
    assert_eq!(deserialized.id, desc.id);
    assert_eq!(deserialized.description, desc.description);
}

#[test]
fn test_policy_request_enrichment() {
    let req = PolicyRequest {
        tool_call_id: "call_123".to_string(),
        tool_name: "dummy_tool".to_string(),
        namespace: ToolNamespace::BuiltIn,
        annotations: ToolAnnotations {
            annotations: vec![ToolAnnotation {
                key: "idempotent".to_string(),
                value: "true".to_string(),
                source: AnnotationSource::BuiltInTrusted,
            }],
        },
        input: json!({"param": "val"}),
        risk: RiskLevel::Low,
        mode: ExecutionMode::Confirm,
        working_dir: std::path::PathBuf::from("/"),
        workspace_root: None,
        user_approved: false,
    };

    assert_eq!(req.namespace, ToolNamespace::BuiltIn);
    assert!(req.annotations.get_trusted_bool("idempotent"));
}

#[test]
fn test_provider_request_with_provider_tool_schema() {
    let tool_schema = ProviderToolSchema {
        name: "dummy_tool".to_string(),
        description: "A dummy tool".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "param": { "type": "string" }
            }
        }),
        strict: Some(true),
    };

    let req = ProviderRequest {
        model: "gpt-4".to_string(),
        messages: vec![],
        tools: vec![tool_schema],
        tool_name_map: vec![],
        max_tokens: 100,
        temperature: None,
        top_p: None,
        stop_sequences: vec![],
        metadata: serde_json::Value::Null,
    };

    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.tools[0].name, "dummy_tool");
    assert_eq!(req.tools[0].strict, Some(true));
}

#[test]
fn test_tool_trait_object_descriptor() {
    let tool: Arc<dyn Tool> = Arc::new(DummyTool);
    let desc = tool.descriptor();
    assert_eq!(desc.id.name, "dummy_tool");
}
