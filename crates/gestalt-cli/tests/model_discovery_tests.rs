use chrono::{Duration, Utc};
use gestalt_cli::config::{load_effective_config, CliOverrides};
use gestalt_cli::model_cache::{
    get_cache_path, load_cached_models, save_cached_models, CachedModels,
};
use gestalt_cli::models::{list_models, search_models};
use gestalt_core::model::{ModelInfo, ModelInfoSource};
use std::fs;
use std::sync::Mutex;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(ref val) = self.original {
            std::env::set_var(self.key, val);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn test_config(temp_dir: &std::path::Path) -> gestalt_cli::config::EffectiveConfig {
    load_effective_config(&CliOverrides {
        workspace: Some(temp_dir.to_path_buf()),
        ..CliOverrides::default()
    })
    .expect("config loads")
}

#[test]
fn test_builtin_models_inclusion() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_dir = std::env::temp_dir().join("gestalt_test_builtin_models");
    fs::create_dir_all(&temp_dir).unwrap();

    let cfg = test_config(&temp_dir);
    let models = list_models(&cfg, None);

    assert!(models.iter().any(|m| m.qualified_id == "openrouter/free"));
    assert!(models.iter().any(|m| m.qualified_id == "openrouter/auto"));
    assert!(models.iter().any(|m| m.qualified_id == "ollama/llama3"));
    assert!(models
        .iter()
        .any(|m| m.qualified_id == "groq/llama3-8b-8192"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_query_matching_search() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_dir = std::env::temp_dir().join("gestalt_test_search_models");
    fs::create_dir_all(&temp_dir).unwrap();

    let cfg = test_config(&temp_dir);

    // Case-insensitive matching by display name
    let results1 = search_models(&cfg, "gemini");
    assert!(!results1.is_empty());
    assert!(results1.iter().any(|m| m.qualified_id == "openrouter/free"));

    // Case-insensitive matching by qualified_id
    let results2 = search_models(&cfg, "ollama");
    assert!(!results2.is_empty());
    assert!(results2.iter().any(|m| m.qualified_id == "ollama/llama3"));

    // Matching by model_id
    let results3 = search_models(&cfg, "llama3");
    assert!(!results3.is_empty());
    assert!(results3.iter().any(|m| m.qualified_id == "ollama/llama3"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_cache_loading_and_expiration() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_cache_{}", unique_id));
    fs::create_dir_all(&temp_dir).unwrap();

    let _cache_guard = EnvVarGuard::set("XDG_CACHE_HOME", &temp_dir);

    let test_model = ModelInfo {
        qualified_id: "openrouter/cached-model".to_string(),
        model_id: "cached-model".to_string(),
        display_name: "Cached Model".to_string(),
        max_context_tokens: 2048,
        max_output_tokens: 512,
        supports_tools: true,
        supports_vision: false,
        supports_json_schema: false,
        supports_thinking: false,
        supports_prompt_caching: false,
        input_cost_per_million: None,
        output_cost_per_million: None,
        source: ModelInfoSource::ProviderDiscovered,
        last_updated: Some(Utc::now().to_rfc3339()),
    };

    // 1. Save and load cache - happy path
    save_cached_models("openrouter", std::slice::from_ref(&test_model)).expect("save cache");
    let loaded = load_cached_models("openrouter").expect("load cache");
    assert!(loaded
        .iter()
        .any(|m| m.qualified_id == "openrouter/cached-model"));

    // 2. Load from list_models with provider cache
    let cfg = test_config(&temp_dir);
    let models_list = list_models(&cfg, Some("openrouter"));
    assert!(models_list
        .iter()
        .any(|m| m.qualified_id == "openrouter/cached-model"));

    // 3. Cache expiration (older than 24 hours)
    let expired_time = Utc::now() - Duration::hours(25);
    let expired_cache = CachedModels {
        last_updated: expired_time.to_rfc3339(),
        models: vec![test_model],
    };
    let cache_path = get_cache_path("openrouter");
    let content = serde_json::to_string(&expired_cache).unwrap();
    fs::write(cache_path, content).unwrap();

    // load_cached_models should return None for expired cache
    let loaded_expired = load_cached_models("openrouter");
    assert!(loaded_expired.is_none());

    // list_models should filter out expired cache
    let models_list_expired = list_models(&cfg, Some("openrouter"));
    assert!(!models_list_expired
        .iter()
        .any(|m| m.qualified_id == "openrouter/cached-model"));

    let _ = fs::remove_dir_all(&temp_dir);
}
