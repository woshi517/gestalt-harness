use gestalt_core::{
    provider::ProviderCapabilities,
    tool::RiskLevel,
    tool_descriptor::{
        CanonicalToolId, ProviderToolFormat, ToolAnnotations, ToolDescriptor, ToolNamespace,
        ToolResponseContract,
    },
};
use gestalt_models::strict_schema::make_strict_schema;
use gestalt_models::tool_schema_adapter::ToolSchemaAdapter;
use serde_json::json;

#[test]
fn test_make_strict_schema_basic() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name"]
    });

    let strict = make_strict_schema(&schema);

    // Check that additionalProperties is false
    assert_eq!(strict["additionalProperties"], false);

    // Check that both fields are required
    let req = strict["required"].as_array().unwrap();
    assert_eq!(req.len(), 2);
    assert!(req.contains(&json!("name")));
    assert!(req.contains(&json!("age")));

    // Check that the optional field "age" is now nullable
    assert_eq!(
        strict["properties"]["age"]["type"],
        json!(["integer", "null"])
    );
    // The required field "name" should NOT be nullable
    assert_eq!(strict["properties"]["name"]["type"], json!("string"));
}

#[test]
fn test_make_strict_schema_recursive() {
    let schema = json!({
        "type": "object",
        "properties": {
            "nested": {
                "type": "object",
                "properties": {
                    "inner_val": { "type": "string" }
                }
            }
        }
    });

    let strict = make_strict_schema(&schema);
    assert_eq!(strict["additionalProperties"], false);
    assert_eq!(
        strict["properties"]["nested"]["additionalProperties"],
        false
    );
    assert_eq!(
        strict["properties"]["nested"]["properties"]["inner_val"]["type"],
        json!(["string", "null"])
    );
}

#[test]
fn test_adapter_strict_mode() {
    let descriptor = ToolDescriptor {
        id: CanonicalToolId {
            namespace: ToolNamespace::BuiltIn,
            name: "test_tool".to_string(),
        },
        description: "A test tool".to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "param": { "type": "string" }
            }
        }),
        risk: RiskLevel::Low,
        annotations: ToolAnnotations::default(),
        response_contract: ToolResponseContract {
            format: ProviderToolFormat::Text,
            shape_rules: None,
        },
        retry_policy: None,
    };

    // 1. With supports_strict_schema = true
    let mut caps = ProviderCapabilities::default();
    caps.supports_strict_schema = true;

    let (adapted, mapping) = ToolSchemaAdapter::adapt(&descriptor, &caps);
    assert_eq!(adapted.strict, Some(true));
    assert_eq!(adapted.input_schema["additionalProperties"], false);
    assert_eq!(mapping.provider_name, "test_tool");

    // 2. With supports_strict_schema = false
    caps.supports_strict_schema = false;
    let (adapted_no_strict, _) = ToolSchemaAdapter::adapt(&descriptor, &caps);
    assert_eq!(adapted_no_strict.strict, None);
    assert!(adapted_no_strict
        .input_schema
        .get("additionalProperties")
        .is_none());
}
