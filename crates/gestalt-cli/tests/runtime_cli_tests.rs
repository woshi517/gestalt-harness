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
    let src = std::path::Path::new("tests/fixtures/workspaces/minimal");
    if src.exists() {
        copy_dir_recursive(src, dest);
    } else {
        // Fallback for different working directories in tests
        let alt_src = std::path::Path::new("../../tests/fixtures/workspaces/minimal");
        if alt_src.exists() {
            copy_dir_recursive(alt_src, dest);
        }
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
            std::fs::copy(&path, dest.join(name)).unwrap();
        }
    }
}

#[tokio::test]
async fn test_inspect_runtime_cli() {
    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-cli-inspect-{}", uuid::Uuid::new_v4()));
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
    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-cli-inspect-sub-{}", uuid::Uuid::new_v4()));
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
async fn test_build_cli_runtime_trust_gating() {
    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-cli-trust-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(temp_dir.join(".gestalt/extensions/local-ext")).unwrap();

    // Write a dummy extension manifest in the local workspace directory
    let local_ext_manifest = r#"
id = "local-ext"
name = "Local Mock Extension"
version = "0.1.0"
runtime = "stdio"

[entrypoint]
command = "non_existent_command_12345"

[capabilities]
tools = false
hooks = false
context = false

[permissions]
allow_network = []
allow_workspace_read = false
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = []
"#;
    std::fs::write(
        temp_dir.join(".gestalt/extensions/local-ext/gestalt.extension.toml"),
        local_ext_manifest,
    )
    .unwrap();

    // 1. First scenario: Untrusted by default.
    let config_toml_untrusted = r#"
[defaults]
provider = "mock-provider"
model = "mock-model"
mode = "confirm"
"#;
    std::fs::write(temp_dir.join(".gestalt/config.toml"), config_toml_untrusted).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_dir.clone()),
        model: None,
        mode: None,
        max_turns: None,
        provider: None,
        profile: None,
    };
    let config = gestalt_cli::config::load_effective_config(&overrides).unwrap();

    let runtime = gestalt_cli::runtime::build_cli_runtime(
        &config,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let events = runtime.event_bus.history();
    let untrusted_rejected = events.iter().any(|e| match e {
        gestalt_runtime::RuntimeEvent::ExtensionRejected { extension_id, reason } => {
            extension_id == "local-ext" && reason.contains("Untrusted project extension ignored")
        }
        _ => false,
    });
    assert!(untrusted_rejected, "Local extension should be rejected as untrusted by default. Events: {:?}", events);

    // 2. Second scenario: Untrusted but allow_untrusted = true in config.
    let config_toml_allow_untrusted = r#"
[defaults]
provider = "mock-provider"
model = "mock-model"
mode = "confirm"

[extensions]
allow_untrusted = true
"#;
    std::fs::write(temp_dir.join(".gestalt/config.toml"), config_toml_allow_untrusted).unwrap();
    let config = gestalt_cli::config::load_effective_config(&overrides).unwrap();

    let runtime = gestalt_cli::runtime::build_cli_runtime(
        &config,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let events = runtime.event_bus.history();
    let allowed_untrusted = events.iter().any(|e| match e {
        gestalt_runtime::RuntimeEvent::ExtensionRejected { extension_id, reason } => {
            extension_id == "local-ext" && reason.contains("Startup failure")
        }
        _ => false,
    });
    assert!(allowed_untrusted, "Local extension should bypass trust gate with allow_untrusted=true. Events: {:?}", events);

    // 3. Third scenario: Explicitly trusted via extensions.trusted list.
    let config_toml_trusted = r#"
[defaults]
provider = "mock-provider"
model = "mock-model"
mode = "confirm"

[extensions]
trusted = ["local-ext"]
"#;
    std::fs::write(temp_dir.join(".gestalt/config.toml"), config_toml_trusted).unwrap();
    let config = gestalt_cli::config::load_effective_config(&overrides).unwrap();

    let runtime = gestalt_cli::runtime::build_cli_runtime(
        &config,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let events = runtime.event_bus.history();
    let trusted_allowed = events.iter().any(|e| match e {
        gestalt_runtime::RuntimeEvent::ExtensionRejected { extension_id, reason } => {
            extension_id == "local-ext" && reason.contains("Startup failure")
        }
        _ => false,
    });
    assert!(trusted_allowed, "Local extension should bypass trust gate when in trusted list. Events: {:?}", events);

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}
