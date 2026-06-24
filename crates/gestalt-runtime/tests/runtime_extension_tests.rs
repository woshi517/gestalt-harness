#![allow(deprecated)]

use gestalt_core::ContextStability;
use gestalt_runtime::{AgentRuntimeBuilder, GestaltExtension, RuntimeError, RuntimeRegistry};
use std::sync::Arc;

struct TestExtension;

impl GestaltExtension for TestExtension {
    fn name(&self) -> &str {
        "test-extension"
    }

    fn register(&self, registry: &mut RuntimeRegistry) -> Result<(), RuntimeError> {
        registry
            .register_verifier("ext-verifier".to_string())
            .unwrap();
        Ok(())
    }
}

#[test]
fn test_extension_registration() {
    let builder = AgentRuntimeBuilder::new().extension(Arc::new(TestExtension));

    assert_eq!(builder.extensions.len(), 1);
    assert_eq!(builder.extensions[0].name(), "test-extension");
}

#[test]
fn test_duplicate_extension_fails() {
    let builder = AgentRuntimeBuilder::new()
        .extension(Arc::new(TestExtension))
        .extension(Arc::new(TestExtension));

    let res = builder.build();
    assert!(res.is_err());
    assert!(format!("{:?}", res.err().unwrap()).contains("Duplicate extension name"));
}

#[tokio::test]
async fn test_context_contributor_stability_is_exposed() {
    let manifest_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = gestalt_runtime::ExtensionManifest::parse(&content).unwrap();

    let event_bus = gestalt_runtime::RuntimeEventBus::new();
    let broker = std::sync::Arc::new(
        gestalt_runtime::ProcessExtensionBroker::spawn(
            manifest.clone(),
            event_bus,
            Default::default(),
            Default::default(),
            true,
        )
        .await
        .unwrap(),
    );

    let mut registry = RuntimeRegistry::new();
    gestalt_runtime::ProcessExtension::new(manifest, broker)
        .register(&mut registry)
        .unwrap();

    let metadata = registry.context_contributors.get("bash_context").unwrap();
    assert_eq!(metadata.stability, ContextStability::TurnDynamic);
}
