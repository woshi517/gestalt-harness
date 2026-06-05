use gestalt_cli::config::{load_effective_config, CliOverrides};
use gestalt_cli::profiles::{inspect_profile, list_profiles, use_profile};
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

#[test]
fn test_profiles_management() {
    let _guard = ENV_MUTEX.lock().unwrap();

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_profiles_{}", unique_id));
    fs::create_dir_all(&temp_dir).unwrap();

    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &temp_dir);

    let config_dir = temp_dir.join("gestalt");
    fs::create_dir_all(&config_dir).unwrap();
    let initial_toml = r#"
[defaults]
profile = "openai"

[profiles.openai]
provider = "openai"
model = "gpt-4o"
"#;
    fs::write(config_dir.join("config.toml"), initial_toml).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        ..CliOverrides::default()
    };

    let config = load_effective_config(&overrides).expect("load config");

    let list = list_profiles(&config).expect("list profiles");
    assert!(list.profiles.iter().any(|p| p.name == "openai"));
    assert!(list.profiles.iter().any(|p| p.name == "anthropic"));

    let openai_profile = list.profiles.iter().find(|p| p.name == "openai").unwrap();
    assert!(openai_profile.active);
    assert_eq!(openai_profile.model, "gpt-4o");

    let inspect = inspect_profile(&config, "openai").expect("inspect profile");
    assert_eq!(inspect.provider, "openai");
    assert_eq!(inspect.model, "gpt-4o");
    assert_eq!(inspect.resolved_provider_kind, "openai");

    let use_report = use_profile(&config, "anthropic").expect("use profile succeeds");
    assert_eq!(use_report.name, "anthropic");

    let config2 = load_effective_config(&overrides).expect("reload config");
    let list2 = list_profiles(&config2).expect("list profiles");
    let anthropic_profile = list2
        .profiles
        .iter()
        .find(|p| p.name == "anthropic")
        .unwrap();
    assert!(anthropic_profile.active);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_profiles_use_workspace_config() {
    let _guard = ENV_MUTEX.lock().unwrap();

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_profiles_ws_{}", unique_id));
    fs::create_dir_all(&temp_dir).unwrap();

    let global_dir = temp_dir.join("global");
    let workspace_dir = temp_dir.join("workspace");
    let ws_gestalt_dir = workspace_dir.join(".gestalt");
    fs::create_dir_all(&global_dir).unwrap();
    fs::create_dir_all(&ws_gestalt_dir).unwrap();

    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &global_dir);

    // Write initial workspace config
    let initial_toml = r#"
[defaults]
profile = "openai"

[profiles.openai]
provider = "openai"
model = "gpt-4o"
"#;
    let ws_config_path = ws_gestalt_dir.join("config.toml");
    fs::write(&ws_config_path, initial_toml).unwrap();

    let overrides = CliOverrides {
        workspace: Some(workspace_dir.clone()),
        ..CliOverrides::default()
    };

    let config = load_effective_config(&overrides).expect("load config");

    // Call use_profile to switch to "anthropic"
    let use_report = use_profile(&config, "anthropic").expect("use profile succeeds");
    assert_eq!(use_report.name, "anthropic");
    assert_eq!(use_report.file_updated, ws_config_path);

    // Verify it updated the workspace config, and did NOT create a global config
    let global_config_path = global_dir.join("gestalt/config.toml");
    assert!(!global_config_path.exists());

    let ws_content = fs::read_to_string(&ws_config_path).unwrap();
    assert!(ws_content.contains("profile = \"anthropic\""));

    let _ = fs::remove_dir_all(&temp_dir);
}
