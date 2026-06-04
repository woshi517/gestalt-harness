use async_trait::async_trait;
use gestalt_cli::config::CliOverrides;
use gestalt_cli::runtime::inspect_runtime;
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
    let src = std::path::Path::new("tests/fixtures/workspaces/minimal");
    if !src.exists() {
        // Fallback for different working directories in tests
        let alt_src = std::path::Path::new("../../tests/fixtures/workspaces/minimal");
        if alt_src.exists() {
            copy_dir_recursive(alt_src, dest);
            return;
        }
    } else {
        copy_dir_recursive(src, dest);
    }
}

fn copy_dir_recursive(src: &std::path::Path, dest: &std::path::Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            copy_dir_recursive(&path, &dest.join(name));
        } else {
            std::fs::copy(&path, &dest.join(name)).unwrap();
        }
    }
}

#[tokio::test]
async fn test_inspect_runtime_cli() {
    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_dir = std::env::temp_dir().join(format!("gestalt-cli-inspect-{}", uuid::Uuid::new_v4()));
    copy_minimal_workspace(&temp_dir);

    // Overwrite config.toml in the copied workspace to use our mock provider
    let config_toml = r#"
[defaults]
profile = "mock-profile"
provider = "mock-provider"
model = "mock-model"
mode = "confirm"
max_turns = 12

[profiles.mock-profile]
provider = "mock-provider"
model = "mock-model"
"#;
    std::fs::create_dir_all(temp_dir.join(".gestalt")).unwrap();
    std::fs::write(temp_dir.join(".gestalt/config.toml"), config_toml).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        model: None,
        mode: None,
        max_turns: None,
        provider: None,
        profile: Some("mock-profile".to_string()),
    };

    let inspect = inspect_runtime(&overrides, None).unwrap();
    assert_eq!(inspect.provider_name, "mock-provider");
    assert_eq!(inspect.provider_model, "mock-model");
    assert_eq!(inspect.execution_mode, "Confirm");
    assert_eq!(inspect.max_turns, 12);
    assert!(!inspect.tools.is_empty());
    assert!(inspect.verifiers.contains(&"FileExistsVerifier".to_string()));
    assert!(inspect.hooks.contains(&"VerificationToolHook".to_string()));

    // Clean up
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn test_runtime_inspect_cli_subcommand() {
    let temp_dir = std::env::temp_dir().join(format!("gestalt-cli-inspect-sub-{}", uuid::Uuid::new_v4()));
    copy_minimal_workspace(&temp_dir);

    // Overwrite config.toml in the copied workspace to use a standard provider
    let config_toml = r#"
[defaults]
profile = "openai-profile"
provider = "openai"
model = "gpt-4o"

[profiles.openai-profile]
provider = "openai"
model = "gpt-4o"
"#;
    std::fs::create_dir_all(temp_dir.join(".gestalt")).unwrap();
    std::fs::write(temp_dir.join(".gestalt/config.toml"), config_toml).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .arg("--workspace")
        .arg(&temp_dir)
        .arg("--format")
        .arg("json")
        .arg("runtime")
        .arg("inspect")
        .output()
        .unwrap();

    assert!(output.status.success(), "CLI runtime inspect command should succeed: {}", String::from_utf8_lossy(&output.stderr));
    let stdout_str = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout_str).unwrap();
    assert_eq!(json["kind"], "runtime.inspect");
    assert_eq!(json["data"]["inspect"]["provider_name"], "openai");
    assert_eq!(json["data"]["inspect"]["provider_model"], "gpt-4o");
    
    // Also assert that the trace sink kind matches (JsonlTraceSink) and hooks list contains EvaluatorHook
    let inspect = &json["data"]["inspect"];
    assert_eq!(inspect["trace_sink_kind"].as_str(), Some("JsonlTraceSink"));
    let hooks = inspect["hooks"].as_array().expect("hooks should be an array");
    let has_evaluator_hook = hooks.iter().any(|h| h.as_str() == Some("EvaluatorHook"));
    assert!(has_evaluator_hook, "inspect report should contain EvaluatorHook");

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}
