use gestalt_cli::auth::set_use_fake_keychain;
use gestalt_cli::config::{load_effective_config, CliOverrides};
use gestalt_cli::connect::{connect_provider, disconnect_provider};
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
fn test_connect_openrouter() {
    let _guard = ENV_MUTEX.lock().unwrap();
    set_use_fake_keychain(true);

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_connect_{}", unique_id));
    fs::create_dir_all(&temp_dir).unwrap();

    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &temp_dir);

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        ..CliOverrides::default()
    };

    let config = load_effective_config(&overrides).expect("load config");

    let report = connect_provider(
        &config,
        "openrouter",
        Some("sk-or-test-key".to_string()),
        false,
        true,
        None,
        None,
        None,
        None,
    )
    .expect("connect succeeds");

    assert_eq!(report.provider, "openrouter");
    assert!(report.keychain_stored);
    assert_eq!(report.profile_created.as_deref(), Some("default"));

    // Reload effective config to verify provider and profile are present
    let config2 = load_effective_config(&overrides).expect("reload config");
    let resolved = config2.resolve_provider().expect("resolve provider");
    assert_eq!(resolved.provider_name, "openrouter");
    assert_eq!(resolved.model, "openrouter/free");
    assert_eq!(
        resolved.auth_ref.as_deref(),
        Some("secret:provider/openrouter")
    );

    // Verify disconnect
    let dis_report =
        disconnect_provider(&config2, "openrouter", true).expect("disconnect succeeds");
    assert_eq!(dis_report.provider, "openrouter");
    assert_eq!(dis_report.profile_removed.as_deref(), Some("default"));

    let config3 = load_effective_config(&overrides).expect("reload config");
    let resolved3 = config3.resolve_provider().expect("resolve provider");
    assert_eq!(resolved3.provider_name, "openrouter");
    assert!(resolved3.auth_ref.is_none());

    let _ = fs::remove_dir_all(&temp_dir);
}
