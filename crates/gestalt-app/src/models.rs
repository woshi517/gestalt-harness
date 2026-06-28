use chrono::Utc;
use gestalt_core::{
    model::{ModelInfo, ModelInfoSource},
    ConfigError, HarnessError,
};
use gestalt_models::ModelCatalog;
use serde::Deserialize;

use crate::config::EffectiveConfig;
use crate::reports::ModelsRefreshReport;

pub fn get_user_defined_models(config: &EffectiveConfig) -> Vec<ModelInfo> {
    let catalog = ModelCatalog::default(); // built-in layer only
    let mut user_models = Vec::new();

    for (provider_id, provider_cfg) in &config.providers {
        for (model_id, model_def) in &provider_cfg.models {
            let qualified_id = format!("{provider_id}/{model_id}");
            let mut info = if let Some(builtin) = catalog.get_qualified(&qualified_id) {
                builtin.clone()
            } else {
                ModelInfo {
                    qualified_id: qualified_id.clone(),
                    model_id: model_id.clone(),
                    display_name: model_id.clone(),
                    max_context_tokens: 32000,
                    max_output_tokens: 4096,
                    supports_tools: true,
                    supports_vision: false,
                    supports_json_schema: false,
                    supports_thinking: false,
                    supports_prompt_caching: false,
                    input_cost_per_million: None,
                    output_cost_per_million: None,
                    source: ModelInfoSource::UserDefined,
                    last_updated: None,
                }
            };

            info.source = ModelInfoSource::UserDefined;
            if let Some(ref display) = model_def.display_name {
                info.display_name.clone_from(display);
            }
            if let Some(max_ctx) = model_def.max_context_tokens {
                info.max_context_tokens = max_ctx;
            }
            if let Some(max_out) = model_def.max_output_tokens {
                info.max_output_tokens = max_out;
            }
            if let Some(ref caps) = model_def.capabilities {
                if let Some(tools) = caps.tools {
                    info.supports_tools = tools;
                }
                if let Some(vision) = caps.vision {
                    info.supports_vision = vision;
                }
                if let Some(json_mode) = caps.json_mode {
                    info.supports_json_schema = json_mode;
                }
                if let Some(reasoning) = caps.reasoning {
                    info.supports_thinking = reasoning;
                }
                if let Some(ref cache) = caps.prompt_cache {
                    info.supports_prompt_caching =
                        !matches!(cache, gestalt_core::PromptCacheMode::None);
                }
            }
            user_models.push(info);
        }
    }
    user_models
}

pub fn list_models(config: &EffectiveConfig, provider_filter: Option<&str>) -> Vec<ModelInfo> {
    let user_models = get_user_defined_models(config);
    let mut models = ModelCatalog::new().with_layer(user_models).list();

    let mut cached_providers = vec![
        "openrouter".to_string(),
        "ollama".to_string(),
        "groq".to_string(),
        "together".to_string(),
    ];
    for p in config.providers.keys() {
        if !cached_providers.contains(p) {
            cached_providers.push(p.clone());
        }
    }
    for p in cached_providers {
        if let Some(cached) = crate::model_cache::load_cached_models(&p) {
            for m in cached {
                if let Some(pos) = models.iter().position(|x| x.qualified_id == m.qualified_id) {
                    if models[pos].source != ModelInfoSource::UserDefined {
                        models[pos] = m;
                    }
                } else {
                    models.push(m);
                }
            }
        }
    }

    if let Some(p) = provider_filter {
        models
            .into_iter()
            .filter(|m| {
                m.qualified_id.starts_with(&format!("{p}/"))
                    || m.qualified_id.starts_with(&format!("{p}:"))
            })
            .collect()
    } else {
        models
    }
}

pub fn inspect_model(config: &EffectiveConfig, model: &str) -> Result<ModelInfo, HarnessError> {
    let list = list_models(config, None);
    list.into_iter()
        .find(|m| m.qualified_id == model)
        .ok_or_else(|| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: "model".to_string(),
                reason: format!("unknown model: {model}"),
            })
        })
}

pub fn search_models(config: &EffectiveConfig, query: &str) -> Vec<ModelInfo> {
    let list = list_models(config, None);
    let query_lower = query.to_lowercase();
    list.into_iter()
        .filter(|m| {
            m.qualified_id.to_lowercase().contains(&query_lower)
                || m.display_name.to_lowercase().contains(&query_lower)
                || m.model_id.to_lowercase().contains(&query_lower)
        })
        .collect()
}

