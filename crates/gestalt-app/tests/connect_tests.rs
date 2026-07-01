use gestalt_app::auth::set_use_fake_keychain;
use gestalt_app::config::{load_effective_config, CliOverrides};
use gestalt_app::connect::{connect_provider, disconnect_provider};
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
    let _guard = ENV_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        None,
    )
    .expect("connect succeeds");

    assert_eq!(report.provider, "openrouter");
    assert!(report.keychain_stored);
    assert_eq!(report.profile_created.as_deref(), Some("default"));

    // Reload effective config to verify provider and profile are present
    let config2 = load_effective_config(&overrides).expect("reload config");
    let resolved = config2.resolve_provider().expect("resolve provider");
    assert_eq!(resolved.provider_name(), "openrouter");
    assert_eq!(resolved.model(), "openrouter/free");
    assert_eq!(
        resolved.auth.credential_ref(),
        gestalt_runtime::unstable::auth::CredentialRef::Keychain("gestalt/openrouter".to_string())
    );

    // Verify disconnect
    let dis_report =
        disconnect_provider(&config2, "openrouter", true).expect("disconnect succeeds");
    assert_eq!(dis_report.provider, "openrouter");
    assert_eq!(dis_report.profile_removed.as_deref(), Some("default"));

    let config3 = load_effective_config(&overrides).expect("reload config");
    let resolved3 = config3.resolve_provider().expect("resolve provider");
    assert_eq!(resolved3.provider_name(), "openrouter");
    assert!(matches!(
        resolved3.auth.credential_ref(),
        gestalt_runtime::unstable::auth::CredentialRef::Environment(_)
    ));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_connect_rejects_legacy_global_config() {
    let _guard = ENV_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_use_fake_keychain(true);

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_connect_legacy_{}", unique_id));
    fs::create_dir_all(&temp_dir).unwrap();

    let global_dir = temp_dir.join("global");
    let legacy_global_dir = global_dir.join("gestalt");
    fs::create_dir_all(&legacy_global_dir).unwrap();

    fs::write(
        legacy_global_dir.join("config.toml"),
        r#"
[defaults]
provider = "openai"
model = "gpt-4o"
"#,
    )
    .unwrap();

    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &global_dir);

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        ..CliOverrides::default()
    };

    let error = load_effective_config(&overrides).expect_err("legacy config must be rejected");

    assert!(matches!(
        error,
        gestalt_core::HarnessError::Config(
            gestalt_core::ConfigError::UnsupportedLegacyConfig { .. }
        )
    ));

    let _ = fs::remove_dir_all(&temp_dir);
}
