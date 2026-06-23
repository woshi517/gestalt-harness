use gestalt_core::ContextStability;
use gestalt_runtime::{
    Capabilities, Entrypoint, ExtensionManifest, GestaltExtension, Permissions, ProcessExtension,
    ProcessExtensionBroker, RuntimeEvent, RuntimeEventBus, RuntimeRegistry,
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
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
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
    assert_eq!(
        registry
            .context_contributors
            .get("bash_context")
            .unwrap()
            .stability,
        ContextStability::TurnDynamic
    );

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
        ignore_patterns: Vec::new(),
    };
    std::env::set_var("TEST_SECRET", "super_secret");
    let output = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
    if let gestalt_core::tool::ToolOutput::Text { content } = output {
        assert!(
            content.contains("TEST_SECRET=unset"),
            "TEST_SECRET should be filtered out: {}",
            content
        );
        assert!(
            content.contains("PATH="),
            "PATH should be present: {}",
            content
        );
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
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
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
        ignore_patterns: Vec::new(),
    };

    // Attempting to write to a disallowed path should fail with PathNotAllowed
    let tool_input = serde_json::json!({
        "target_file": "/tmp/forbidden_test_file.txt"
    });
    let result = tool.execute(tool_input, &ctx).await;
    assert!(result.is_err());
    let err_str = format!("{:?}", result.err().unwrap());
    assert!(
        err_str.contains("PathNotAllowed"),
        "Expected PathNotAllowed error, got: {}",
        err_str
    );

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
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
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
        ignore_patterns: Vec::new(),
    };

    // Attempting to contact google.com when allow_network is empty should fail with NetworkDenied
    let tool_input = serde_json::json!({
        "url": "http://google.com"
    });
    let result = tool.execute(tool_input, &ctx).await;
    assert!(result.is_err());
    let err_str = format!("{:?}", result.err().unwrap());
    assert!(
        err_str.contains("NetworkDenied"),
        "Expected NetworkDenied error, got: {}",
        err_str
    );

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_rejects_shell_bypass_in_args() {
    let manifest = ExtensionManifest {
        id: "shell-bypass".to_string(),
        name: "Shell Bypass".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: None,
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "env".to_string(),
            args: vec!["bash".to_string(), "script.sh".to_string()],
        },
        capabilities: Capabilities {
            tools: false,
            hooks: false,
            context: false,
            ..Default::default()
        },
        permissions: Permissions {
            allow_network: vec![],
            allow_workspace_read: false,
            allow_workspace_write: false,
            allow_shell: false,
            allow_all_paths: false,
            allowed_paths: vec![],
        },
        tools: vec![],
        hooks: vec![],
        context_injectors: vec![],
    };

    let event_bus = RuntimeEventBus::new();
    let result = ProcessExtensionBroker::spawn(
        manifest,
        event_bus,
        Default::default(),
        Default::default(),
        false,
    )
    .await;
    assert!(
        result.is_err(),
        "shell-like entrypoint args should be rejected"
    );
}

