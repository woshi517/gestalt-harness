use gestalt_core::{
    approval::AutoApprovalProvider,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    tool::ToolCatalog,
};
use gestalt_mcp::{McpConnectionState, McpLifecycleMode, McpServerConfig, McpTransportConfig};
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig, RuntimeEvent};
use std::sync::Arc;

struct DummyContextPipeline;
impl gestalt_core::context::ContextPipeline for DummyContextPipeline {
    fn process(
        &self,
        _history: &[gestalt_core::SessionMessage],
        _budget: &gestalt_core::context::TokenBudget,
    ) -> Vec<gestalt_core::message::Message> {
        Vec::new()
    }
    fn version(&self) -> &str {
        "dummy"
    }
}

struct DummyProvider;
#[async_trait::async_trait]
impl gestalt_core::provider::Provider for DummyProvider {
    fn id(&self) -> &str {
        "dummy"
    }
    fn display_name(&self) -> &str {
        "Dummy"
    }
    fn default_model(&self) -> &str {
        "dummy"
    }
    fn capabilities(&self) -> &gestalt_core::provider::ProviderCapabilities {
        static CAP: gestalt_core::provider::ProviderCapabilities =
            gestalt_core::provider::ProviderCapabilities {
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
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }
    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[gestalt_core::message::Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }
    async fn stream(
        &self,
        _request: gestalt_core::provider::ProviderRequest,
    ) -> Result<gestalt_core::provider::EventStream, gestalt_core::error::HarnessError> {
        Err(gestalt_core::error::HarnessError::Provider(
            gestalt_core::error::ProviderError::UnknownProvider("dummy".to_string()),
        ))
    }
}

struct DummyToolCatalog;
impl gestalt_core::tool::ToolCatalog for DummyToolCatalog {
    fn schemas(&self) -> Vec<gestalt_core::tool::ToolSchema> {
        Vec::new()
    }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

struct AllowAllPolicyEngine;
#[async_trait::async_trait]
impl PolicyEngine for AllowAllPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

fn build_runtime_config(servers: Vec<(&str, McpLifecycleMode)>) -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    for (name, lifecycle) in servers {
        config.mcp_servers.insert(
            name.to_string(),
            McpServerConfig {
                name: name.to_string(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "cargo".to_string(),
                    args: vec![
                        "run",
                        "--package",
                        "gestalt-mcp",
                        "--bin",
                        "mock_mcp_server",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                    cwd: None,
                    env: std::collections::HashMap::new(),
                },
                lifecycle,
                trust_level: Some("high".to_string()),
                allow_sampling: false,
                env: std::collections::HashMap::new(),
                tool_annotations: std::collections::HashMap::new(),
                timeouts: None,
                display_name: None,
            },
        );
    }
    config
}

#[tokio::test]
async fn test_findings_lazy_servers_stay_disconnected() {
    let config = build_runtime_config(vec![("lazy-server", McpLifecycleMode::Lazy)]);

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(DummyProvider))
        .tools(Arc::new(DummyToolCatalog))
        .middleware(Arc::new(DummyContextPipeline))
        .policy(Arc::new(AllowAllPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    // Verify it is initially Disconnected (lazy lifecycle)
    let states = runtime.mcp_registry.get_all_states(5);
    let state = states
        .iter()
        .find(|s| s.server_id.0 == "lazy-server")
        .unwrap();
    assert_eq!(state.connection_state, McpConnectionState::Disconnected);

    // Call run_prompt to verify prompt execution does NOT eagerly connect lazy servers
    let input = gestalt_runtime::UserInput {
        prompt: "hello".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: None,
        artifact_dir: None,
    };
    let _ = runtime.run_prompt(input).await;

    // Verify it is STILL Disconnected after run_prompt
    let states = runtime.mcp_registry.get_all_states(5);
    let state = states
        .iter()
        .find(|s| s.server_id.0 == "lazy-server")
        .unwrap();
    assert_eq!(
        state.connection_state,
        McpConnectionState::Disconnected,
        "Lazy server was eagerly connected during run_prompt!"
    );

    // Call get_client which initiates connection
    let _client = runtime
        .mcp_registry
        .get_client("lazy-server")
        .await
        .unwrap();

    // Verify it is now Connected
    let states = runtime.mcp_registry.get_all_states(5);
    let state = states
        .iter()
        .find(|s| s.server_id.0 == "lazy-server")
        .unwrap();
    assert_eq!(state.connection_state, McpConnectionState::Connected);
}

#[tokio::test]
async fn test_findings_duplicate_tool_names_scoping() {
    let mut config = build_runtime_config(vec![
        ("server-a", McpLifecycleMode::Lazy),
        ("server-b", McpLifecycleMode::Lazy),
    ]);
    // Set threshold so we are in progressive discovery mode (threshold = 1)
    config.mcp_discovery_threshold = Some(1);

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(DummyProvider))
        .tools(Arc::new(DummyToolCatalog))
        .middleware(Arc::new(DummyContextPipeline))
        .policy(Arc::new(AllowAllPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    // Eagerly connect both so we can populate tools cache
    runtime
        .mcp_registry
        .get_client("server-a")
        .await
        .unwrap()
        .list_tools()
        .await
        .unwrap();
    runtime
        .mcp_registry
        .get_client("server-b")
        .await
        .unwrap()
        .list_tools()
        .await
        .unwrap();

    // Build the raw catalog (without planner) to get unfiltered MCP tools
    let raw_catalog = gestalt_runtime::tool_catalog::ComposedToolCatalog::new(
        Arc::new(DummyToolCatalog),
        std::collections::BTreeMap::new(),
    )
    .unwrap()
    .with_mcp(runtime.mcp_registry.clone());

    // Verify both mock_tool descriptors exist in the raw catalog
    let descriptors = raw_catalog.descriptors();
    let has_a = descriptors
        .iter()
        .any(|d| d.id.to_string() == "mcp:server-a:mock_tool");
    let has_b = descriptors
        .iter()
        .any(|d| d.id.to_string() == "mcp:server-b:mock_tool");
    assert!(has_a);
    assert!(has_b);

    // Build the planner helper
    let planner = gestalt_runtime::tool_catalog_planner::ToolCatalogPlanner::new(
        gestalt_runtime::tool_catalog_planner::ToolProfile::All,
    )
    .with_mcp(
        Some(1),
        runtime.mcp_discovery_state.clone(),
        runtime.mcp_registry.clone(),
    );

    // In discovery mode with threshold = 1, neither should be in final planned descriptors initially (since selected is empty)
    let planned = planner.plan(&raw_catalog);
    let planned_has_a = planned
        .iter()
        .any(|d| d.id.to_string() == "mcp:server-a:mock_tool");
    let planned_has_b = planned
        .iter()
        .any(|d| d.id.to_string() == "mcp:server-b:mock_tool");
    assert!(!planned_has_a);
    assert!(!planned_has_b);

    // Add only "mcp:server-a:mock_tool" to selection list
    {
        let mut state = runtime.mcp_discovery_state.lock().unwrap();
        state
            .selected_tools
            .push("mcp:server-a:mock_tool".to_string());
    }

    // Now, only "mcp:server-a:mock_tool" should be exposed, but NOT "mcp:server-b:mock_tool"
    let planned = planner.plan(&raw_catalog);
    let planned_has_a = planned
        .iter()
        .any(|d| d.id.to_string() == "mcp:server-a:mock_tool");
    let planned_has_b = planned
        .iter()
        .any(|d| d.id.to_string() == "mcp:server-b:mock_tool");
    assert!(planned_has_a);
    assert!(
        !planned_has_b,
        "mcp:server-b:mock_tool was exposed even though only server-a was selected!"
    );
}

#[tokio::test]
async fn test_findings_concurrent_first_use() {
    let config = build_runtime_config(vec![("concurrent-server", McpLifecycleMode::Lazy)]);

    let runtime = Arc::new(
        AgentRuntimeBuilder::new()
            .provider(Arc::new(DummyProvider))
            .tools(Arc::new(DummyToolCatalog))
            .middleware(Arc::new(DummyContextPipeline))
            .policy(Arc::new(AllowAllPolicyEngine))
            .approval(Arc::new(AutoApprovalProvider))
            .config(config)
            .build()
            .unwrap(),
    );

    // Spawn 10 concurrent requests to get the client
    let mut tasks = vec![];
    for _ in 0..10 {
        let runtime = runtime.clone();
        tasks.push(tokio::spawn(async move {
            runtime.mcp_registry.get_client("concurrent-server").await
        }));
    }

    let mut clients = vec![];
    for t in tasks {
        let client = t.await.unwrap().unwrap();
        clients.push(client);
    }

    // Verify all returned Arc<McpClient> point to the exact same memory address (no duplicate spawns)
    let first = &clients[0];
    for c in &clients[1..] {
        assert!(
            Arc::ptr_eq(first, c),
            "Concurrent calls spawned different McpClient instances!"
        );
    }
}

#[tokio::test]
async fn test_findings_failure_reporting_in_discovery() {
    let mut config = RuntimeConfig::default();
    config.mcp_servers.insert(
        "broken-server".to_string(),
        McpServerConfig {
            name: "broken-server".to_string(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "non_existent_command_12345".to_string(),
                args: vec![],
                cwd: None,
                env: std::collections::HashMap::new(),
            },
            lifecycle: McpLifecycleMode::Lazy,
            trust_level: Some("high".to_string()),
            allow_sampling: false,
            env: std::collections::HashMap::new(),
            tool_annotations: std::collections::HashMap::new(),
            timeouts: None,
            display_name: None,
        },
    );

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(DummyProvider))
        .tools(Arc::new(DummyToolCatalog))
        .middleware(Arc::new(DummyContextPipeline))
        .policy(Arc::new(AllowAllPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    // Calling list_all_tools (which is used by search_tools) should fail and return the error
    let res = runtime.mcp_registry.list_all_tools().await;
    assert!(
        res.is_err(),
        "broken server connection succeeded or failed silently!"
    );
}

#[tokio::test]
async fn test_findings_event_emission_and_list_changed() {
    let config = build_runtime_config(vec![("event-server", McpLifecycleMode::Lazy)]);

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(DummyProvider))
        .tools(Arc::new(DummyToolCatalog))
        .middleware(Arc::new(DummyContextPipeline))
        .policy(Arc::new(AllowAllPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    // Subscribe to runtime events
    let mut rx = runtime.event_bus.subscribe();

    // Trigger lazy connection
    let client = runtime
        .mcp_registry
        .get_client("event-server")
        .await
        .unwrap();

    // Verify we received Connecting and Connected events
    let mut events = vec![];
    while let Ok(evt) = rx.try_recv() {
        events.push((*evt).clone());
    }

    let has_connecting = events.iter().any(|e| matches!(e, RuntimeEvent::McpServerConnecting { ref server_name } if server_name == "event-server"));
    let has_connected = events.iter().any(|e| matches!(e, RuntimeEvent::McpServerConnected { ref server_name, .. } if server_name == "event-server"));
    assert!(has_connecting, "Missing McpServerConnecting event");
    assert!(has_connected, "Missing McpServerConnected event");

    // Clear event history
    events.clear();

    // Retrieve tools (which triggers catalog refresh)
    let _tools = client.list_tools().await.unwrap();

    // Verify McpToolCatalogRefreshed is emitted
    while let Ok(evt) = rx.try_recv() {
        events.push((*evt).clone());
    }
    let has_refreshed = events.iter().any(|e| matches!(e, RuntimeEvent::McpToolCatalogRefreshed { ref server_name, .. } if server_name == "event-server"));
    assert!(has_refreshed, "Missing McpToolCatalogRefreshed event");

    // Clear event history
    events.clear();

    // Retrieve tool from catalog (populates backing tool with event_bus)
    let tool = runtime
        .tools
        .get("mcp:event-server:mock_tool")
        .expect("Tool not found in catalog");
    let ctx = gestalt_core::tool::ToolContext {
        working_dir: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        workspace_root: None,
        timeout: std::time::Duration::from_secs(10),
        allow_network: true,
        environment: std::collections::HashMap::new(),
        max_output_bytes: 4096,
        artifact_dir: None,
        current_tool_call_id: None,
        ignore_patterns: Vec::new(),
    };

    // Execute the tool (which triggers tool call start/completed events)
    let _exec_res = tool
        .execute(serde_json::json!({"input": "trigger_list_changed"}), &ctx)
        .await
        .unwrap();

    // Wait a brief moment for background notification processing
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify McpToolCallStarted, McpToolCallCompleted, and McpToolListChanged are all emitted
    while let Ok(evt) = rx.try_recv() {
        events.push((*evt).clone());
    }

    let has_call_started = events.iter().any(|e| matches!(e, RuntimeEvent::McpToolCallStarted { ref server_name, ref tool_name, .. } if server_name == "event-server" && tool_name == "mock_tool"));
    let has_call_completed = events.iter().any(|e| matches!(e, RuntimeEvent::McpToolCallCompleted { ref server_name, ref tool_name, success: true, .. } if server_name == "event-server" && tool_name == "mock_tool"));
    let has_list_changed = events.iter().any(|e| matches!(e, RuntimeEvent::McpToolListChanged { ref server_name } if server_name == "event-server"));

    assert!(has_call_started, "Missing McpToolCallStarted event");
    assert!(has_call_completed, "Missing McpToolCallCompleted event");
    assert!(has_list_changed, "Missing McpToolListChanged event");
}

#[tokio::test]
async fn test_findings_risk_reduction_requires_annotations() {
    let mut config = RuntimeConfig::default();

    // Server A: High trust, but NO tool annotations
    config.mcp_servers.insert(
        "server-high-no-ann".to_string(),
        McpServerConfig {
            name: "server-high-no-ann".to_string(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "cargo".to_string(),
                args: vec![
                    "run",
                    "--package",
                    "gestalt-mcp",
                    "--bin",
                    "mock_mcp_server",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
                cwd: None,
                env: std::collections::HashMap::new(),
            },
            lifecycle: McpLifecycleMode::Lazy,
            trust_level: Some("high".to_string()),
            allow_sampling: false,
            env: std::collections::HashMap::new(),
            tool_annotations: std::collections::HashMap::new(),
            timeouts: None,
            display_name: None,
        },
    );

    // Server B: High trust, WITH read_only tool annotation
    let mut annotations = std::collections::HashMap::new();
    let mut ann_vals = std::collections::HashMap::new();
    ann_vals.insert("read_only".to_string(), "true".to_string());
    annotations.insert("mock_tool".to_string(), ann_vals);

    config.mcp_servers.insert(
        "server-high-with-ann".to_string(),
        McpServerConfig {
            name: "server-high-with-ann".to_string(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "cargo".to_string(),
                args: vec![
                    "run",
                    "--package",
                    "gestalt-mcp",
                    "--bin",
                    "mock_mcp_server",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
                cwd: None,
                env: std::collections::HashMap::new(),
            },
            lifecycle: McpLifecycleMode::Lazy,
            trust_level: Some("high".to_string()),
            allow_sampling: false,
            env: std::collections::HashMap::new(),
            tool_annotations: annotations,
            timeouts: None,
            display_name: None,
        },
    );

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(DummyProvider))
        .tools(Arc::new(DummyToolCatalog))
        .middleware(Arc::new(DummyContextPipeline))
        .policy(Arc::new(AllowAllPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    // Populate caches
    runtime
        .mcp_registry
        .get_client("server-high-no-ann")
        .await
        .unwrap()
        .list_tools()
        .await
        .unwrap();
    runtime
        .mcp_registry
        .get_client("server-high-with-ann")
        .await
        .unwrap()
        .list_tools()
        .await
        .unwrap();

    // Look up mock_tool from server-high-no-ann
    let tool_no_ann = runtime
        .tools
        .get("mcp:server-high-no-ann:mock_tool")
        .unwrap();
    // Risk should be Medium
    assert_eq!(
        tool_no_ann.risk(&serde_json::json!({})),
        gestalt_core::tool::RiskLevel::Medium
    );

    // Look up mock_tool from server-high-with-ann
    let tool_with_ann = runtime
        .tools
        .get("mcp:server-high-with-ann:mock_tool")
        .unwrap();
    // Risk should be Low because of the read_only annotation
    assert_eq!(
        tool_with_ann.risk(&serde_json::json!({})),
        gestalt_core::tool::RiskLevel::Low
    );
}
