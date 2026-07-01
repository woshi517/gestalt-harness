use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use gestalt_app::config::{
    explain_config, load_effective_config, validate_workspace_config, CliOverrides, SandboxType,
};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

// These tests mutate process-global env vars, so keep the whole file serialized.
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restores the config-related environment on drop, even if the test panics.
struct TestEnvGuard {
    vars: Vec<(&'static str, Option<String>)>,
}

impl TestEnvGuard {
    fn clear() -> Self {
        const VARS: [&str; 6] = [
            "GESTALT_PROFILE",
            "GESTALT_PROVIDER",
            "GESTALT_MODEL",
            "GESTALT_MODE",
            "GESTALT_MAX_TURNS",
            "XDG_CONFIG_HOME",
        ];
        let vars = VARS
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect::<Vec<_>>();

        std::env::remove_var("GESTALT_PROFILE");
        std::env::remove_var("GESTALT_PROVIDER");
        std::env::remove_var("GESTALT_MODEL");
        std::env::remove_var("GESTALT_MODE");
        std::env::remove_var("GESTALT_MAX_TURNS");
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");

        Self { vars }
    }

    fn set_xdg_config_home(path: &std::path::Path) -> Self {
        let original = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", path);
        Self {
            vars: vec![("XDG_CONFIG_HOME", original)],
        }
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.vars {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

#[test]
fn validate_workspace_fixture_config() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();
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
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();

    // Create temporary directories
    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_{}", unique_id));
    let global_config_dir = temp_dir.join("gestalt");
    fs::create_dir_all(&global_config_dir).unwrap();

    let workspace_dir = temp_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).unwrap();

    // Set XDG_CONFIG_HOME to temp_dir so dirs::config_dir() points to temp_dir
    let _xdg_guard = TestEnvGuard::set_xdg_config_home(&temp_dir);

    fs::write(
        global_config_dir.join("gestalt.json"),
        r#"{"version":1,"tools":{"bash_timeout_secs":99,"max_output_tokens":5000}}"#,
    )
    .unwrap();
    fs::write(
        workspace_dir.join("gestalt.json"),
        r#"{"version":1,"tools":{"sandbox_type":"docker"},"context":{"max_context_window":60000}}"#,
    )
    .unwrap();

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
    assert_eq!(config.tools.sandbox_type, Some(SandboxType::Docker));
    assert_eq!(config.context.max_context_window, Some(60000));

    // Check that default values are used for unspecified fields
    assert_eq!(config.context.reserved_output_tokens, None);
    assert_eq!(config.context.safety_margin_tokens, Some(2048));

    // 2. Explain config
    let explanation = explain_config(&overrides).expect("explain config");

    // Reported source matches actual winning value
    let bash_timeout = explanation.get("tools.bash_timeout_secs").unwrap();
    assert_eq!(bash_timeout.value, serde_json::json!(99));
    assert_eq!(bash_timeout.source, "Global Config File");
    assert_eq!(bash_timeout.winning_layer, "global");
    assert!(bash_timeout
        .source_location
        .as_deref()
        .is_some_and(|path| path.ends_with("gestalt/gestalt.json")));
    assert!(!bash_timeout.defaulted);
    assert!(bash_timeout.overridden);
    assert!(!bash_timeout.redacted);

    let sandbox_type = explanation.get("tools.sandbox_type").unwrap();
    assert_eq!(sandbox_type.value, serde_json::json!("docker"));
    assert_eq!(sandbox_type.source, "Workspace Config File");
    assert_eq!(sandbox_type.winning_layer, "workspace");
    assert!(sandbox_type.overridden);

    let max_context = explanation.get("context.max_context_window").unwrap();
    assert_eq!(max_context.value, serde_json::json!(60000));
    assert_eq!(max_context.source, "Workspace Config File");

    let reserved_tokens = explanation.get("context.reserved_output_tokens").unwrap();
    assert_eq!(reserved_tokens.value, serde_json::json!(4096));
    assert_eq!(reserved_tokens.source, "Default");
    assert_eq!(reserved_tokens.winning_layer, "default");
    assert!(reserved_tokens.defaulted);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn legacy_config_is_rejected_without_parsing() {
    let _guard = lock_env();
    let temp_dir = std::env::temp_dir().join(format!("gestalt-legacy-{}", uuid::Uuid::new_v4()));
    let _env = TestEnvGuard::set_xdg_config_home(&temp_dir);
    let workspace = temp_dir.join("workspace");
    fs::create_dir_all(workspace.join(".gestalt")).unwrap();
    fs::write(
        workspace.join(".gestalt/config.toml"),
        vec![b'x'; 1024 * 1024],
    )
    .unwrap();

    let error = load_effective_config(&CliOverrides {
        workspace: Some(workspace),
        ..CliOverrides::default()
    })
    .expect_err("legacy config must fail before parsing");

    assert!(matches!(
        error,
        gestalt_core::HarnessError::Config(
            gestalt_core::ConfigError::UnsupportedLegacyConfig { .. }
        )
    ));
}

#[test]
fn test_profile_resolution() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();
    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/profiled")),
        ..CliOverrides::default()
    })
    .expect("config validates");

