use std::sync::Arc;
use gestalt_runtime::{AgentRuntimeBuilder, GestaltExtension, RuntimeRegistry, RuntimeError};

struct TestExtension;

impl GestaltExtension for TestExtension {
    fn name(&self) -> &str {
        "test-extension"
    }

    fn register(&self, registry: &mut RuntimeRegistry) -> Result<(), RuntimeError> {
        registry.register_verifier("ext-verifier".to_string()).unwrap();
        Ok(())
    }
}

#[test]
fn test_extension_registration() {
    let builder = AgentRuntimeBuilder::new()
        .extension(Arc::new(TestExtension));
    
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
