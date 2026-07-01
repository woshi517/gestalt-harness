use std::sync::Arc;

use gestalt_core::tool::{ToolCatalog, ToolSchema};
use gestalt_runtime::unstable::extension::{RuntimeExtensionSnapshot, RuntimeGeneration};
use gestalt_runtime::unstable::{RuntimeRegistryBuilder, ToolRegistrationSnapshot};
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
fn registry_snapshot_fingerprint_is_deterministic_for_equal_inputs() {
    let left = registry_with_one_tool().snapshot();
    let right = registry_with_one_tool().snapshot();

    assert_eq!(left.fingerprint, right.fingerprint);
    assert_eq!(
        left.tools.keys().collect::<Vec<_>>(),
        right.tools.keys().collect::<Vec<_>>()
    );
}

#[test]
fn registry_snapshot_is_not_changed_by_later_builder_mutation() {
    let mut builder = registry_with_one_tool();
    let snapshot = builder.snapshot();

    builder
        .register_tool("tool-b".to_string(), json!({ "name": "tool-b" }))
        .unwrap();

    assert!(snapshot.tools.contains_key("tool-a"));
    assert!(!snapshot.tools.contains_key("tool-b"));
    assert_eq!(snapshot.tools.len(), 1);
}

#[test]
fn runtime_extension_snapshot_pins_generation_fingerprint_and_catalog() {
    let registry_snapshot = registry_with_one_tool().snapshot();
    let catalog = Arc::new(EmptyToolCatalog);
    let snapshot = RuntimeExtensionSnapshot::from_registry_snapshot(
        RuntimeGeneration(7),
        registry_snapshot.clone(),
        catalog,
        #[cfg(feature = "mcp")]
        Arc::new(gestalt_runtime::unstable::mcp::McpRegistry::new(
            std::path::PathBuf::from("."),
            std::collections::HashMap::new(),
        )),
    );

    assert_eq!(snapshot.generation, RuntimeGeneration(7));
    assert_eq!(snapshot.fingerprint, registry_snapshot.fingerprint);
    assert_eq!(snapshot.tool_catalog.schemas().len(), 0);
    assert!(snapshot.package_health.is_empty());
}

fn registry_with_one_tool() -> RuntimeRegistryBuilder {
    let mut registry = RuntimeRegistryBuilder::new();
    registry
        .register_tool("tool-a".to_string(), json!({ "name": "tool-a" }))
        .unwrap();
    registry
        .register_hook("before_tool_policy".to_string())
        .unwrap();
    registry
        .register_verifier("no_secrets".to_string())
        .unwrap();
    registry.register_extension("ext-a".to_string()).unwrap();
    registry
}

#[test]
fn registry_snapshot_carries_typed_hook_and_verifier_descriptors() {
    let snapshot = registry_with_one_tool().snapshot();

    assert_eq!(snapshot.hooks[0].name, "before_tool_policy");
    assert_eq!(snapshot.verifiers[0].name, "no_secrets");
}

#[test]
fn registry_snapshot_preserves_tool_metadata() {
    let snapshot = registry_with_one_tool().snapshot();
    let tool: &ToolRegistrationSnapshot = snapshot.tools.get("tool-a").unwrap();

    assert_eq!(tool.name, "tool-a");
    assert_eq!(tool.schema_hash.len(), 64);
}