#[tokio::test]
async fn test_process_extension_hooks_dispatch() {
    use gestalt_runtime::{BeforeContextBuildCtx, CompositionHooks, HookOutcome};
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let mut manifest = ExtensionManifest::parse(&content).unwrap();

    // Enable hooks capability and register a hook
    manifest.capabilities.hooks = true;
    manifest.hooks = vec![gestalt_runtime::manifest::HookDeclaration {
        name: "mock_hook".to_string(),
        lifecycle_point: "before_context_build".to_string(),
        failure_mode: None,
        timeout_ms: None,
    }];

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
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
        artifact_dir: None,
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

#[tokio::test]
async fn test_process_extension_prepare_next_turn_dispatch() {
    use gestalt_runtime::{CompositionHooks, HookOutcome, PrepareNextTurnCtx};
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let mut manifest = ExtensionManifest::parse(&content).unwrap();

    manifest.capabilities.hooks = true;
    manifest.hooks = vec![gestalt_runtime::manifest::HookDeclaration {
        name: "mock_hook".to_string(),
        lifecycle_point: "prepare_next_turn".to_string(),
        failure_mode: None,
        timeout_ms: None,
    }];

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );

    let extension = Arc::new(ProcessExtension::new(manifest.clone(), broker.clone()));
    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![extension as Arc<dyn GestaltExtension>],
    };

    let ctx = PrepareNextTurnCtx {
        session_id: "test-session-id".to_string(),
        history: vec![],
        turn_index: 0,
        current_model: "mock-model".to_string(),
        current_provider: "mock".to_string(),
    };

    let result = composed.prepare_next_turn(&ctx).await.unwrap();
    match result {
        HookOutcome::Block { reason } => {
            assert_eq!(reason, "blocked by mock extension hook");
        }
        _ => panic!("Expected HookOutcome::Block, got: {:?}", result),
    }

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_prepare_next_turn_switch_model_dispatch() {
    use gestalt_runtime::{CompositionHooks, HookOutcome, PrepareNextTurnCtx};
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-switch-model-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let mut manifest = ExtensionManifest::parse(&content).unwrap();

    manifest.capabilities.hooks = true;
    manifest.hooks = vec![gestalt_runtime::manifest::HookDeclaration {
        name: "mock_switch_model_hook".to_string(),
        lifecycle_point: "prepare_next_turn".to_string(),
        failure_mode: None,
        timeout_ms: None,
    }];

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );

    let extension = Arc::new(ProcessExtension::new(manifest.clone(), broker.clone()));
    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![extension as Arc<dyn GestaltExtension>],
    };

    let ctx = PrepareNextTurnCtx {
        session_id: "test-session-id".to_string(),
        history: vec![],
        turn_index: 1,
        current_model: "mock-model".to_string(),
        current_provider: "mock".to_string(),
    };

    let result = composed.prepare_next_turn(&ctx).await.unwrap();
    match result {
        HookOutcome::SwitchModel {
            model, provider, ..
        } => {
            assert_eq!(model, "cheaper-model");
            assert_eq!(provider.as_deref(), Some("mock"));
        }
        other => panic!("Expected HookOutcome::SwitchModel, got: {:?}", other),
    }

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_limits_max_message_bytes() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = ExtensionManifest::parse(&content).unwrap();

    let event_bus = RuntimeEventBus::new();
    let mut sub = event_bus.subscribe();

    // Set max_message_bytes to 150 bytes (larger than initialize response (~91 bytes) but too small for tools/call response (~728 bytes))
    let limits = gestalt_runtime::config::ExtensionLimitsConfig {
        max_message_bytes: Some(150),
        max_pending_requests: None,
        max_protocol_errors: None,
    };

    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            limits,
            true,
        )
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
        workspace_root: None,
        timeout: std::time::Duration::from_secs(5),
        allow_network: false,
        environment: std::collections::HashMap::new(),
        max_output_bytes: 1024,
        artifact_dir: None,
        current_tool_call_id: None,
        ignore_patterns: Vec::new(),
    };

    let result = tool.execute(serde_json::json!({}), &ctx).await;
    assert!(result.is_err());

    // Verify event bus received ExtensionError containing "Message size limit exceeded"
    let mut found_error = false;
    while let Ok(evt) = sub.try_recv() {
        if let RuntimeEvent::ExtensionError { message, .. } = &*evt {
            if message.contains("Message size limit exceeded") {
                found_error = true;
            }
        }
    }
    assert!(
        found_error,
        "Expected to find size limit error in event bus"
    );

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_limits_max_pending_requests() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = ExtensionManifest::parse(&content).unwrap();

    let event_bus = RuntimeEventBus::new();

    // Set max_pending_requests to 1
    let limits = gestalt_runtime::config::ExtensionLimitsConfig {
        max_message_bytes: None,
        max_pending_requests: Some(1),
        max_protocol_errors: None,
    };

    let timeouts = gestalt_runtime::config::ExtensionTimeoutsConfig {
        initialize_ms: Some(5000),
        hook_ms: Some(1000),
        context_ms: Some(5000),
        tool_ms: Some(5000),
        shutdown_ms: Some(1000),
    };

    let broker = Arc::new(
        ProcessExtensionBroker::spawn(manifest.clone(), event_bus.clone(), timeouts, limits, true)
            .await
            .unwrap(),
    );

    // Call unsupported methods to keep requests pending
    let call_fut1 = broker.call("unsupported_method_hang", None);
    let call_fut2 = broker.call("unsupported_method_hang_2", None);

    let (res1, res2) = tokio::join!(call_fut1, call_fut2);

    let first_timeout = res1.is_err() && res1.as_ref().err().unwrap() == "Request timed out";
    let second_timeout = res2.is_err() && res2.as_ref().err().unwrap() == "Request timed out";
    let first_rejected =
        res1.is_err() && res1.as_ref().err().unwrap() == "Too many pending requests";
    let second_rejected =
        res2.is_err() && res2.as_ref().err().unwrap() == "Too many pending requests";

    assert!(
        (first_timeout && second_rejected) || (second_timeout && first_rejected),
        "Expected one timeout and one immediate rejection, got: res1={:?}, res2={:?}",
        res1,
        res2
    );

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_context_trust_downgrade() {
    let manifest = ExtensionManifest {
        id: "trust-downgrade-ext".to_string(),
        name: "Trust Downgrade Test".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then
    req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2)
  fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "context/inject" ]; then
    echo '{"jsonrpc":"2.0","result":{"items":[{"content":"item1","trust":"trusted","priority":"critical"}]},"id":"'"$req_id"'"}'
  fi
