use gestalt_core::{
    context::{
        ContextPacket, ContextStability, PromptAssemblyStrategy, PromptCachePlan, PromptSegment,
        PromptSegmentKind, PromptSnapshot,
    },
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
        cache_plan: None,
        metadata: serde_json::Value::Null,
        reasoning_effort: None,
        text_verbosity: None,
    };

    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.tools[0].name, "dummy_tool");
    assert_eq!(req.tools[0].strict, Some(true));
}

#[test]
fn test_prompt_snapshot_hash_is_deterministic() {
    let messages = vec![gestalt_core::Message::System {
        content: "stable prefix".to_string(),
    }];

    let first = PromptSnapshot::new(messages.clone(), 3);
    let second = PromptSnapshot::new(messages, 3);

    assert_eq!(first.snapshot_hash, second.snapshot_hash);
    assert_eq!(first.prefix_hash, second.prefix_hash);
    assert_eq!(first.message_hashes, second.message_hashes);
}

#[test]
fn test_prompt_cache_plan_round_trips_through_serde() {
    let snapshot = PromptSnapshot::new(
        vec![gestalt_core::Message::System {
            content: "stable prefix".to_string(),
        }],
        7,
    );
    let segment = PromptSegment::from_messages(
        PromptSegmentKind::Snapshot,
        ContextStability::SessionStatic,
        &snapshot.messages,
    );
    let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot)
        .with_segments(vec![segment]);

    let serialized = serde_json::to_value(&plan).unwrap();
    let round_tripped: PromptCachePlan = serde_json::from_value(serialized).unwrap();

    assert_eq!(plan, round_tripped);
}

#[test]
fn test_context_packet_serializes_cache_metadata() {
    let snapshot = PromptSnapshot::new(
        vec![gestalt_core::Message::System {
            content: "stable prefix".to_string(),
        }],
        11,
    );
    let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot);
    let packet = ContextPacket {
        messages: snapshot.messages.clone(),
        packet_hash: "packet-hash".to_string(),
        pipeline_version: "pipeline-v1".to_string(),
        tokenizer_id: "default".to_string(),
        token_estimate: 42,
        sources: vec![],
        omissions: vec![],
        message_hashes: snapshot.message_hashes.clone(),
        prompt_assembly_strategy: PromptAssemblyStrategy::Snapshot,
        snapshot_hash: Some(snapshot.snapshot_hash.clone()),
        cache_prefix_hash: Some(snapshot.prefix_hash.clone()),
        segments: vec![PromptSegment::from_messages(
            PromptSegmentKind::Snapshot,
            ContextStability::SessionStatic,
            &snapshot.messages,
        )],
        cache_plan: Some(plan),
        prompt_source: Some("default".to_string()),
    };

    let serialized = serde_json::to_value(&packet).unwrap();
    let round_tripped: ContextPacket = serde_json::from_value(serialized).unwrap();

    assert_eq!(packet, round_tripped);
}

#[test]
fn test_cache_events_round_trip_through_serde() {
    let events = vec![
        gestalt_core::AgentEvent::PromptSnapshotCreated {
            snapshot_hash: "snapshot-hash".to_string(),
            prefix_hash: "prefix-hash".to_string(),
            created_turn: 4,
        },
        gestalt_core::AgentEvent::PromptSnapshotLoaded {
            snapshot_hash: "snapshot-hash".to_string(),
            source: "resume".to_string(),
        },
        gestalt_core::AgentEvent::PromptSnapshotReused {
            snapshot_hash: "snapshot-hash".to_string(),
            prefix_hash: "prefix-hash".to_string(),
        },
        gestalt_core::AgentEvent::PromptCachePlanGenerated {
            snapshot_hash: "snapshot-hash".to_string(),
            prefix_hash: "prefix-hash".to_string(),
            prefix_message_count: 3,
        },
        gestalt_core::AgentEvent::EphemeralContextInjected {
            source: "budget_exhaustion".to_string(),
            token_estimate: 17,
        },
    ];

    for event in events {
        let serialized = serde_json::to_value(&event).unwrap();
        let round_tripped: gestalt_core::AgentEvent = serde_json::from_value(serialized).unwrap();
        assert_eq!(event, round_tripped);
    }
}

#[test]
fn test_tool_trait_object_descriptor() {
    let tool: Arc<dyn Tool> = Arc::new(DummyTool);
    let desc = tool.descriptor();
    assert_eq!(desc.id.name, "dummy_tool");
}

use gestalt_core::{ApiFormat, ResolvedModelSnapshot, ModelSelection, ModelCapabilities, PromptCacheMode};

#[test]
fn api_format_uses_snake_case_wire_names() {
    assert_eq!(
        serde_json::to_value(ApiFormat::OpenAiResponses).unwrap(),
        serde_json::json!("openai_responses")
    );
}

#[test]
fn resolved_model_snapshot_round_trips() {
    let snapshot = ResolvedModelSnapshot {
        selection: ModelSelection {
            provider_id: "openai".into(),
            model_id: "gpt-5.1".into(),
            variant: Some("high".into()),
        },
        api_format: ApiFormat::OpenAiResponses,
        display_name: Some("GPT-5.1".into()),
        max_context_tokens: 400_000,
        max_output_tokens: 32_768,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            reasoning: true,
            prompt_cache: PromptCacheMode::Automatic,
        },
    };

    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(
        serde_json::from_value::<ResolvedModelSnapshot>(value).unwrap(),
        snapshot
    );
}

