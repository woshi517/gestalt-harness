use std::sync::Arc;

use gestalt_core::provider::Provider;
use gestalt_models::{
    AnthropicProvider, OpenAiProvider, ResolvedCredential,
    auth::{CredentialResolver, CredentialSource, ProviderAuthConfig},
};
use serde_json::json;

struct StubResolver;

impl CredentialResolver for StubResolver {
    fn resolve(
        &self,
        auth: &ProviderAuthConfig,
    ) -> Result<ResolvedCredential, gestalt_core::HarnessError> {
        Ok(ResolvedCredential::new(
            "sk-ant-test-secret".to_string(),
            CredentialSource::Environment {
                variable: auth.api_key_env.clone(),
            },
        ))
    }
}

#[test]
fn anthropic_auth_resolves_custom_env_var() {
    let config = json!({"api_key_env": "GESTALT_TEST_ANTHROPIC_KEY"});
    let provider =
        AnthropicProvider::new_with_resolver(&config, Arc::new(StubResolver)).expect("provider constructs");

    let resolved = StubResolver
        .resolve(provider.auth_config())
        .expect("credential resolves");

    assert_eq!(resolved.secret(), "sk-ant-test-secret");
    assert_eq!(
        resolved.source(),
        &CredentialSource::Environment {
            variable: "GESTALT_TEST_ANTHROPIC_KEY".to_string()
        }
    );
}

#[test]
fn openai_compatible_preserves_base_url_and_model() {
    let provider = OpenAiProvider::new(json!({
        "id": "openai-compatible",
        "base_url": "https://example.test/v1",
        "default_model": "gpt-4o-mini",
        "api_key_env": "GESTALT_TEST_OPENAI_KEY"
    }))
    .expect("provider constructs");

    assert_eq!(provider.id(), "openai-compatible");
    assert_eq!(provider.default_model(), "gpt-4o-mini");
    assert_eq!(provider.auth_config().api_key_env, "GESTALT_TEST_OPENAI_KEY");
}

#[test]
fn resolved_credential_debug_and_display_are_redacted() {
    let credential = ResolvedCredential::new(
        "sk-test-secret".to_string(),
        CredentialSource::Environment {
            variable: "OPENAI_API_KEY".to_string(),
        },
    );

    assert!(!format!("{credential:?}").contains("sk-test-secret"));
    assert_eq!(credential.to_string(), "[REDACTED]");
}
