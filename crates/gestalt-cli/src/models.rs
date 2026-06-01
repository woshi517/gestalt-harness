use gestalt_core::{ConfigError, HarnessError, model::ModelInfo};
use gestalt_models::ModelCatalog;

use crate::config::EffectiveConfig;

fn catalog(_config: &EffectiveConfig) -> ModelCatalog {
    ModelCatalog::new()
}

pub fn list_models(config: &EffectiveConfig) -> Vec<ModelInfo> {
    catalog(config).list()
}

pub fn inspect_model(config: &EffectiveConfig, model: &str) -> Result<ModelInfo, HarnessError> {
    catalog(config).get(model).ok_or_else(|| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: "model".to_string(),
            reason: format!("unknown model: {model}"),
        })
    })
}

pub fn refresh_models(config: &EffectiveConfig) -> String {
    format!("built-in catalog available: {} models", list_models(config).len())
}

pub fn select_model(config: &EffectiveConfig, model: &str) -> Result<String, HarnessError> {
    let info = inspect_model(config, model)?;
    Ok(format!("selected {} ({})", info.qualified_id, info.display_name))
}
