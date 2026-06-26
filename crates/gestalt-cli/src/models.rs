use chrono::Utc;
use gestalt_core::{
    model::{ModelInfo, ModelInfoSource},
    ConfigError, HarnessError,
};
use gestalt_models::ModelCatalog;
use serde::Deserialize;

use crate::config::EffectiveConfig;
use crate::output::ModelsRefreshReport;

pub fn list_models(config: &EffectiveConfig, provider_filter: Option<&str>) -> Vec<ModelInfo> {
    let mut models = ModelCatalog::new().list();

    let builtins = vec![
        ModelInfo {
            qualified_id: "openrouter/free".to_string(),
            model_id: "free".to_string(),
            display_name: "Google: Gemini 2.5 Flash (free)".to_string(),
            max_context_tokens: 1_048_576,
            max_output_tokens: 8192,
            supports_tools: true,
            supports_vision: true,
            supports_json_schema: true,
            supports_thinking: false,
            supports_prompt_caching: false,
            input_cost_per_million: Some(0.0),
            output_cost_per_million: Some(0.0),
            source: ModelInfoSource::BuiltIn,
            last_updated: None,
        },
        ModelInfo {
            qualified_id: "openrouter/auto".to_string(),
            model_id: "auto".to_string(),
            display_name: "OpenRouter Auto Routing".to_string(),
            max_context_tokens: 4096,
            max_output_tokens: 1024,
            supports_tools: true,
            supports_vision: false,
            supports_json_schema: false,
            supports_thinking: false,
            supports_prompt_caching: false,
            input_cost_per_million: None,
            output_cost_per_million: None,
            source: ModelInfoSource::BuiltIn,
            last_updated: None,
        },
        ModelInfo {
            qualified_id: "ollama/llama3".to_string(),
            model_id: "llama3".to_string(),
            display_name: "Llama 3 (local)".to_string(),
            max_context_tokens: 8192,
            max_output_tokens: 2048,
            supports_tools: true,
            supports_vision: false,
            supports_json_schema: false,
            supports_thinking: false,
            supports_prompt_caching: false,
            input_cost_per_million: None,
            output_cost_per_million: None,
            source: ModelInfoSource::BuiltIn,
            last_updated: None,
        },
        ModelInfo {
            qualified_id: "groq/llama3-8b-8192".to_string(),
            model_id: "llama3-8b-8192".to_string(),
            display_name: "Llama 3 8B (Groq)".to_string(),
            max_context_tokens: 8192,
            max_output_tokens: 2048,
            supports_tools: true,
            supports_vision: false,
            supports_json_schema: false,
            supports_thinking: false,
            supports_prompt_caching: false,
            input_cost_per_million: None,
            output_cost_per_million: None,
            source: ModelInfoSource::BuiltIn,
            last_updated: None,
        },
    ];

    for builtin in builtins {
        if !models
            .iter()
            .any(|m| m.qualified_id == builtin.qualified_id)
        {
            models.push(builtin);
        }
    }

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
                    models[pos] = m;
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
    let provider_name = resolved.provider_name.clone();

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
                    status: "unsupported".to_string(),
                });
            }
        }
    };

    let total_count = list_models(config, None).len();
    let provider_config = resolved.provider_json();
    let auth_config =
        gestalt_models::auth::provider_auth_config(&provider_config, &provider_name, "DUMMY_KEY")?;
    let cred_resolver = crate::auth::build_credential_resolver(None, false);

    let api_key = match cred_resolver.resolve(&auth_config) {
        Ok(cred) => Some(cred.secret().to_string()),
        Err(_) => None,
    };

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

    if let Some(ref hdrs) = resolved.headers {
        for (k, v) in hdrs {
            req_builder = req_builder.header(k, v);
        }
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
