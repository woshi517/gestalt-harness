use gestalt_core::{
    approval::AutoApprovalProvider,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    session::Session,
    tool_descriptor::ToolNamespace,
    agent::executor::ToolExecutor,
};
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};
use gestalt_mcp::{McpLifecycleMode, McpServerConfig, McpTransportConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct AssertingPolicyEngine {
    called: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl PolicyEngine for AssertingPolicyEngine {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        println!("=== AssertingPolicyEngine evaluating request: tool_name='{}', namespace='{:?}'", request.tool_name, request.namespace);
        if request.tool_name == "mcp:mock-server:mock_tool" {
            assert_eq!(request.namespace, ToolNamespace::Mcp("mock-server".to_string()));
            self.called.store(true, Ordering::SeqCst);
        }
        PolicyDecision::allowed(None)
    }
}

struct DummyContextPipeline;
impl gestalt_core::context::ContextPipeline for DummyContextPipeline {
    fn process(&self, _history: &[gestalt_core::message::Message], _budget: &gestalt_core::context::TokenBudget) -> Vec<gestalt_core::message::Message> {
        Vec::new()
    }
    fn version(&self) -> &str {
        "dummy"
    }
}

struct DummyProvider;
#[async_trait::async_trait]
impl gestalt_core::provider::Provider for DummyProvider {
    fn id(&self) -> &str { "dummy" }
    fn display_name(&self) -> &str { "Dummy" }
    fn default_model(&self) -> &str { "dummy" }
    fn capabilities(&self) -> &gestalt_core::provider::ProviderCapabilities {
        static CAP: gestalt_core::provider::ProviderCapabilities = gestalt_core::provider::ProviderCapabilities {
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: false,
            supports_strict_schema: false,
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> { None }
    fn count_tokens(&self, _model: &str, _messages: &[gestalt_core::message::Message]) -> Result<usize, gestalt_core::error::HarnessError> { Ok(0) }
    async fn stream(&self, _request: gestalt_core::provider::ProviderRequest) -> Result<gestalt_core::provider::EventStream, gestalt_core::error::HarnessError> {
        Err(gestalt_core::error::HarnessError::Provider(gestalt_core::error::ProviderError::UnknownProvider("dummy".to_string())))
    }
}

struct DummyToolCatalog;
impl gestalt_core::tool::ToolCatalog for DummyToolCatalog {
    fn schemas(&self) -> Vec<gestalt_core::tool::ToolSchema> { Vec::new() }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> { None }
}

#[tokio::test]
async fn test_runtime_mcp_policy_check_and_execution() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut config = RuntimeConfig::default();
    config.mcp_servers.insert("mock-server".to_string(), McpServerConfig {
        name: "mock-server".to_string(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "cargo".to_string(),
            args: vec!["run", "--package", "gestalt-mcp", "--bin", "mock_mcp_server"].into_iter().map(String::from).collect(),
            cwd: None,
            env: std::collections::HashMap::new(),
        },
        lifecycle: McpLifecycleMode::Lazy,
        trust_level: Some("medium".to_string()),
        allow_sampling: false,
        env: std::collections::HashMap::new(),
        tool_annotations: std::collections::HashMap::new(),
        timeouts: None,
        display_name: None,
    });

    let called = Arc::new(AtomicBool::new(false));
    let policy_engine = AssertingPolicyEngine { called: called.clone() };

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(DummyProvider))
        .tools(Arc::new(DummyToolCatalog))
        .middleware(Arc::new(DummyContextPipeline))
        .policy(Arc::new(policy_engine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    // Eagerly connect to populate tools cache for testing
    runtime.mcp_registry.list_all_tools().await.unwrap();

    // Verify tool name mappings resolve
    let tools_list: Vec<(gestalt_core::tool_descriptor::CanonicalToolId, String, String)> = runtime.tools.descriptors().iter().map(|desc| {
        let display_name = desc.id.name.clone();
        let descriptor_hash = "hash".to_string();
        (desc.id.clone(), display_name, descriptor_hash)
    }).collect();

    let tool_name_mappings = gestalt_core::tool_name_mapping::ToolNameMapping::build_mapping_with_resolution(&tools_list);

    let mapping = tool_name_mappings.iter().find(|m| {
        match &m.internal_id.namespace {
            ToolNamespace::Mcp(server) => {
                server == "mock-server" && m.internal_id.name == "mock_tool"
            }
            _ => false,
        }
    }).expect("mock_tool mapping found");

    let proposed = gestalt_core::turn::ProposedToolCall {
        id: "call_123".to_string(),
        name: mapping.provider_name.clone(),
        input: serde_json::json!({"input": "hello"}),
    };

    let executor = ToolExecutor::new(
        runtime.tools.clone(),
        runtime.policy.clone(),
        runtime.approval.clone(),
    );

    let mut session_grants = Vec::new();
    let session = Session::new(
        "session_123",
        gestalt_core::session::SessionConfig {
            model: "test-model".to_string(),
            provider: "test-provider".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 10,
        },
        gestalt_core::context::TokenBudget {
            model_limit: 1000,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 1,
        },
        gestalt_core::tool::ToolContext {
            working_dir: workspace_root.clone(),
            workspace_root: Some(workspace_root.clone()),
            timeout: std::time::Duration::from_secs(10),
            allow_network: true,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 4096,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        gestalt_core::session::ExecutionMode::Confirm,
        gestalt_core::snapshot::WorkspaceSnapshot {
            workspace_root: workspace_root.clone(),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "dummy-hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );
    let cancel_token = gestalt_core::cancel::CancelToken::new();
    
    let results = executor.execute_tool_batch(
        &session,
        vec![proposed],
        &tool_name_mappings,
        &mut |_event| Ok(()),
        &mut session_grants,
        1,
        10,
        &gestalt_core::HookRegistry::new(),
        &cancel_token,
        None,
    ).await.unwrap();

    assert_eq!(results.len(), 1);
    let (_turn, _call_id, exec_result, _duration_ms, _error_msg) = &results[0];
    
    assert_eq!(exec_result.is_error, false);
    assert_eq!(exec_result.content, "Mock tool response");

    assert!(called.load(Ordering::SeqCst), "AssertingPolicyEngine was not called for mock_tool");
}