done"#.to_string(),
            ],
        },
        capabilities: Capabilities {
            context: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![],
        context_injectors: vec![
            gestalt_runtime::manifest::ContextInjectorDeclaration {
                name: "test_context".to_string(),
                stability: Some(ContextStability::TurnDynamic),
            }
        ],
    };

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            false,
        )
        .await
        .unwrap(),
    );

    let mut registry = RuntimeRegistry::new();
    let extension = ProcessExtension::new(manifest.clone(), broker.clone());
    extension.register(&mut registry).unwrap();

    let contributor = registry
        .context_contributors
        .get("test_context")
        .unwrap()
        .contributor
        .clone();

    let msg = contributor
        .contribute(&std::env::current_dir().unwrap())
        .await
        .unwrap();

    if let gestalt_core::message::Message::System { content } = msg {
        assert_eq!(content, "item1");
    } else {
        panic!("Expected system message with combined content");
    }

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_hook_aggregation() {
    use gestalt_runtime::{BeforeContextBuildCtx, CompositionHooks, HookOutcome};

    let manifest1 = ExtensionManifest {
        id: "ext-1".to_string(),
        name: "Extension 1".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "hooks/call" ]; then
    echo '{"jsonrpc":"2.0","result":{"type":"add_context","message":{"role":"system","content":"context-from-ext1"}},"id":"'"$req_id"'"}'
  fi
done"#.to_string(),
            ],
        },
        capabilities: Capabilities {
            hooks: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![
            gestalt_runtime::manifest::HookDeclaration {
                name: "hook_1".to_string(),
                lifecycle_point: "before_context_build".to_string(),
                failure_mode: None,
                timeout_ms: None,
            }
        ],
        context_injectors: vec![],
    };

    let manifest2 = ExtensionManifest {
        id: "ext-2".to_string(),
        name: "Extension 2".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "hooks/call" ]; then
    echo '{"jsonrpc":"2.0","result":{"type":"add_context","message":{"role":"system","content":"context-from-ext2"}},"id":"'"$req_id"'"}'
  fi
