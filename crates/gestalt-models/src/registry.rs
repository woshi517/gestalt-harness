use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use gestalt_core::{error::HarnessError, provider::Provider};
use serde_json::json;

use crate::{AnthropicProvider, OpenAiProvider};

pub type ProviderConfig = serde_json::Value;

pub type ProviderFactory =
    Box<dyn Fn(ProviderConfig) -> Result<Arc<dyn Provider>, HarnessError> + Send + Sync>;

static REGISTRY: OnceLock<RwLock<HashMap<&'static str, ProviderFactory>>> = OnceLock::new();

pub fn register(name: &'static str, factory: ProviderFactory) -> Result<(), HarnessError> {
    REGISTRY
        .get_or_init(init_defaults)
        .write()
        .map_err(|_| {
            HarnessError::Provider(gestalt_core::ProviderError::UnexpectedResponse {
                details: "provider registry poisoned".to_string(),
            })
        })?
        .insert(name, factory);
    Ok(())
}

pub fn get(name: &str, config: ProviderConfig) -> Result<Arc<dyn Provider>, HarnessError> {
    let registry = REGISTRY
        .get_or_init(init_defaults)
        .read()
        .map_err(|_| {
            HarnessError::Provider(gestalt_core::ProviderError::UnexpectedResponse {
                details: "provider registry poisoned".to_string(),
            })
        })?;

    let factory = registry
        .get(name)
        .ok_or_else(|| {
            HarnessError::Provider(gestalt_core::ProviderError::UnknownProvider(name.to_string()))
        })?;
    let provider = factory(config);
    drop(registry);

    provider
}

#[must_use]
pub fn registered() -> Vec<String> {
    let Ok(registry) = REGISTRY.get_or_init(init_defaults).read() else {
        return Vec::new();
    };

    let mut providers = registry.keys().map(|key| (*key).to_string()).collect::<Vec<_>>();
    providers.sort();
    providers
}

fn init_defaults() -> RwLock<HashMap<&'static str, ProviderFactory>> {
    let mut map: HashMap<&'static str, ProviderFactory> = HashMap::new();

    map.insert(
        "anthropic",
        Box::new(|config| Ok(Arc::new(AnthropicProvider::new(config)?))),
    );

    map.insert(
        "openai",
        Box::new(|config| Ok(Arc::new(OpenAiProvider::new(config)?))),
    );

    map.insert(
        "openai-compatible",
        Box::new(|config| {
            let config = merge_defaults(
                config,
                json!({
                    "id": "openai-compatible",
                    "display_name": "OpenAI Compatible",
                    "api_key_env": "OPENAI_COMPATIBLE_API_KEY"
                }),
            );
            Ok(Arc::new(OpenAiProvider::new(config)?))
        }),
    );

    RwLock::new(map)
}

fn merge_defaults(mut config: ProviderConfig, defaults: ProviderConfig) -> ProviderConfig {
    let Some(config_map) = config.as_object_mut() else {
        return defaults;
    };
    let Some(default_map) = defaults.as_object() else {
        return config;
    };

    for (key, value) in default_map {
        config_map.entry(key.clone()).or_insert_with(|| value.clone());
    }

    config
}
