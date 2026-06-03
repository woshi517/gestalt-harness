use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use gestalt_cli::config::{validate_workspace_config, explain_config, load_effective_config, CliOverrides};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Restores an environment variable on drop, even if the test panics.
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
fn validate_workspace_fixture_config() {
    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/minimal")),
        ..CliOverrides::default()
    })
    .expect("config validates");

    assert_eq!(config.selected_provider().expect("provider"), "anthropic");
    assert_eq!(
        config.selected_model().as_deref(),
        Some("claude-sonnet-4-6")
    );
}

#[test]
fn test_config_precedence_and_sources() {
    let _guard = ENV_MUTEX.lock().unwrap();
    
    // Create temporary directories
    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_{}", unique_id));
    let global_config_dir = temp_dir.join("gestalt");
    fs::create_dir_all(&global_config_dir).unwrap();
    
    let workspace_dir = temp_dir.join("workspace");
    let workspace_config_dir = workspace_dir.join(".gestalt");
    fs::create_dir_all(&workspace_config_dir).unwrap();
    
    // Set XDG_CONFIG_HOME to temp_dir so dirs::config_dir() points to temp_dir
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &temp_dir);
    
    // Write global config.toml - defines [tools] but NOT [context]
    let global_toml = r#"
[tools]
bash_timeout_secs = 99
max_output_tokens = 5000
"#;
    fs::write(global_config_dir.join("config.toml"), global_toml).unwrap();
    
    // Write workspace config.toml - defines [context] and overrides parts of [tools]
    let workspace_toml = r#"
[tools]
sandbox_type = "docker"

[context]
max_context_window = 60000
"#;
    fs::write(workspace_config_dir.join("config.toml"), workspace_toml).unwrap();
    
    let overrides = CliOverrides {
        workspace: Some(workspace_dir.clone()),
        ..CliOverrides::default()
    };
    
    // 1. Load effective config
    let config = load_effective_config(&overrides).expect("load config");
    
    // Check that global values survive when omitted in workspace
    assert_eq!(config.tools.bash_timeout_secs, Some(99));
    assert_eq!(config.tools.max_output_tokens, Some(5000));
    
    // Check that workspace overrides/extends global values
    assert_eq!(config.tools.sandbox_type.as_deref(), Some("docker"));
    assert_eq!(config.context.max_context_window, Some(60000));
    
    // Check that default values are used for unspecified fields
    assert_eq!(config.context.reserved_output_tokens, Some(8000));
    
    // 2. Explain config
    let explanation = explain_config(&overrides).expect("explain config");
    
    // Reported source matches actual winning value
    let bash_timeout = explanation.get("tools.bash_timeout_secs").unwrap();
    assert_eq!(bash_timeout.value, serde_json::json!(99));
    assert_eq!(bash_timeout.source, "Global Config File");
    
    let sandbox_type = explanation.get("tools.sandbox_type").unwrap();
    assert_eq!(sandbox_type.value, serde_json::json!("docker"));
    assert_eq!(sandbox_type.source, "Workspace Config File");
    
    let max_context = explanation.get("context.max_context_window").unwrap();
    assert_eq!(max_context.value, serde_json::json!(60000));
    assert_eq!(max_context.source, "Workspace Config File");
    
    let reserved_tokens = explanation.get("context.reserved_output_tokens").unwrap();
    assert_eq!(reserved_tokens.value, serde_json::json!(8000));
    assert_eq!(reserved_tokens.source, "Default");
    
    let _ = fs::remove_dir_all(&temp_dir);
}