    let resolved = config.resolve_provider().expect("resolve provider");
    assert_eq!(resolved.profile_name.as_deref(), Some("default"));
    assert_eq!(resolved.provider_name(), "openrouter");
    assert_eq!(
        resolved.api_format(),
        gestalt_core::ApiFormat::OpenAiChatCompletions
    );
    assert_eq!(resolved.model(), "openrouter/free");
}

#[test]
fn test_profile_cli_override() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();
    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/profiled")),
        profile: Some("anthropic".to_string()),
        ..CliOverrides::default()
    })
    .expect("config validates");

    let resolved = config.resolve_provider().expect("resolve provider");
    assert_eq!(resolved.profile_name.as_deref(), Some("anthropic"));
    assert_eq!(resolved.provider_name(), "anthropic");
    assert_eq!(
        resolved.api_format(),
        gestalt_core::ApiFormat::AnthropicMessages
    );
    assert_eq!(resolved.model(), "claude-3-5-sonnet-20241022");
}

#[test]
fn test_provider_model_cli_overrides_beat_profile() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();
    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/profiled")),
        provider: Some("openai".to_string()),
        model: Some("gpt-4o-custom".to_string()),
        ..CliOverrides::default()
    })
    .expect("config validates");

    let resolved = config.resolve_provider().expect("resolve provider");
    assert_eq!(resolved.profile_name.as_deref(), Some("default"));
    assert_eq!(resolved.provider_name(), "openai");
    assert_eq!(
        resolved.api_format(),
        gestalt_core::ApiFormat::OpenAiResponses
    );
    assert_eq!(resolved.model(), "gpt-4o-custom");
}

