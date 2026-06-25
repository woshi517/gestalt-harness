use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gestalt_core::tool::{ToolCatalog, ToolSchema};
use gestalt_runtime::extension::{
    ComponentFingerprint, ComponentInstanceId, ComponentKind, ExtensionInventory,
    ExtensionLauncher, ExtensionManager, ExtensionProcessInstance, ExtensionProcessState,
    ExtensionRuntimeComponent, NoopExtensionLauncher, ResolvedExtensionPackage,
    RuntimeExtensionSnapshot, RuntimeGeneration,
};
use gestalt_runtime::{ExtensionManifest, RuntimeEventBus, RuntimeRegistryBuilder};
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

#[test]
fn extension_manager_publishes_active_snapshot_atomically() {
    let manager = manager_with_snapshot(snapshot_with_generation(0));
    let next = Arc::new(snapshot_with_generation(1));

    manager.publish_snapshot(next.clone()).unwrap();

    assert_eq!(manager.current_generation(), RuntimeGeneration(1));
    assert!(Arc::ptr_eq(&manager.active_snapshot(), &next));
}

#[test]
fn extension_manager_owns_package_inventory() {
    let mut inventory = ExtensionInventory::default();
    inventory.add_package(resolved_package("com.example.review", "review-primary"));
    let manager = manager_with_snapshot(snapshot_with_generation(0)).with_inventory(inventory);

    let inventory = manager.inventory();
    let package = inventory.find_package("com.example.review").unwrap();

    assert_eq!(package.descriptor.id, "com.example.review");
    assert_eq!(package.instance_id, "review-primary");
}

#[test]
fn component_fingerprint_is_deterministic_and_changes_with_execution_inputs() {
    let component = runtime_component("com.example.review", "review-primary", "lifecycle");
    let same = runtime_component("com.example.review", "review-primary", "lifecycle");
    let changed = runtime_component("com.example.review", "review-primary", "other");

    assert_eq!(
        ComponentFingerprint::from_component(&component),
        ComponentFingerprint::from_component(&same)
    );
    assert_ne!(
        ComponentFingerprint::from_component(&component),
        ComponentFingerprint::from_component(&changed)
    );
    assert_eq!(
        component.reuse_key(),
        (
            ComponentInstanceId::new("com.example.review", "review-primary", "lifecycle"),
            ComponentFingerprint::from_component(&component)
        )
    );
}

#[test]
fn draining_process_instances_reject_new_calls_but_track_existing_calls() {
    let process = ExtensionProcessInstance::new("component-a".to_string());
    let guard = process.begin_call().unwrap();

    process.transition_to(ExtensionProcessState::Draining);

    assert!(process.begin_call().is_err());
    assert_eq!(process.in_flight_calls(), 1);
    drop(guard);
    assert_eq!(process.in_flight_calls(), 0);
}

#[tokio::test]
async fn extension_manager_reuses_ready_processes_and_tracks_health() {
    let launcher = Arc::new(CountingLauncher::default());
    let manager = ExtensionManager::new(
        Arc::new(snapshot_with_generation(0)),
        RuntimeEventBus::new(),
        launcher.clone(),
    );
    let component = runtime_component("com.example.review", "review-primary", "lifecycle");

    let first = manager.launch_process(&component).await.unwrap();
    let second = manager.launch_process(&component).await.unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(launcher.launch_count(), 1);
    assert_eq!(manager.process_instances().len(), 1);
    assert_eq!(
        manager.process_health(),
        vec![gestalt_runtime::extension::ExtensionInstanceHealth {
            instance_id: component.id.canonical_id(),
            status: gestalt_runtime::extension::ExtensionInstanceHealthStatus::Ready,
            message: None,
        }]
    );

    manager.drain_process(&component).await.unwrap();

    assert_eq!(first.state(), ExtensionProcessState::Draining);
    assert_eq!(
        manager.process_health(),
        vec![gestalt_runtime::extension::ExtensionInstanceHealth {
            instance_id: component.id.canonical_id(),
            status: gestalt_runtime::extension::ExtensionInstanceHealthStatus::Degraded,
            message: Some("process is draining".to_string()),
        }]
    );

    manager.shutdown_process(&component).await.unwrap();

    assert!(manager.process_instances().is_empty());
}

#[tokio::test]
async fn legacy_process_extensions_reuse_manager_owned_broker_instances() {
    let manifest = mock_extension_manifest();
    let manager = manager_with_snapshot(snapshot_with_generation(0));

    let first = manager
        .launch_legacy_process_extension(
            manifest.clone(),
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap();
    let second = manager
        .launch_legacy_process_extension(manifest, Default::default(), Default::default(), true)
        .await
        .unwrap();

    assert_eq!(manager.process_instances().len(), 1);
    assert!(Arc::ptr_eq(&first.broker, &second.broker));

    manager.shutdown_all().await.unwrap();
}

fn manager_with_snapshot(snapshot: RuntimeExtensionSnapshot) -> ExtensionManager {
    ExtensionManager::new(
        Arc::new(snapshot),
        RuntimeEventBus::new(),
        Arc::new(NoopExtensionLauncher),
    )
}

fn snapshot_with_generation(generation: u64) -> RuntimeExtensionSnapshot {
    let registry = RuntimeRegistryBuilder::new().snapshot();
    let catalog = Arc::new(EmptyToolCatalog);
    let mcp = Arc::new(gestalt_mcp::McpRegistry::new(
        std::env::current_dir().unwrap(),
        HashMap::new(),
    ));
    RuntimeExtensionSnapshot::from_registry_snapshot(
        RuntimeGeneration(generation),
        registry,
        catalog,
        mcp,
    )
}

fn resolved_package(package_id: &str, instance_id: &str) -> ResolvedExtensionPackage {
    let manifest = gestalt_runtime::extension::ExtensionManifestV2::parse(&format!(
        r#"
manifest_version = 2

[package]
id = "{package_id}"
name = "Review"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "review.lifecycle"]
"#
    ))
    .unwrap();

    ResolvedExtensionPackage::from_v2_manifest(manifest, instance_id).unwrap()
}

fn runtime_component(
    package_id: &str,
    instance_id: &str,
    component_id: &str,
) -> ExtensionRuntimeComponent {
    ExtensionRuntimeComponent {
        id: ComponentInstanceId::new(package_id, instance_id, component_id),
        kind: ComponentKind::GestaltLifecycle,
        optional: false,
        entrypoint_command: "python".to_string(),
        entrypoint_args: vec!["-m".to_string(), "review.lifecycle".to_string()],
        config: json!({ "policySet": "default" }),
        grants_fingerprint: "grants-a".to_string(),
        trust_fingerprint: "trust-a".to_string(),
        protocol_fingerprint: Some("protocol-a".to_string()),
        package_version: "1.0.0".to_string(),
        manifest_hash: None,
        executable_hash: None,
        dependency_lock_hash: None,
    }
}

fn mock_extension_manifest() -> ExtensionManifest {
    let manifest_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(manifest_path).unwrap();
    ExtensionManifest::parse(&content).unwrap()
}

#[derive(Default)]
struct CountingLauncher {
    launches: AtomicUsize,
}

impl CountingLauncher {
    fn launch_count(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ExtensionLauncher for CountingLauncher {
    async fn launch(
        &self,
        component: &ExtensionRuntimeComponent,
    ) -> gestalt_runtime::Result<Arc<ExtensionProcessInstance>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let process = Arc::new(ExtensionProcessInstance::new(component.id.canonical_id()));
        process.transition_to(ExtensionProcessState::Ready);
        Ok(process)
    }
}
