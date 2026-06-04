use serde_json::json;
use gestalt_runtime::{RuntimeRegistry, compute_schema_hash, compute_tool_schema_hash};

#[test]
fn test_registry_duplicate_checks() {
    let mut reg = RuntimeRegistry::new();
    
    reg.register_tool("tool1".to_string(), json!({})).unwrap();
    let res = reg.register_tool("tool1".to_string(), json!({}));
    assert!(res.is_err());
    assert!(format!("{:?}", res.err().unwrap()).contains("Duplicate tool"));

    reg.register_verifier("verifier1".to_string()).unwrap();
    let res = reg.register_verifier("verifier1".to_string());
    assert!(res.is_err());

    reg.register_hook("hook1".to_string()).unwrap();
    let res = reg.register_hook("hook1".to_string());
    assert!(res.is_err());
}

#[test]
fn test_schema_hashes() {
    let schema1 = json!({
        "name": "test_tool",
        "description": "a test tool",
    });
    let hash1 = compute_schema_hash(&schema1);
    assert!(!hash1.is_empty());

    let schemas = vec![schema1];
    let hash_all = compute_tool_schema_hash(&schemas);
    assert!(!hash_all.is_empty());
}