#[test]
fn test_policy_monotonicity_enforcement() {
    let _guard = lock_env();
    use gestalt_app::config::WorkspaceConfig;
    let global: WorkspaceConfig =
        serde_json::from_str(r#"{"version":1,"policies":{"paths":{"allow_read":["/a","/b"]}}}"#)
            .unwrap();
    let workspace: WorkspaceConfig =
        serde_json::from_str(r#"{"version":1,"policies":{"paths":{"allow_read":["/a","/c"]}}}"#)
            .unwrap();

    let merge_res = global.merge(workspace);
    assert!(merge_res.is_err());
    let err = merge_res.unwrap_err();
    assert!(
        err.to_string()
            .contains("workspace policy tries to widen authority"),
        "expected widening error, got: {}",
        err
    );
}

#[test]
fn test_policy_deny_union_merge() {
    let _guard = lock_env();
    use gestalt_app::config::WorkspaceConfig;
    let global: WorkspaceConfig =
        serde_json::from_str(r#"{"version":1,"policies":{"paths":{"deny_read":["/secret1"]}}}"#)
            .unwrap();
    let workspace: WorkspaceConfig =
        serde_json::from_str(r#"{"version":1,"policies":{"paths":{"deny_read":["/secret2"]}}}"#)
            .unwrap();

    let merged = global.merge(workspace).expect("merge succeeds");
    let paths = merged.policies.unwrap().paths;
    let deny_read = paths.deny_read.unwrap();
    assert!(deny_read.contains(&"/secret1".to_string()));
    assert!(deny_read.contains(&"/secret2".to_string()));
    assert_eq!(deny_read.len(), 2);
}

#[test]
fn test_extension_instances_parse_with_snake_case_grants() {
    let _guard = lock_env();
    use gestalt_app::config::WorkspaceConfig;

    let json = r#"
{
  "version": 1,
  "extensions": {
    "explicit_loads": ["./legacy"],
    "disabled": ["old-ext"],
    "trusted": ["legacy-ext"],
    "allow_untrusted": true,
    "instances": {
      "review-primary": {
        "package": "com.example.review",
      "enabled": true,
      "components": {
        "lifecycle": true,
        "client-metadata": false
      },
      "config": {
        "policySet": "default"
      },
      "grants": {
          "workspace_read": true,
          "workspace_write": false,
          "network": ["api.example.com"]
      }
    }
  }
}
}
"#;

    let config: WorkspaceConfig = serde_json::from_str(json).unwrap();
    let extensions = config.extensions.unwrap();
    let instance = extensions.instances.get("review-primary").unwrap();

    assert_eq!(extensions.explicit_loads, ["./legacy"]);
    assert_eq!(extensions.disabled, ["old-ext"]);
    assert_eq!(extensions.trusted, ["legacy-ext"]);
    assert!(extensions.allow_untrusted);
    assert_eq!(instance.package, "com.example.review");
    assert!(instance.enabled);
    assert_eq!(instance.components.get("lifecycle"), Some(&true));
    assert_eq!(instance.components.get("client-metadata"), Some(&false));
    assert_eq!(instance.config["policySet"], "default");
    assert!(instance.grants.workspace_read);
    assert!(!instance.grants.workspace_write);
    assert_eq!(instance.grants.network, ["api.example.com"]);
}

#[test]
fn legacy_extension_grant_aliases_are_rejected() {
    let _guard = lock_env();
    use gestalt_app::config::WorkspaceConfig;

    let json = r#"
{
  "version": 1,
  "extensions": {
    "instances": {
      "review-primary": {
        "package": "com.example.review",
        "grants": {
          "workspaceRead": true
        }
      }
    }
  }
}
"#;

    assert!(serde_json::from_str::<WorkspaceConfig>(json).is_err());
}

#[test]
fn legacy_secret_auth_ref_is_rejected() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_secret_{}", unique_id));
    let workspace_dir = temp_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).unwrap();
    fs::write(
        workspace_dir.join("gestalt.json"),
        r#"{
          "version": 1,
          "providers": {
            "openai": {
              "auth_ref": "secret:provider/openai"
            }
          }
        }"#,
    )
    .unwrap();

    let error = load_effective_config(&CliOverrides {
        workspace: Some(workspace_dir.clone()),
        ..CliOverrides::default()
    })
    .expect_err("legacy secret auth_ref must be rejected");

    assert!(error
        .to_string()
        .contains("legacy secret: syntax is not supported"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn provider_kind_is_rejected_as_unknown() {
    let _guard = lock_env();
    use gestalt_app::config::WorkspaceConfig;

    let json = r#"
{
  "version": 1,
  "providers": {
    "openai": {
      "kind": "openai"
    }
  }
}
"#;

    assert!(serde_json::from_str::<WorkspaceConfig>(json).is_err());
}

#[test]
fn explain_config_reports_leaf_provenance_and_redacts_secrets() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_explain_{}", unique_id));
    let global_config_dir = temp_dir.join("gestalt");
    let workspace_dir = temp_dir.join("workspace");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::create_dir_all(&workspace_dir).unwrap();
    let _xdg_guard = TestEnvGuard::set_xdg_config_home(&temp_dir);
    fs::write(
        global_config_dir.join("gestalt.json"),
        r#"{
          "version": 1,
          "providers": {
            "openai": {
              "base_url": "https://global.example/v1"
            }
          }
        }"#,
    )
    .unwrap();
    fs::write(
        workspace_dir.join("gestalt.json"),
        r#"{
          "version": 1,
          "providers": {
            "openai": {
              "auth_ref": "keychain:gestalt/openai"
            }
          }
        }"#,
    )
    .unwrap();

    let explanation = explain_config(&CliOverrides {
        workspace: Some(workspace_dir.clone()),
        ..CliOverrides::default()
    })
    .expect("explain config");

    let auth_ref = explanation
        .get("providers.openai.auth_ref")
        .expect("auth_ref entry");
    assert_eq!(auth_ref.winning_layer, "workspace");
    assert!(auth_ref.redacted);
    assert_eq!(auth_ref.value, serde_json::json!("[REDACTED]"));
    assert_eq!(
        explanation["providers.openai.base_url"].winning_layer,
        "global"
    );
    assert_eq!(explanation["providers.openai.id"].winning_layer, "default");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_extension_instances_merge_additively() {
    let _guard = lock_env();
    use gestalt_app::config::WorkspaceConfig;

    let global: WorkspaceConfig = serde_json::from_str(
        r#"
{
  "version": 1,
  "extensions": {
    "instances": {
      "global-review": {
        "package": "com.example.review",
        "enabled": true
      }
    }
  }
}
"#,
    )
    .unwrap();
    let workspace: WorkspaceConfig = serde_json::from_str(
        r#"
{
  "version": 1,
  "extensions": {
    "instances": {
      "workspace-review": {
        "package": "com.example.review",
        "enabled": false
      }
    }
  }
}
"#,
    )
    .unwrap();

    let merged = global.merge(workspace).unwrap();
    let instances = merged.extensions.unwrap().instances;

    assert!(instances.contains_key("global-review"));
    assert!(instances.contains_key("workspace-review"));
    assert!(!instances["workspace-review"].enabled);
}

#[test]
fn test_effective_config_fingerprint_stability() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();

    let config1 = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/minimal")),
        ..CliOverrides::default()
    })
    .expect("config validates");

    let config2 = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/minimal")),
        ..CliOverrides::default()
    })
    .expect("config validates");

    let fp1 = config1.compute_fingerprint();
    let fp2 = config2.compute_fingerprint();

    assert!(!fp1.is_empty());
    assert_eq!(
        fp1, fp2,
        "Effective config fingerprints must be stable across repeated loads"
    );
}

