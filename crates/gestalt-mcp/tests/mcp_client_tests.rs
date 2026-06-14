use gestalt_mcp::{
    McpLifecycleMode, McpServerConfig, McpTransportConfig, McpRegistry,
};
use std::path::PathBuf;

#[tokio::test]
async fn test_mock_mcp_server_integration() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut configs = std::collections::HashMap::new();
    
    let config = McpServerConfig {
        name: "mock-server".to_string(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "cargo".to_string(),
            args: vec!["run", "--package", "gestalt-mcp", "--bin", "mock_mcp_server"].into_iter().map(String::from).collect(),
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
    };
    configs.insert("mock-server".to_string(), config);

    let registry = McpRegistry::new(workspace_root, configs);

    // List tools
    let tools = registry.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    let (server_id, tool_schema) = &tools[0];
    assert_eq!(server_id.0, "mock-server");
    assert_eq!(tool_schema.name, "mock_tool");
    assert_eq!(tool_schema.description, "A mock tool for testing");

    // Call tool
    let result = registry.call_tool(
        "mock-server",
        "mock_tool",
        serde_json::json!({"input": "test-val"})
    ).await.unwrap();

    assert!(!result.is_error);
    assert_eq!(result.content, "Mock tool response");

    // Check status inspector
    let states = registry.get_all_states(5);
    assert_eq!(states.len(), 1);
    let state = &states[0];
    assert_eq!(state.server_id.0, "mock-server");
    assert_eq!(state.connection_state, gestalt_mcp::McpConnectionState::Connected);
    assert_eq!(state.tool_count, 1);
    assert_eq!(state.trust_level.as_deref(), Some("high"));
}
