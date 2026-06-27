use std::sync::Arc;

use gestalt_core::provider::Provider;
use gestalt_models::{
    auth::{ConfiguredCredential, CredentialResolver, CredentialSource, ProviderAuthConfig},
    AnthropicProvider, OpenAiChatCompletionsProvider, ResolvedCredential,
};
use serde_json::json;

struct StubResolver;

impl CredentialResolver for StubResolver {
    fn resolve(
        &self,
        auth: &ProviderAuthConfig,
    ) -> Result<ResolvedCredential, gestalt_core::HarnessError> {
        let var = match &auth.credential {
            ConfiguredCredential::Environment(v) => v.clone(),
            _ => "DUMMY".to_string(),
        };
        Ok(ResolvedCredential::new(
            "sk-ant-test-secret".to_string(),
            CredentialSource::Environment { variable: var },
        ))
    }
}

#[test]
fn anthropic_auth_resolves_custom_env_var() {
    let config = json!({"api_key_env": "GESTALT_TEST_ANTHROPIC_KEY"});
    let provider = AnthropicProvider::new_with_resolver(&config, Arc::new(StubResolver))
        .expect("provider constructs");

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
    let provider = OpenAiChatCompletionsProvider::new(json!({
        "id": "openai-compatible",
        "base_url": "https://example.test/v1",
        "default_model": "gpt-4o-mini",
        "api_key_env": "GESTALT_TEST_OPENAI_KEY"
    }))
    .expect("provider constructs");

    assert_eq!(provider.id(), "openai-compatible");
    assert_eq!(provider.default_model(), "gpt-4o-mini");
    assert_eq!(
        provider.auth_config().credential_ref(),
        gestalt_models::auth::CredentialRef::Environment("GESTALT_TEST_OPENAI_KEY".to_string())
    );
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

#[test]
fn test_chain_credential_resolver() {
    struct MissingResolver;
    impl CredentialResolver for MissingResolver {
        fn resolve(
            &self,
            auth: &ProviderAuthConfig,
        ) -> Result<ResolvedCredential, gestalt_core::HarnessError> {
            Err(gestalt_core::HarnessError::Provider(
                gestalt_core::ProviderError::AuthFailed {
                    provider: auth.provider_id.clone(),
                },
            ))
        }
    }

    struct SuccessResolver;
    impl CredentialResolver for SuccessResolver {
        fn resolve(
            &self,
            _auth: &ProviderAuthConfig,
        ) -> Result<ResolvedCredential, gestalt_core::HarnessError> {
            Ok(ResolvedCredential::new(
                "sk-success".to_string(),
                CredentialSource::Session,
            ))
        }
    }

    let auth = ProviderAuthConfig {
        provider_id: "test".to_string(),
        credential: ConfiguredCredential::Environment("ENV_VAR".to_string()),
    };

    let chain = gestalt_models::auth::ChainCredentialResolver::new(vec![
        Arc::new(MissingResolver),
        Arc::new(SuccessResolver),
    ]);

    let resolved = chain.resolve(&auth).expect("resolves successfully");
    assert_eq!(resolved.secret(), "sk-success");
    assert_eq!(resolved.source(), &CredentialSource::Session);
}

#[test]
fn test_credential_ref_parsing() {
    let auth_keychain = ProviderAuthConfig {
        provider_id: "test".to_string(),
        credential: ConfiguredCredential::Keychain("provider/openrouter".to_string()),
    };
    assert_eq!(
        auth_keychain.credential_ref(),
        gestalt_models::auth::CredentialRef::Keychain("provider/openrouter".to_string())
    );

    let auth_env = ProviderAuthConfig {
        provider_id: "test".to_string(),
        credential: ConfiguredCredential::Environment("ENV_VAR".to_string()),
    };
    assert_eq!(
        auth_env.credential_ref(),
        gestalt_models::auth::CredentialRef::Environment("ENV_VAR".to_string())
    );
}