done"#.to_string(),
            ],
        },
        capabilities: Capabilities {
            hooks: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![
            gestalt_runtime::manifest::HookDeclaration {
                name: "hook_2".to_string(),
                lifecycle_point: "before_context_build".to_string(),
                failure_mode: None,
                timeout_ms: None,
            }
        ],
        context_injectors: vec![],
    };

    let event_bus = RuntimeEventBus::new();

    let broker1 = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest1.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext1 =
        Arc::new(ProcessExtension::new(manifest1, broker1.clone())) as Arc<dyn GestaltExtension>;

    let broker2 = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest2.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext2 =
        Arc::new(ProcessExtension::new(manifest2, broker2.clone())) as Arc<dyn GestaltExtension>;

    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![ext1, ext2],
    };

    let ctx = BeforeContextBuildCtx {
        session_id: "test-session".to_string(),
        history: vec![],
        artifact_dir: None,
    };

    let result = composed.before_context_build(&ctx).await.unwrap();

    if let HookOutcome::Aggregated(outcomes) = result {
        assert_eq!(outcomes.len(), 2);

        if let HookOutcome::AddContext {
            message: gestalt_core::message::Message::System { content },
        } = &outcomes[0]
        {
            assert_eq!(content, "context-from-ext1");
        } else {
            panic!("Expected HookOutcome::AddContext, got {:?}", outcomes[0]);
        }

        if let HookOutcome::AddContext {
            message: gestalt_core::message::Message::System { content },
        } = &outcomes[1]
        {
            assert_eq!(content, "context-from-ext2");
        } else {
            panic!("Expected HookOutcome::AddContext, got {:?}", outcomes[1]);
        }
    } else {
        panic!("Expected HookOutcome::Aggregated, got {:?}", result);
    }

    broker1.shutdown().await;
    broker2.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_hook_namespaced_annotation() {
    use gestalt_runtime::{BeforeContextBuildCtx, CompositionHooks, HookOutcome};

    let manifest = ExtensionManifest {
        id: "annotation-ext".to_string(),
        name: "Annotation Extension".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "hooks/call" ]; then
    echo '{"jsonrpc":"2.0","result":{"type":"annotate","metadata":{"foo":"bar"}},"id":"'"$req_id"'"}'
  fi
done"#.to_string(),
            ],
        },
        capabilities: Capabilities {
            hooks: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![
            gestalt_runtime::manifest::HookDeclaration {
                name: "annotation_hook".to_string(),
                lifecycle_point: "before_context_build".to_string(),
                failure_mode: None,
                timeout_ms: None,
            }
        ],
        context_injectors: vec![],
    };

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext =
        Arc::new(ProcessExtension::new(manifest, broker.clone())) as Arc<dyn GestaltExtension>;

    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![ext],
    };

    let ctx = BeforeContextBuildCtx {
        session_id: "test-session".to_string(),
        history: vec![],
        artifact_dir: None,
    };

    let result = composed.before_context_build(&ctx).await.unwrap();

    if let HookOutcome::Annotate { metadata } = result {
        assert_eq!(
            metadata,
            serde_json::json!({ "annotation-ext": { "foo": "bar" } })
        );
    } else {
        panic!("Expected HookOutcome::Annotate, got {:?}", result);
    }

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_hook_model_switch_conflict() {
    use gestalt_runtime::{CompositionHooks, PrepareNextTurnCtx};

    let manifest1 = ExtensionManifest {
        id: "switch-ext-1".to_string(),
        name: "Switch Extension 1".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "hooks/call" ]; then
    echo '{"jsonrpc":"2.0","result":{"type":"switch_model","model":"model-a","provider":"prov-a"},"id":"'"$req_id"'"}'
  fi
done"#.to_string(),
            ],
        },
        capabilities: Capabilities {
            hooks: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![
            gestalt_runtime::manifest::HookDeclaration {
                name: "hook_1".to_string(),
                lifecycle_point: "prepare_next_turn".to_string(),
                failure_mode: None,
                timeout_ms: None,
            }
        ],
        context_injectors: vec![],
    };

    let manifest2 = ExtensionManifest {
        id: "switch-ext-2".to_string(),
        name: "Switch Extension 2".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "hooks/call" ]; then
    echo '{"jsonrpc":"2.0","result":{"type":"switch_model","model":"model-b","provider":"prov-b"},"id":"'"$req_id"'"}'
  fi
