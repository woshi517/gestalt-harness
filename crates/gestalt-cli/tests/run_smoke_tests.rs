use async_trait::async_trait;
use gestalt_cli::config::{validate_workspace_config, CliOverrides};
use gestalt_core::{
    event::AgentEvent,
    message::Message,
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    HarnessError,
};
use std::sync::Arc;

struct MockProvider {
    capabilities: ProviderCapabilities,
}

impl MockProvider {
    fn new() -> Self {
        Self {
            capabilities: ProviderCapabilities {
                supports_tools: true,
                supports_parallel_tools: true,
                supports_vision: false,
                supports_documents: false,
                supports_thinking: false,
                supports_json_schema_tools: true,
                supports_prompt_caching: false,
                supports_usage_reporting: false,
                supports_streaming: true,
            },
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        "mock-provider"
    }

    fn display_name(&self) -> &str {
        "Mock Provider"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(&self, _model: &str, _messages: &[Message]) -> Result<usize, HarnessError> {
        Ok(0)
    }

    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, HarnessError> {
        let events = vec![AgentEvent::Stop {
            reason: gestalt_core::event::StopReason::EndTurn,
        }];
        let stream = futures::stream::iter(events.into_iter().map(Ok::<_, HarnessError>));
        Ok(Box::pin(stream))
    }
}

fn copy_minimal_workspace(dest: &std::path::Path) {
    let src = std::path::Path::new("../../tests/fixtures/workspaces/minimal");
    let src_gestalt = src.join(".gestalt");
    let dest_gestalt = dest.join(".gestalt");
    std::fs::create_dir_all(&dest_gestalt).unwrap();

    for entry in std::fs::read_dir(&src_gestalt).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        std::fs::copy(src_gestalt.join(&name), dest_gestalt.join(&name)).unwrap();
    }
}

#[tokio::test]
async fn test_cli_smoke_prompt_source() {
    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    // Create a temporary workspace based on tests/fixtures/workspaces/minimal
    let temp_dir = std::env::temp_dir().join(format!("gestalt-cli-smoke-{}", uuid::Uuid::new_v4()));
    copy_minimal_workspace(&temp_dir);

    let gestalt_dir = temp_dir.join(".gestalt");

    // Overwrite config.toml in the copied workspace to use our mock provider
    let config_toml = r#"
[defaults]
profile = "mock-profile"
provider = "mock-provider"
model = "mock-model"
mode = "confirm"
max_turns = 1

[profiles.mock-profile]
provider = "mock-provider"
model = "mock-model"
"#;
    std::fs::write(gestalt_dir.join("config.toml"), config_toml).unwrap();

    // 1. With policies.toml prompt override -> prompt_source should be "override"
    let policies_toml = r#"
[paths]
allow_read  = [".", "sources/", "docs/", "src/"]
allow_write = ["docs/", ".gestalt/"]
deny_write  = [".git/", "secrets/", ".env", "*.key"]

[tools.bash]
default      = "confirm"
yolo_allow   = ["ls", "cat", "grep", "rg", "find"]
always_deny  = ["dd", "mkfs", "fdisk"]

[network]
default = "confirm"

[prompt]
override = "Smoke test override prompt"
"#;
    std::fs::write(gestalt_dir.join("policies.toml"), policies_toml).unwrap();

    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(temp_dir.clone()),
        ..CliOverrides::default()
    })
    .unwrap();

    let log_dir = gestalt_cli::run::run_prompt(
        &config,
        "run smoke",
        None,
        gestalt_core::CancelToken::new(),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let trace_file = log_dir.join("trace.jsonl");
    let content = std::fs::read_to_string(trace_file).unwrap();

    let mut found_override = false;
    for line in content.lines() {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(event) = val.get("event") {
                if event.get("type").and_then(|v| v.as_str()) == Some("context_built") {
                    let prompt_src = event.get("prompt_source").and_then(|v| v.as_str());
                    if prompt_src == Some("override") {
                        found_override = true;
                    }
                }
            }
        }
    }
    if !found_override {
        println!("TRACE CONTENT:\n{}", content);
    }
    assert!(
        found_override,
        "Should record prompt_source as override in trace"
    );

    // 2. Remove override from policies.toml -> prompt_source should be "default"
    let policies_toml_no_override = r#"
[paths]
allow_read  = [".", "sources/", "docs/", "src/"]
allow_write = ["docs/", ".gestalt/"]
deny_write  = [".git/", "secrets/", ".env", "*.key"]

[tools.bash]
default      = "confirm"
yolo_allow   = ["ls", "cat", "grep", "rg", "find"]
always_deny  = ["dd", "mkfs", "fdisk"]

[network]
default = "confirm"
"#;
    std::fs::write(gestalt_dir.join("policies.toml"), policies_toml_no_override).unwrap();

    let config2 = validate_workspace_config(&CliOverrides {
        workspace: Some(temp_dir.clone()),
        ..CliOverrides::default()
    })
    .unwrap();

    let log_dir2 = gestalt_cli::run::run_prompt(
        &config2,
        "run smoke",
        None,
        gestalt_core::CancelToken::new(),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let trace_file2 = log_dir2.join("trace.jsonl");
    let content2 = std::fs::read_to_string(trace_file2).unwrap();

    let mut found_default = false;
    for line in content2.lines() {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(event) = val.get("event") {
                if event.get("type").and_then(|v| v.as_str()) == Some("context_built") {
                    let prompt_src = event.get("prompt_source").and_then(|v| v.as_str());
                    if prompt_src == Some("default") {
                        found_default = true;
                    }
                }
            }
        }
    }
    assert!(
        found_default,
        "Should record prompt_source as default in trace when no override"
    );

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cli_smoke_custom_provider_via_profile() {
    let _ = gestalt_models::registry::register(
        "custom-mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_dir = std::env::temp_dir().join(format!(
        "gestalt-cli-smoke-profile-{}",
        uuid::Uuid::new_v4()
    ));
    copy_minimal_workspace(&temp_dir);

    let gestalt_dir = temp_dir.join(".gestalt");

    // Configure a custom profile pointing to our custom mock provider connection
    let config_toml = r#"
[defaults]
profile = "custom-profile"

[profiles.custom-profile]
provider = "custom-mock"

[providers.custom-mock]
kind = "custom-mock-provider"
default_model = "mock-model"
"#;
    std::fs::write(gestalt_dir.join("config.toml"), config_toml).unwrap();

    let policies_toml = r#"
[paths]
allow_read  = [".", "sources/", "docs/", "src/"]
allow_write = ["docs/", ".gestalt/"]
deny_write  = [".git/", "secrets/", ".env", "*.key"]

[tools.bash]
default      = "confirm"
yolo_allow   = ["ls", "cat", "grep", "rg", "find"]
always_deny  = ["dd", "mkfs", "fdisk"]

[network]
default = "confirm"
"#;
    std::fs::write(gestalt_dir.join("policies.toml"), policies_toml).unwrap();

    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(temp_dir.clone()),
        ..CliOverrides::default()
    })
    .unwrap();

    // Verify it resolves kind to custom-mock-provider
    let resolved = config.resolve_provider().unwrap();
    assert_eq!(resolved.provider_name, "custom-mock");
    assert_eq!(resolved.kind, "custom-mock-provider");
    assert_eq!(resolved.model, "mock-model");

    // Execute run_prompt to verify the loop runs
    let log_dir = gestalt_cli::run::run_prompt(
        &config,
        "run smoke",
        None,
        gestalt_core::CancelToken::new(),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert!(log_dir.join("trace.jsonl").exists());

    let _ = std::fs::remove_dir_all(&temp_dir);
}
