use gestalt_runtime::{
    ExtensionManifest, GestaltExtension, ProcessExtension, ProcessExtensionBroker, RuntimeEvent,
    RuntimeEventBus, RuntimeRegistry,
};
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn test_process_extension_lifecycle_and_execution() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = ExtensionManifest::parse(&content).unwrap();

    let event_bus = RuntimeEventBus::new();
    let mut sub = event_bus.subscribe();

    // Spawn broker
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(manifest.clone(), event_bus.clone())
            .await
            .unwrap(),
    );

    // Check spawned events
    let mut events = Vec::new();
    while let Ok(evt) = sub.try_recv() {
        events.push((*evt).clone());
    }
    assert!(events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::ProcessSpawned { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::ExtensionLoaded { .. })));

    // Register with registry
    let mut registry = RuntimeRegistry::new();
    let extension = ProcessExtension::new(manifest.clone(), broker.clone());
    extension.register(&mut registry).unwrap();

    assert!(registry.tools.contains_key("bash_tool"));
    assert!(registry.context_contributors.contains_key("bash_context"));

    // Execute tool
    let tool = registry
        .tools
        .get("bash_tool")
        .unwrap()
        .tool
        .as_ref()
        .unwrap();
    let ctx = gestalt_core::tool::ToolContext {
        working_dir: std::env::current_dir().unwrap(),
        workspace_root: None,
        timeout: std::time::Duration::from_secs(5),
        allow_network: false,
        environment: std::collections::HashMap::new(),
        max_output_bytes: 1024,
        artifact_dir: None,
        current_tool_call_id: None,
    };
    std::env::set_var("TEST_SECRET", "super_secret");
    let output = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
    if let gestalt_core::tool::ToolOutput::Text { content } = output {
        assert!(content.contains("TEST_SECRET=unset"), "TEST_SECRET should be filtered out: {}", content);
        assert!(content.contains("PATH="), "PATH should be present: {}", content);
        assert!(!content.contains("PATH=unset"), "PATH should not be unset");
    } else {
        panic!("Expected text output");
    }

    // Execute context contributor
    let contributor = registry
        .context_contributors
        .get("bash_context")
        .unwrap()
        .contributor
        .clone();
    let msg = contributor
        .contribute(&std::env::current_dir().unwrap())
        .await
        .unwrap();
    if let gestalt_core::message::Message::System { content } = msg {
        assert_eq!(content, "injected context");
    } else {
        panic!("Expected system message");
    }

    // Shutdown broker
    broker.shutdown().await;

    // Check exit event
    let mut events = Vec::new();
    while let Ok(evt) = sub.try_recv() {
        events.push((*evt).clone());
    }
    assert!(events
        .iter()
        .any(|e| matches!(e, RuntimeEvent::ProcessExited { .. })));
}

#[tokio::test]
async fn test_process_extension_host_filesystem_permissions() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = ExtensionManifest::parse(&content).unwrap();

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(manifest.clone(), event_bus.clone())
            .await
            .unwrap(),
    );

    let mut registry = RuntimeRegistry::new();
    let extension = ProcessExtension::new(manifest.clone(), broker.clone());
    extension.register(&mut registry).unwrap();

    let tool = registry
        .tools
        .get("bash_tool")
        .unwrap()
        .tool
        .as_ref()
        .unwrap();

    let ctx = gestalt_core::tool::ToolContext {
        working_dir: std::env::current_dir().unwrap(),
        workspace_root: Some(std::env::current_dir().unwrap()),
        timeout: std::time::Duration::from_secs(5),
        allow_network: false,
        environment: std::collections::HashMap::new(),
        max_output_bytes: 1024,
        artifact_dir: None,
        current_tool_call_id: None,
    };

    // Attempting to write to a disallowed path should fail with PathNotAllowed
    let tool_input = serde_json::json!({
        "target_file": "/tmp/forbidden_test_file.txt"
    });
    let result = tool.execute(tool_input, &ctx).await;
    assert!(result.is_err());
    let err_str = format!("{:?}", result.err().unwrap());
    assert!(err_str.contains("PathNotAllowed"), "Expected PathNotAllowed error, got: {}", err_str);

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_host_network_permissions() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = ExtensionManifest::parse(&content).unwrap();

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(manifest.clone(), event_bus.clone())
            .await
            .unwrap(),
    );

    let mut registry = RuntimeRegistry::new();
    let extension = ProcessExtension::new(manifest.clone(), broker.clone());
    extension.register(&mut registry).unwrap();

    let tool = registry
        .tools
        .get("bash_tool")
        .unwrap()
        .tool
        .as_ref()
        .unwrap();

    let ctx = gestalt_core::tool::ToolContext {
        working_dir: std::env::current_dir().unwrap(),
        workspace_root: Some(std::env::current_dir().unwrap()),
        timeout: std::time::Duration::from_secs(5),
        allow_network: false,
        environment: std::collections::HashMap::new(),
        max_output_bytes: 1024,
        artifact_dir: None,
        current_tool_call_id: None,
    };

    // Attempting to contact google.com when allow_network is empty should fail with NetworkDenied
    let tool_input = serde_json::json!({
        "url": "http://google.com"
    });
    let result = tool.execute(tool_input, &ctx).await;
    assert!(result.is_err());
    let err_str = format!("{:?}", result.err().unwrap());
    assert!(err_str.contains("NetworkDenied"), "Expected NetworkDenied error, got: {}", err_str);

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_hooks_dispatch() {
    use gestalt_runtime::{BeforeContextBuildCtx, HookOutcome, CompositionHooks};
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let mut manifest = ExtensionManifest::parse(&content).unwrap();
    
    // Enable hooks capability and register a hook
    manifest.capabilities.hooks = true;
    manifest.hooks = vec![gestalt_runtime::manifest::HookDeclaration {
        name: "mock_hook".to_string(),
        lifecycle_point: "before_context_build".to_string(),
    }];

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(manifest.clone(), event_bus.clone())
            .await
            .unwrap(),
    );

    let extension = Arc::new(ProcessExtension::new(manifest.clone(), broker.clone()));
    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![extension as Arc<dyn GestaltExtension>],
    };

    let ctx = BeforeContextBuildCtx {
        session_id: "test-session-id".to_string(),
        history: vec![],
    };

    let result = composed.before_context_build(&ctx).await.unwrap();
    match result {
        HookOutcome::Block { reason } => {
            assert_eq!(reason, "blocked by mock extension hook");
        }
        _ => panic!("Expected HookOutcome::Block, got: {:?}", result),
    }

    broker.shutdown().await;
}