#[test]
fn test_variant_fingerprint_changes() {
    let _guard = lock_env();
    let fp1 = gestalt_runtime::unstable::inspect::compute_variant_fingerprint(
        "model-a",
        "provider-a",
        1000,
        Some(0.7),
        Some(0.9),
        Some(&gestalt_core::provider::ReasoningEffort::Low),
        Some(&gestalt_core::provider::TextVerbosity::Medium),
    );

    let fp2 = gestalt_runtime::unstable::inspect::compute_variant_fingerprint(
        "model-a",
        "provider-a",
        1000,
        Some(0.7),
        Some(0.9),
        Some(&gestalt_core::provider::ReasoningEffort::High),
        Some(&gestalt_core::provider::TextVerbosity::Medium),
    );

    let fp3 = gestalt_runtime::unstable::inspect::compute_variant_fingerprint(
        "model-a",
        "provider-a",
        1000,
        Some(0.7),
        Some(0.9),
        Some(&gestalt_core::provider::ReasoningEffort::Low),
        Some(&gestalt_core::provider::TextVerbosity::High),
    );

    assert_ne!(
        fp1, fp2,
        "Variant fingerprint must change when reasoning effort changes"
    );
    assert_ne!(
        fp1, fp3,
        "Variant fingerprint must change when text verbosity changes"
    );
}

#[test]
fn test_structured_config_validation_and_precedence() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_{}", unique_id));
    let workspace_dir = temp_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).unwrap();
    fs::write(
        workspace_dir.join("gestalt.json"),
        r#"{"version":1,"context":{"workspace":{"path":".gestalt/structured_workspace.md"}}}"#,
    )
    .unwrap();

    let overrides = CliOverrides {
        workspace: Some(workspace_dir.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).expect("load config");
    assert_eq!(
        config
            .context
            .workspace
            .as_ref()
            .unwrap()
            .path
            .as_ref()
            .unwrap(),
        &PathBuf::from(".gestalt/structured_workspace.md")
    );

    fs::write(
        workspace_dir.join("gestalt.json"),
        r#"{"version":1,"context":{"workspace":{"enabled":false,"required":true}}}"#,
    )
    .unwrap();
    let config3 = load_effective_config(&overrides);
    assert!(
        config3.is_err(),
        "enabled=false combined with required=true must fail"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_reasoning_and_thinking_validation() {
    let _guard = lock_env();
    let _env = TestEnvGuard::clear();

    let unique_id = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("gestalt_test_val_{}", unique_id));
    let workspace_dir = temp_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).unwrap();

    let overrides = CliOverrides {
        workspace: Some(workspace_dir.clone()),
        ..CliOverrides::default()
    };

    // 1. gpt-4o-mini (reasoning = false) with reasoning_effort should fail
    let invalid_reasoning = r#"{
      "version":1,
      "defaults":{"model":"gpt-4o-mini","provider":"openai"},
      "providers":{"openai":{"models":{"gpt-4o-mini":{"options":{"reasoning_effort":"high"}}}}}
    }"#;
    fs::write(workspace_dir.join("gestalt.json"), invalid_reasoning).unwrap();
    let res = load_effective_config(&overrides).and_then(|config| config.resolve_provider());
    assert!(
        res.is_err(),
        "reasoning_effort on non-reasoning model must fail"
    );

    // 2. claude-3-5-sonnet (reasoning = false) with thinking should fail
    let invalid_thinking = r#"{
      "version":1,
      "defaults":{"model":"claude-3-5-sonnet","provider":"anthropic"},
      "providers":{"anthropic":{"models":{"claude-3-5-sonnet":{"options":{"thinking":{"type":"enabled","budget_tokens":1024}}}}}}
    }"#;
    fs::write(workspace_dir.join("gestalt.json"), invalid_thinking).unwrap();
    let res2 = load_effective_config(&overrides).and_then(|config| config.resolve_provider());
    assert!(
        res2.is_err(),
        "thinking on non-reasoning Anthropic model must fail"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}
