use gestalt_core::{model::ModelInfo, ConfigError, HarnessError};
use gestalt_models::ModelCatalog;

use crate::config::EffectiveConfig;

use crate::output::ModelsRefreshReport;

fn catalog(_config: &EffectiveConfig) -> ModelCatalog {
    ModelCatalog::new()
}

pub fn list_models(config: &EffectiveConfig, provider_filter: Option<&str>) -> Vec<ModelInfo> {
    let list = catalog(config).list();
    if let Some(p) = provider_filter {
        list.into_iter()
            .filter(|m| m.qualified_id.starts_with(&format!("{p}/")))
            .collect()
    } else {
        list
    }
}

pub fn inspect_model(config: &EffectiveConfig, model: &str) -> Result<ModelInfo, HarnessError> {
    catalog(config).get(model).ok_or_else(|| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: "model".to_string(),
            reason: format!("unknown model: {model}"),
        })
    })
}

pub async fn refresh_models(config: &EffectiveConfig, live: bool) -> Result<ModelsRefreshReport, HarnessError> {
    let count = list_models(config, None).len();
    if !live {
        return Ok(ModelsRefreshReport {
            count,
            status: "offline".to_string(),
        });
    }

    let status = if let Ok(provider) = config.selected_provider() {
        match crate::providers::probe_provider(config, &provider).await {
            Ok(_) => "live performed".to_string(),
            Err(HarnessError::Provider(gestalt_core::ProviderError::AuthFailed { .. })) => {
                "offline".to_string()
            }
            Err(HarnessError::Provider(gestalt_core::ProviderError::UnexpectedResponse { .. })) => {
                "unsupported".to_string()
            }
            Err(_) => {
                "offline".to_string()
            }
        }
    } else {
        "unsupported".to_string()
    };

    Ok(ModelsRefreshReport { count, status })
}

pub fn select_model(config: &EffectiveConfig, model: &str) -> Result<String, HarnessError> {
    let info = inspect_model(config, model)?;
    Ok(format!(
        "selected {} ({})",
        info.qualified_id, info.display_name
    ))
}
