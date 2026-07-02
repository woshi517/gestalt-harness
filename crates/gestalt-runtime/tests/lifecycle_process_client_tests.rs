use std::sync::Arc;

use gestalt_core::tool::{ToolCatalog, ToolSchema};
use gestalt_runtime::unstable::extension::{
    ComponentInstanceId, ComponentKind, ExtensionManager, ExtensionRuntimeComponent,
    LocalProcessLauncher, RuntimeExtensionSnapshot, RuntimeGeneration,
};
use gestalt_runtime::unstable::lifecycle::{
    InitializeRequestV2, LifecycleCapabilityKind, LifecycleClient, LifecycleInvokeRequestV2,
    ProcessLifecycleClient,
};
use gestalt_runtime::unstable::{RuntimeEventBus, RuntimeRegistryBuilder};
use serde_json::json;

struct EmptyToolCatalog;

impl ToolCatalog for EmptyToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

#[tokio::test]
async fn process_lifecycle_client_invokes_v2_capabilities_through_child_process() {
    let event_bus = RuntimeEventBus::new();
    let manager = Arc::new(ExtensionManager::new(
        Arc::new(snapshot_with_generation(0)),
        event_bus.clone(),
        Arc::new(LocalProcessLauncher),
        gestalt_runtime::unstable::activation::HostLaunchContext::default(),
    ));
    let component = lifecycle_component();
    let host_context = gestalt_runtime::unstable::activation::HostLaunchContext {
        event_bus: event_bus.clone(),
        workspace_root: std::path::PathBuf::from("."),
        allow_network: false,
        effective_permissions: None,
        trusted_extension_pins: vec![],
        timeout_initialize_ms: 10000,
        timeout_hook_ms: 5000,
        timeout_context_ms: 15000,
        timeout_tool_ms: 60000,
        timeout_shutdown_ms: 5000,
        max_message_bytes: 8_388_608,
        max_pending_requests: 16,
        environment: std::collections::HashMap::new(),
        package_source_root: None,
        extension_instances: std::collections::BTreeMap::new(),
        allow_untrusted_extensions: false,
        #[cfg(feature = "mcp")]
        mcp_servers: std::collections::HashMap::new(),
    };
    let client = ProcessLifecycleClient::new(manager.clone(), component.clone(), host_context);

    let initialized = client
        .initialize(InitializeRequestV2 {
            supported_versions: vec!["2.0".to_string()],
        })
        .await
        .unwrap();
    assert_eq!(initialized.negotiated_version, "2.0");
    assert!(initialized.supports_cancellation);

    let described = client.describe_capabilities().await.unwrap();
    assert_eq!(described.len(), 1);
    assert_eq!(described[0].component_id, component.id.canonical_id());
    assert_eq!(
        described[0].capability,
        LifecycleCapabilityKind::ContextProvider
    );

    let invoked = client
        .invoke(LifecycleInvokeRequestV2 {
            component_id: component.id.canonical_id(),
            capability: LifecycleCapabilityKind::ContextProvider,
            payload: json!({ "request": "context" }),
        })
        .await
        .unwrap();
    assert_eq!(
        invoked.payload,
        json!({
            "component_id": component.id.canonical_id(),
            "handled_capability": "context_provider",
            "echo": { "request": "context" }
        })
    );

    assert_eq!(manager.process_instances().len(), 1);
    assert_eq!(
        manager.process_instances()[0].state(),
        gestalt_runtime::unstable::extension::ExtensionProcessState::Ready
    );

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn process_lifecycle_client_reuses_processes_and_respects_draining_state() {
    let manager = Arc::new(ExtensionManager::new(
        Arc::new(snapshot_with_generation(0)),
        RuntimeEventBus::new(),
        Arc::new(LocalProcessLauncher),
        gestalt_runtime::unstable::activation::HostLaunchContext::default(),
    ));
    let component = lifecycle_component();
    let host_context = gestalt_runtime::unstable::activation::HostLaunchContext {
        event_bus: manager.event_bus.clone(),
        workspace_root: std::path::PathBuf::from("."),
        allow_network: false,
        effective_permissions: None,
        trusted_extension_pins: vec![],
        timeout_initialize_ms: 10000,
        timeout_hook_ms: 5000,
        timeout_context_ms: 15000,
        timeout_tool_ms: 60000,
        timeout_shutdown_ms: 5000,
        max_message_bytes: 8_388_608,
        max_pending_requests: 16,
        environment: std::collections::HashMap::new(),
        package_source_root: None,
        extension_instances: std::collections::BTreeMap::new(),
        allow_untrusted_extensions: false,
        #[cfg(feature = "mcp")]
        mcp_servers: std::collections::HashMap::new(),
    };
    let first =
        ProcessLifecycleClient::new(manager.clone(), component.clone(), host_context.clone());
    let second = ProcessLifecycleClient::new(manager.clone(), component.clone(), host_context);

    first.describe_capabilities().await.unwrap();
    second.describe_capabilities().await.unwrap();

    assert_eq!(manager.process_instances().len(), 1);

    manager.drain_process(&component).await.unwrap();

    let err = second.describe_capabilities().await.unwrap_err();
    assert!(
        err.to_string().contains("not accepting new calls"),
        "unexpected error: {err}"
    );

    manager.shutdown_all().await.unwrap();
}

fn snapshot_with_generation(generation: u64) -> RuntimeExtensionSnapshot {
    let registry = RuntimeRegistryBuilder::new().snapshot();
    let catalog = Arc::new(EmptyToolCatalog);
    RuntimeExtensionSnapshot::from_registry_snapshot(
        RuntimeGeneration(generation),
        registry,
        catalog,
        #[cfg(feature = "mcp")]
        Arc::new(gestalt_runtime::unstable::mcp::McpRegistry::new(
            std::path::PathBuf::from("."),
            std::collections::HashMap::new(),
        )),
    )
}

fn lifecycle_component() -> ExtensionRuntimeComponent {
    ExtensionRuntimeComponent {
        id: ComponentInstanceId::new("com.example.lifecycle", "primary", "lifecycle"),
        kind: ComponentKind::GestaltLifecycle,
        optional: false,
        supports_cancellation: true,
        entrypoint_command: "python3".to_string(),
        entrypoint_args: vec![
            "-c".to_string(),
            r#"import json,sys
while True:
    line = sys.stdin.readline()
    if not line:
        break
    req = json.loads(line)
    method = req.get("method")
    req_id = req.get("id")
    params = req.get("params") or {}
    if method == "initialize":
        versions = params.get("supported_versions")
        if versions is not None:
            result = {"negotiated_version": "2.0", "supports_cancellation": True}
        else:
            result = {"negotiated_version": "2.0", "supports_cancellation": True}
    elif method == "capabilities/describe":
        result = [{
            "component_id": "component:com.example.lifecycle:primary:lifecycle",
            "capability": "context_provider",
            "priority": 10,
            "timeout_ms": 250,
            "failure_mode": "fail_open",
            "data_scope": "current_turn"
        }]
    elif method == "lifecycle/invoke":
        result = {
            "payload": {
                "component_id": params["component_id"],
                "handled_capability": params["capability"],
                "echo": params["payload"]
            }
        }
    elif method == "shutdown":
        result = {}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "result": result, "id": req_id}) + "\n")
    sys.stdout.flush()
"#
            .to_string(),
        ],
        config: json!({ "policySet": "default" }),
        grants_fingerprint: "grants-a".to_string(),
        trust: gestalt_runtime::unstable::ExtensionTrust::BuiltIn,
        protocol_fingerprint: Some("2.0".to_string()),
        package_version: "1.0.0".to_string(),
        manifest_hash: None,
        executable_hash: None,
        dependency_lock_hash: None,
        permissions: gestalt_runtime::unstable::manifest::Permissions {
            allow_shell: true,
            ..Default::default()
        },
        grants: gestalt_runtime::unstable::extension::ExtensionGrantConfig {
            shell: true,
            ..Default::default()
        },
        package_source_root: None,
    }
}
