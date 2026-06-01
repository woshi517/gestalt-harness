use gestalt_models::AnthropicProvider;
use serde_json::json;

#[test]
fn inline_api_keys_are_rejected_without_echoing_secret() {
    let Err(err) = AnthropicProvider::new(json!({"api_key": "sk-ant-test-secret"})) else {
        panic!("inline key should be rejected");
    };

    let rendered = err.to_string();
    assert!(rendered.contains("api_key"));
    assert!(!rendered.contains("sk-ant-test-secret"));
}
