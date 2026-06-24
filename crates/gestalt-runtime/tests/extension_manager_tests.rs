use std::collections::HashMap;
use std::sync::Arc;

use gestalt_core::tool::{ToolCatalog, ToolSchema};
use gestalt_runtime::extension::{
    ComponentFingerprint, ComponentInstanceId, ComponentKind, ExtensionInventory, ExtensionManager,
    ExtensionProcessInstance, ExtensionProcessState, ExtensionRuntimeComponent,
    NoopExtensionLauncher, ResolvedExtensionPackage, RuntimeExtensionSnapshot, RuntimeGeneration,
};
use gestalt_runtime::{RuntimeEventBus, RuntimeRegistryBuilder};
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
    }
}
