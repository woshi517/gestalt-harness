use crate::config::{
    global_config_path, mutate_workspace_config_file, workspace_config_path, EffectiveConfig,
};
use crate::output::{ProfilesInspectReport, ProfilesListReport, ProfilesUseReport};
use gestalt_core::{ConfigError, HarnessError};

pub fn list_profiles(config: &EffectiveConfig) -> Result<ProfilesListReport, HarnessError> {
    let active_profile = config.resolve_provider().ok().and_then(|r| r.profile_name);
    let mut profiles = std::collections::HashMap::new();

    // Built-in profiles:
    profiles.insert(
        "default".to_string(),
        ("openrouter".to_string(), "openrouter/free".to_string()),
    );
    profiles.insert(
        "openrouter".to_string(),
        ("openrouter".to_string(), "openrouter/free".to_string()),
    );
    profiles.insert(
        "anthropic".to_string(),
        (
            "anthropic".to_string(),
            "claude-3-5-sonnet-20241022".to_string(),
        ),
    );
    profiles.insert(
        "openai".to_string(),
        ("openai".to_string(), "gpt-4o-mini".to_string()),
    );
    profiles.insert(
        "ollama".to_string(),
        ("ollama".to_string(), "llama3".to_string()),
    );
    profiles.insert(
        "groq".to_string(),
        ("groq".to_string(), "llama3-8b-8192".to_string()),
    );
    profiles.insert(
        "together".to_string(),
        (
            "together".to_string(),
            "mistralai/Mixtral-8x7B-Instruct-v0.1".to_string(),
        ),
    );

    // Configured profiles override built-ins or add new ones:
    for (name, prof_cfg) in &config.profiles {
        let provider = prof_cfg.provider.clone().unwrap_or_else(|| name.clone());
        let model = prof_cfg.model.clone().unwrap_or_else(|| {
            if let Some(prov_cfg) = config.providers.get(&provider) {
                prov_cfg.default_model.clone().unwrap_or_default()
            } else if let Some(builtin) = crate::provider_catalog::get_builtin_provider(&provider) {
                builtin.default_model.clone().unwrap_or_default()
            } else {
                "unknown".to_string()
            }
        });
        profiles.insert(name.clone(), (provider, model));
    }

    let mut entries = Vec::new();
    for (name, (provider, model)) in profiles {
        let active = Some(name.clone()) == active_profile;
        entries.push(crate::output::ProfileInfoEntry {
            name,
            provider,
            model,
            active,
        });
    }

    entries.sort_by(|a, b| {
        if a.active && !b.active {
            std::cmp::Ordering::Less
        } else if !a.active && b.active {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    Ok(ProfilesListReport { profiles: entries })
}

pub fn inspect_profile(
    config: &EffectiveConfig,
    name: &str,
) -> Result<ProfilesInspectReport, HarnessError> {
    let mut temp_cfg = config.clone();
    temp_cfg.defaults.profile = Some(name.to_string());
    temp_cfg.defaults.provider = None; // clear provider override

    let resolved = temp_cfg.resolve_provider()?;
    let active_profile = config.resolve_provider().ok().and_then(|r| r.profile_name);
    let active = Some(name.to_string()) == active_profile;

    Ok(ProfilesInspectReport {
        name: name.to_string(),
        provider: resolved.provider_name,
        model: resolved.model,
        active,
        resolved_provider_kind: resolved.kind,
        resolved_base_url: resolved.base_url,
        resolved_auth_ref: resolved.auth_ref,
        resolved_api_key_env: resolved.api_key_env,
    })
}

pub fn use_profile(
    config: &EffectiveConfig,
    name: &str,
) -> Result<ProfilesUseReport, HarnessError> {
    let list = list_profiles(config)?;
    if !list.profiles.iter().any(|p| p.name == name) {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "profile".to_string(),
            reason: format!("profile '{name}' not found"),
        }));
    }

    let workspace_path = workspace_config_path(&config.workspace_root);
    let legacy_workspace_path = config.workspace_root.join(".gestalt/config.toml");
    let legacy_workspace_policies = config.workspace_root.join(".gestalt/policies.toml");
    let file_path = if workspace_path.exists()
        || legacy_workspace_path.exists()
        || legacy_workspace_policies.exists()
    {
        workspace_path
    } else {
        global_config_path()
    };

    mutate_workspace_config_file(&file_path, |ws_cfg| {
        let mut defaults = ws_cfg.defaults.clone().unwrap_or_default();
        defaults.profile = Some(name.to_string());
        ws_cfg.defaults = Some(defaults);
    })?;

    Ok(ProfilesUseReport {
        name: name.to_string(),
        active: true,
        file_updated: file_path,
    })
}