done"#.to_string(),
            ],
        },
        capabilities: Capabilities {
            hooks: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![
            gestalt_runtime::manifest::HookDeclaration {
                name: "hook_2".to_string(),
                lifecycle_point: "prepare_next_turn".to_string(),
                failure_mode: None,
                timeout_ms: None,
            }
        ],
        context_injectors: vec![],
    };

    let event_bus = RuntimeEventBus::new();
    let mut sub = event_bus.subscribe();

    let broker1 = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest1.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext1 =
        Arc::new(ProcessExtension::new(manifest1, broker1.clone())) as Arc<dyn GestaltExtension>;

    let broker2 = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest2.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext2 =
        Arc::new(ProcessExtension::new(manifest2, broker2.clone())) as Arc<dyn GestaltExtension>;

    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![ext1, ext2],
    };

    let ctx = PrepareNextTurnCtx {
        session_id: "test-session".to_string(),
        history: vec![],
        turn_index: 0,
        current_model: "base-model".to_string(),
        current_provider: "base-provider".to_string(),
    };

    let _result = composed.prepare_next_turn(&ctx).await.unwrap();

    let mut events = Vec::new();
    while let Ok(evt) = sub.try_recv() {
        events.push((*evt).clone());
    }

    let has_conflict = events.iter().any(|e| {
        if let RuntimeEvent::RuntimeError { message } = e {
            message.contains("Conflict: SwitchModel requested by extension")
                && message.contains("conflicts with previous override")
        } else {
            false
        }
    });
    assert!(
        has_conflict,
        "Expected a conflict RuntimeError event, observed events: {:?}",
        events
    );

    broker1.shutdown().await;
    broker2.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_hook_failure_mode_policies() {
    use gestalt_runtime::{
        BeforeContextBuildCtx, BeforeToolPolicyCtx, CompositionHooks, HookOutcome,
    };

    let manifest = ExtensionManifest {
        id: "failing-ext".to_string(),
        name: "Failing Extension".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  elif [ "$method" = "hooks/call" ]; then
    exit 1
  fi
done"#
                    .to_string(),
            ],
        },
        capabilities: Capabilities {
            hooks: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![
            gestalt_runtime::manifest::HookDeclaration {
                name: "hook_policy".to_string(),
                lifecycle_point: "before_tool_policy".to_string(),
                failure_mode: Some("closed".to_string()),
                timeout_ms: Some(100),
            },
            gestalt_runtime::manifest::HookDeclaration {
                name: "hook_context".to_string(),
                lifecycle_point: "before_context_build".to_string(),
                failure_mode: Some("open".to_string()),
                timeout_ms: Some(100),
            },
        ],
        context_injectors: vec![],
    };

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext =
        Arc::new(ProcessExtension::new(manifest, broker.clone())) as Arc<dyn GestaltExtension>;

    let composed = gestalt_runtime::composition_hooks::ComposedCompositionHooks {
        user_hooks: None,
        extensions: vec![ext],
    };

    let ctx_context = BeforeContextBuildCtx {
        session_id: "test-session".to_string(),
        history: vec![],
        artifact_dir: None,
    };
    let result_context = composed.before_context_build(&ctx_context).await.unwrap();
    assert_eq!(result_context, HookOutcome::Continue);

    let ctx_policy = BeforeToolPolicyCtx {
        session_id: "test-session".to_string(),
        tool_name: "test-tool".to_string(),
        tool_input: serde_json::Value::Null,
    };
    let result_policy = composed.before_tool_policy(&ctx_policy).await.unwrap();
    if let HookOutcome::Block { reason } = result_policy {
        assert!(reason.contains("Hook 'hook_policy' failed"));
    } else {
        panic!("Expected HookOutcome::Block, got {:?}", result_policy);
    }

    broker.shutdown().await;
}

