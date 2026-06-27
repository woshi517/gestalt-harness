use gestalt_models::AnthropicProvider;
use serde_json::json;

#[test]
fn inline_api_keys_are_accepted() {
    let provider = AnthropicProvider::new(json!({"api_key": "sk-ant-test-secret"}));
    assert!(provider.is_ok());
}
