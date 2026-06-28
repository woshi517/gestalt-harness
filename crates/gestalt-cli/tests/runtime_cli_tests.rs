use async_trait::async_trait;
use gestalt_app::config::CliOverrides;
use gestalt_app::runtime_factory::inspect_runtime;
use gestalt_core::{
    event::AgentEvent,
    message::Message,
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    HarnessError,
};
use gestalt_runtime as gestalt_models;
use serde_json::{json, Value};
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
                supports_strict_schema: false,
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
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/workspaces/minimal");
    copy_dir_recursive(&src, dest);
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
            std::fs::copy(&path, dest.join(name)).unwrap();
        }
    }
}

fn update_workspace_config<F>(workspace: &std::path::Path, update: F)
where
    F: FnOnce(&mut Value),
{
    let path = workspace.join("gestalt.json");
    let input = std::fs::read_to_string(&path).unwrap();
    let mut json: Value = serde_json::from_str(&input).unwrap();
    update(&mut json);
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
}

#[tokio::test]
async fn test_inspect_runtime_cli() {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-cli-inspect-{}", uuid::Uuid::new_v4()));
    copy_minimal_workspace(&temp_dir);

    update_workspace_config(&temp_dir, |json| {
        json["defaults"]["profile"] = json!("mock-profile");
        json["defaults"]["provider"] = json!("mock-provider");
        json["defaults"]["model"] = json!("mock-model");
        json["defaults"]["max_turns"] = json!(12);
        json["profiles"] = json!({
            "mock-profile": {
                "provider": "mock-provider",
                "model": "mock-model"
            }
        });
    });

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        model: None,
        mode: None,
        max_turns: None,
        provider: None,
        profile: Some("mock-profile".to_string()),
        skills: Vec::new(),
        context_window_override: None,
    };

    let inspect = inspect_runtime(&overrides, None).await.unwrap();
    assert_eq!(inspect.provider_name, "mock-provider");
    assert_eq!(inspect.provider_model, "mock-model");
    assert_eq!(inspect.execution_mode, "Confirm");
    assert_eq!(inspect.max_turns, 12);
    assert!(!inspect.tools.is_empty());
    assert!(inspect
        .verifiers
        .contains(&"FileExistsVerifier".to_string()));
    assert!(inspect.hooks.contains(&"VerificationToolHook".to_string()));

    // Clean up
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn test_runtime_inspect_cli_subcommand() {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-cli-inspect-sub-{}", uuid::Uuid::new_v4()));
    copy_minimal_workspace(&temp_dir);

    update_workspace_config(&temp_dir, |json| {
        json["defaults"]["profile"] = json!("openai-profile");
        json["profiles"] = json!({
            "openai-profile": {
                "provider": "openai",
                "model": "gpt-4o"
            }
        });
    });

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .env("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir")
        .arg("--workspace")
        .arg(&temp_dir)
        .arg("--format")
        .arg("json")
        .arg("runtime")
        .arg("inspect")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "CLI runtime inspect command should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout_str = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout_str).unwrap();
    assert_eq!(json["kind"], "runtime.inspect");
    assert_eq!(json["data"]["inspect"]["provider_name"], "openai");
    assert_eq!(json["data"]["inspect"]["provider_model"], "gpt-4o");

    // Also assert that the trace sink kind matches (JsonlTraceSink) and hooks list contains EvaluatorHook
    let inspect = &json["data"]["inspect"];
    assert_eq!(inspect["trace_sink_kind"].as_str(), Some("JsonlTraceSink"));
    let hooks = inspect["hooks"]
        .as_array()
        .expect("hooks should be an array");
    let has_evaluator_hook = hooks.iter().any(|h| h.as_str() == Some("EvaluatorHook"));
    assert!(
        has_evaluator_hook,
        "inspect report should contain EvaluatorHook"
    );

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_build_cli_runtime_loads_configured_extension_instance() {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_dir = std::env::temp_dir().join(format!("gestalt-cli-trust-{}", uuid::Uuid::new_v4()));
    copy_minimal_workspace(&temp_dir);

    std::fs::create_dir_all(temp_dir.join(".gestalt/extensions/tools")).unwrap();
    std::fs::write(
        temp_dir.join(".gestalt/extensions/tools/gestalt.extension.toml"),
        r#"
manifest_version = 2

[package]
id = "com.example.tools"
name = "Example Tools"
version = "1.0.0"

[[components]]
id = "echo"
kind = "command-tool"
description = "Echo JSON"
input_schema = { type = "object" }
risk = "Low"
read_only = true
idempotent = true

[components.entrypoint]
command = "/bin/cat"
args = []
"#,
    )
    .unwrap();

    update_workspace_config(&temp_dir, |json| {
        json["defaults"]["profile"] = json!("mock-profile");
        json["profiles"] = json!({
            "mock-profile": {
                "provider": "mock-provider",
                "model": "mock-model"
            }
        });
        json["extensions"] = json!({
            "allow_untrusted": true,
            "instances": {
                "review-primary": {
                    "package": "com.example.tools",
                    "enabled": true,
                    "components": {
                        "echo": true
                    }
                }
            }
        });
    });

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        model: None,
        mode: None,
        max_turns: None,
        provider: None,
        profile: None,
        skills: Vec::new(),
        context_window_override: None,
    };
    let config = gestalt_app::config::load_effective_config(&overrides).unwrap();

    let runtime = gestalt_app::runtime_factory::build_cli_runtime(&config, None, None, None, None)
        .await
        .unwrap();

    let tool_name = "extension:com.example.tools@review-primary:echo";
    let registered = runtime.tools.get(tool_name).is_some();
    assert!(
        registered,
        "Configured extension instance should register a unique command tool"
    );

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}
