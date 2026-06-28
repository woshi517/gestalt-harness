use gestalt_app::config::{EffectiveConfig, SecretString};
use gestalt_cli::output::{CliReport, ConfigShowReport};

#[test]
fn test_config_show_redaction() {
    let mut config = EffectiveConfig::default();
    let mut prov = gestalt_app::config::ProviderConfig::default();
    prov.api_key = Some(SecretString("sk-ant-test-secret".to_string()));
    config.providers.insert("openai".to_string(), prov);

    let report = ConfigShowReport {
        config,
        source: false,
        explain_map: None,
    };

    // Test text mode
    let text = report.render_text();
    assert!(!text.contains("sk-ant-test-secret"));
    assert!(text.contains("[REDACTED]"));

    // Test JSON mode
    let json_val = serde_json::to_value(&report).unwrap();
    let json_str = json_val.to_string();
    assert!(!json_str.contains("sk-ant-test-secret"));
    assert!(json_str.contains("[REDACTED]"));
}