#[tokio::test]
async fn test_process_extension_negotiated_protocol_fingerprint() {
    use gestalt_runtime::{AgentRuntime, RuntimeConfig};

    struct FPProvider;
    #[async_trait::async_trait]
    impl gestalt_core::provider::Provider for FPProvider {
        fn id(&self) -> &str {
            "fp"
        }
        fn display_name(&self) -> &str {
            "FP"
        }
        fn default_model(&self) -> &str {
            "model"
        }
        fn capabilities(&self) -> &gestalt_core::provider::ProviderCapabilities {
            static CAP: gestalt_core::provider::ProviderCapabilities =
                gestalt_core::provider::ProviderCapabilities {
                    supports_tools: false,
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
        ) -> Result<gestalt_core::provider::EventStream, gestalt_core::error::HarnessError>
        {
            Err(gestalt_core::error::HarnessError::Cancelled)
        }
    }

    struct FPToolCatalog;
    impl gestalt_core::tool::ToolCatalog for FPToolCatalog {
        fn schemas(&self) -> Vec<gestalt_core::tool::ToolSchema> {
            vec![]
        }
        fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
            None
        }
    }

    struct FPMiddleware;
    impl gestalt_core::context::ContextPipeline for FPMiddleware {
        fn process(
            &self,
            _history: &[gestalt_core::SessionMessage],
            _budget: &gestalt_core::context::TokenBudget,
        ) -> Vec<gestalt_core::message::Message> {
            vec![]
        }
        fn version(&self) -> &str {
            "1"
        }
    }

    struct FPPolicyEngine;
    #[async_trait::async_trait]
    impl gestalt_core::policy::PolicyEngine for FPPolicyEngine {
        async fn evaluate(
            &self,
            _request: gestalt_core::policy::PolicyRequest,
        ) -> gestalt_core::policy::PolicyDecision {
            gestalt_core::policy::PolicyDecision::allowed(None)
        }
    }

    let manifest1 = ExtensionManifest {
        id: "fp-ext".to_string(),
        name: "FP Extension".to_string(),
        version: "0.1.0".to_string(),
        manifest_version: None,
        protocol_version: Some("1.1".to_string()),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while read -r line; do
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2); fi
  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)
  if [ "$method" = "initialize" ]; then
    echo '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"'"$req_id"'"}'
  fi
done"#
                    .to_string(),
            ],
        },
        capabilities: Capabilities {
            tools: true,
            ..Default::default()
        },
        permissions: Permissions {
            allow_shell: true,
            allow_workspace_read: true,
            ..Default::default()
        },
        tools: vec![],
        hooks: vec![],
        context_injectors: vec![],
    };

    let event_bus = RuntimeEventBus::new();
    let broker = Arc::new(
        ProcessExtensionBroker::spawn(
            manifest1.clone(),
            event_bus.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );
    let ext = Arc::new(ProcessExtension::new(manifest1, broker.clone()));

    let runtime = AgentRuntime::new(
        Arc::new(FPProvider),
        Arc::new(FPToolCatalog),
        Arc::new(FPMiddleware),
        Arc::new(FPPolicyEngine),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        None,
        RuntimeConfig::default(),
        gestalt_core::HookRegistry::default(),
        RuntimeRegistry::default(),
        None,
        event_bus.clone(),
        Arc::new(gestalt_mcp::McpRegistry::new(
            std::env::current_dir().unwrap(),
            Default::default(),
        )),
        Arc::new(std::sync::Mutex::new(
            gestalt_runtime::McpDiscoveryState::new(),
        )),
        vec![ext.clone() as Arc<dyn GestaltExtension>],
    );

    let inspect = runtime.inspect();
    let fp1 = inspect
        .negotiated_protocol_fingerprint
        .expect("Expected negotiated protocol fingerprint");
    assert!(!fp1.is_empty());

    broker.shutdown().await;
}
