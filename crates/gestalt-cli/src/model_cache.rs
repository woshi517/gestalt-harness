use std::fs;
use std::path::PathBuf;
use gestalt_core::model::ModelInfo;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Serialize, Deserialize)]
pub struct CachedModels {
    pub last_updated: String,
    pub models: Vec<ModelInfo>,
}

pub fn get_cache_path(provider: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("gestalt/models")
        .join(format!("{}.json", provider))
}

pub fn load_cached_models(provider: &str) -> Option<Vec<ModelInfo>> {
    let path = get_cache_path(provider);
    if !path.exists() {
        return None;
    }
    
    let content = fs::read_to_string(&path).ok()?;
    let cached: CachedModels = serde_json::from_str(&content).ok()?;
    
    let last_updated = DateTime::parse_from_rfc3339(&cached.last_updated).ok()?.with_timezone(&Utc);
    let now = Utc::now();
    if now.signed_duration_since(last_updated).num_hours() >= 24 {
        return None;
    }
    
    Some(cached.models)
}

pub fn save_cached_models(provider: &str, models: &[ModelInfo]) -> Result<(), std::io::Error> {
    let path = get_cache_path(provider);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    let cached = CachedModels {
        last_updated: Utc::now().to_rfc3339(),
        models: models.to_vec(),
    };
    
    let content = serde_json::to_string_pretty(&cached).map_err(|e| std::io::Error::other(e))?;
    fs::write(path, content)?;
    Ok(())
}