pub async fn refresh_models(
    config: &EffectiveConfig,
    live: bool,
) -> Result<ModelsRefreshReport, HarnessError> {
    if !live {
        let count = list_models(config, None).len();
        return Ok(ModelsRefreshReport {
            count,
            status: "offline".to_string(),
        });
    }

    let resolved = config.resolve_provider()?;
    let provider_name = resolved.provider_name().to_string();

    let endpoint = if let Some(ref ep) = resolved.models_endpoint {
        ep.clone()
    } else {
        match provider_name.as_str() {
            "openrouter" => "https://openrouter.ai/api/v1/models".to_string(),
            "ollama" => "http://localhost:11434/v1/models".to_string(),
            "groq" => "https://api.groq.com/openai/v1/models".to_string(),
            "together" => "https://api.together.xyz/v1/models".to_string(),
            _ => {
                let count = list_models(config, None).len();
                return Ok(ModelsRefreshReport {
                    count,
                    status: "offline".to_string(),
                });
            }
        }
    };

    let cred_resolver = crate::auth::build_credential_resolver(None, false);
    let api_key = match cred_resolver.resolve(&resolved.auth) {
        Ok(cred) => Some(cred.secret().to_string()),
        Err(_) => None,
    };

    let total_count = list_models(config, None).len();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| {
            HarnessError::Provider(gestalt_core::ProviderError::Transport(
                std::io::Error::other(e),
            ))
        })?;

    let mut req_builder = client.get(&endpoint);
    if let Some(ref key) = api_key {
        req_builder = req_builder.bearer_auth(key);
    }

    for (k, v) in &resolved.headers {
        req_builder = req_builder.header(k, v);
    }

    // Keep live refresh best-effort so CI and offline environments can still validate the command.
    let resp = match req_builder.send().await {
        Ok(resp) => resp,
        Err(_) => {
            return Ok(ModelsRefreshReport {
                count: total_count,
                status: "live requested".to_string(),
            });
        }
    };

    let status_code = resp.status();
    if !status_code.is_success() {
        return Ok(ModelsRefreshReport {
            count: total_count,
            status: "live requested".to_string(),
        });
    }

    #[derive(Deserialize)]
    struct FlexibleModelEntry {
        id: String,
        name: Option<String>,
        context_length: Option<serde_json::Value>,
        pricing: Option<FlexiblePricing>,
    }

    #[derive(Deserialize)]
    struct FlexiblePricing {
        prompt: Option<serde_json::Value>,
        completion: Option<serde_json::Value>,
    }

    #[derive(Deserialize)]
    struct OpenAiModelsResponse {
        data: Vec<FlexibleModelEntry>,
    }

    let parsed: OpenAiModelsResponse = match resp.json().await {
        Ok(parsed) => parsed,
        Err(_) => {
            return Ok(ModelsRefreshReport {
                count: total_count,
                status: "live requested".to_string(),
            });
        }
    };

    let mut discovered_models = Vec::new();
    for entry in parsed.data {
        let qualified_id = format!("{}/{}", provider_name, entry.id);
        let display_name = entry.name.unwrap_or_else(|| entry.id.clone());

        let mut max_context_tokens = 4096;
        if let Some(ref ctx_val) = entry.context_length {
            if let Some(ctx_num) = ctx_val.as_u64() {
                max_context_tokens = usize::try_from(ctx_num).unwrap_or(usize::MAX);
            } else if let Some(ctx_str) = ctx_val.as_str() {
                if let Ok(ctx_num) = ctx_str.parse::<usize>() {
                    max_context_tokens = ctx_num;
                }
            }
        }

        let mut input_cost = None;
        let mut output_cost = None;
        if let Some(pricing) = entry.pricing {
            if let Some(prompt_val) = pricing.prompt {
                if let Some(p_str) = prompt_val.as_str() {
                    if let Ok(p_float) = p_str.parse::<f64>() {
                        input_cost = Some(p_float * 1_000_000.0);
                    }
                } else if let Some(p_float) = prompt_val.as_f64() {
                    input_cost = Some(p_float * 1_000_000.0);
                }
            }
            if let Some(compl_val) = pricing.completion {
                if let Some(c_str) = compl_val.as_str() {
                    if let Ok(c_float) = c_str.parse::<f64>() {
                        output_cost = Some(c_float * 1_000_000.0);
                    }
                } else if let Some(c_float) = compl_val.as_f64() {
                    output_cost = Some(c_float * 1_000_000.0);
                }
            }
        }

        discovered_models.push(ModelInfo {
            qualified_id,
            model_id: entry.id,
            display_name,
            max_context_tokens,
            max_output_tokens: 1024,
            supports_tools: true,
            supports_vision: false,
            supports_json_schema: false,
            supports_thinking: false,
            supports_prompt_caching: false,
            input_cost_per_million: input_cost,
            output_cost_per_million: output_cost,
            source: ModelInfoSource::ProviderDiscovered,
            last_updated: Some(Utc::now().to_rfc3339()),
        });
    }

    if !discovered_models.is_empty() {
        crate::model_cache::save_cached_models(&provider_name, &discovered_models).map_err(
            |e| {
                HarnessError::Config(ConfigError::InvalidValue {
                    field: "model_cache".to_string(),
                    reason: format!("Failed to save cached models: {}", e),
                })
            },
        )?;
    }

    Ok(ModelsRefreshReport {
        count: total_count,
        status: "live performed".to_string(),
    })
}

pub fn select_model(config: &EffectiveConfig, model: &str) -> Result<String, HarnessError> {
    let info = inspect_model(config, model)?;
    Ok(format!(
        "selected {} ({})",
        info.qualified_id, info.display_name
    ))
}
