use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::control::{HostControl, RuntimeControl};
use gestalt_runtime::TrustedExtensionPin;
use gestalt_runtime::{
    activation::HostLaunchContext, AgentRuntimeBuilder, ReloadExtensionsRequest, RuntimeConfig,
    RuntimeHost,
};
use gestalt_runtime::{
    discovery::{DiscoverySource, ExtensionDiscovery},
    extension::{ExtensionManager, ResolvedExtensionPackage, RuntimeExtensionSnapshot},
};
use sha2::{Digest, Sha256};

struct EmptyToolCatalog;

impl ToolCatalog for EmptyToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn display_name(&self) -> &str {
        "Mock"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: false,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: true,
            supports_strict_schema: false,
        };
        &CAP
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

struct MockPolicyEngine;

#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

#[tokio::test]
async fn dry_run_reload_does_not_publish_generation() {
    let runtime = runtime();

    let report = runtime
        .reload_extensions(ReloadExtensionsRequest {
            dry_run: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(!report.published);
    assert_eq!(report.previous_generation.0, 0);
    assert_eq!(report.candidate_generation.0, 1);
    assert_eq!(runtime.current_generation().0, 0);
}

#[tokio::test]
async fn reload_publishes_next_generation_and_inspect_reports_it() {
    let runtime = runtime();

    let report = runtime
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();

    assert!(report.published);
    assert_eq!(runtime.current_generation(), report.candidate_generation);
    assert_eq!(
        runtime.inspect_runtime().await.runtime_generation,
        report.candidate_generation.0
    );
}

fn runtime() -> gestalt_runtime::AgentRuntime {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_host_owns_workspace_and_generation_lineage() {
    let builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            workspace_root: std::path::PathBuf::from("/test/host/workspace"),
            ..Default::default()
        });
    let host = Arc::new(
        gestalt_runtime::RuntimeHost::new(
            builder,
            Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
        )
        .unwrap(),
    );

    assert_eq!(
        host.workspace_root,
        std::path::PathBuf::from("/test/host/workspace")
    );
    assert_eq!(host.current_generation().0, 0);

    let session_id = host.spawn_session("test-session", None).await.unwrap();
    let registry = host.session_registry.lock().unwrap();
    let session_runtime = registry.get(&session_id).unwrap();
    assert_eq!(
        session_runtime.config.workspace_root,
        std::path::PathBuf::from("/test/host/workspace")
    );
    assert_eq!(session_runtime.current_generation().0, 0);
}

#[tokio::test]
async fn test_per_session_override_cannot_mutate_critical_inputs() {
    let builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            workspace_root: std::path::PathBuf::from("/test/host/workspace"),
            ..Default::default()
        });
    let host = Arc::new(
        gestalt_runtime::RuntimeHost::new(
            builder,
            Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
        )
        .unwrap(),
    );

    let mut config_override = RuntimeConfig::default();
    config_override.workspace_root = std::path::PathBuf::from("/malicious/override");

    let session_id = host
        .spawn_session("test-session-override", Some(config_override))
        .await
        .unwrap();
    let registry = host.session_registry.lock().unwrap();
    let session_runtime = registry.get(&session_id).unwrap();

    assert_eq!(
        session_runtime.config.workspace_root,
        std::path::PathBuf::from("/test/host/workspace")
    );
}

#[tokio::test]
async fn host_reload_advances_shared_generation_only_once_across_sessions() {
    let builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default());
    let host = Arc::new(
        gestalt_runtime::RuntimeHost::new(
            builder,
            Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
        )
        .unwrap(),
    );

    host.spawn_session("session-a", None).await.unwrap();
    host.spawn_session("session-b", None).await.unwrap();

    let report = host
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();

    assert_eq!(report.previous_generation.0, 0);
    assert_eq!(report.candidate_generation.0, 1);
    assert_eq!(host.current_generation().0, 1);

    let sessions = host.session_registry.lock().unwrap();
    for runtime in sessions.values() {
        assert_eq!(runtime.current_generation().0, 1);
    }
}

#[tokio::test]
async fn runtime_host_initialization_activates_configured_packages() {
    let temp = TempTree::new("gestalt-runtime-host-init");
    let workspace_root = temp.path().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();

    let package = lifecycle_package(
        &workspace_root.join("host-init-package"),
        "host-init-package",
        "Host Init Package",
        "1.0.0",
    );

    let builder = test_builder(workspace_root.clone())
        .config(RuntimeConfig {
            workspace_root: workspace_root.clone(),
            trusted_extension_pins: vec![TrustedExtensionPin::new(
                package.descriptor.id.clone(),
                package.manifest_hash.clone(),
            )],
            ..Default::default()
        })
        .extension_package(package);

    let host = RuntimeHost::new(
        builder,
        Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
    )
    .unwrap();

    assert_eq!(host.current_generation().0, 1);
    assert_eq!(host.extension_manager.process_instances().len(), 1);
    assert_eq!(
        host.extension_manager
            .active_snapshot()
            .resolved_packages
            .len(),
        1
    );
    assert_eq!(
        host.extension_manager.active_snapshot().resolved_packages[0]
            .descriptor
            .id,
        "host-init-package"
    );
    assert_eq!(
        host.extension_manager
            .active_snapshot()
            .lifecycle_clients
            .len(),
        1
    );
}

#[tokio::test]
async fn runtime_host_initialization_returns_error_when_activation_fails() {
    let temp = TempTree::new("gestalt-runtime-host-init-fails");
    let workspace_root = temp.path().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();

    let mut package = lifecycle_package(
        &workspace_root.join("host-init-fail-package"),
        "host-init-fail-package",
        "Host Init Fail Package",
        "1.0.0",
    );
    package.components[0].entrypoint.command = "definitely-missing-binary".to_string();

    let builder = test_builder(workspace_root.clone())
        .config(RuntimeConfig {
            workspace_root: workspace_root.clone(),
            trusted_extension_pins: vec![TrustedExtensionPin::new(
                package.descriptor.id.clone(),
                package.manifest_hash.clone(),
            )],
            ..Default::default()
        })
        .extension_package(package);

    let err = RuntimeHost::new(
        builder,
        Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
    )
    .err()
    .expect("host construction should fail");

    assert!(err.to_string().contains("Spawn failed"));
}

#[tokio::test]
async fn reload_rediscovers_added_removed_and_changed_packages() {
    let temp = TempTree::new("gestalt-runtime-reload");
    let workspace_root = temp.path().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();

    let alpha_hash = write_lifecycle_manifest(
        &workspace_root.join(".gestalt/extensions/alpha"),
        "alpha",
        "Alpha Extension",
        "1.0.0",
    );
    let beta_hash = lifecycle_manifest_hash(
        &workspace_root.join(".gestalt/extensions/beta"),
        "beta",
        "Beta Extension",
        "2.0.0",
    );

    let builder = test_builder(workspace_root.clone()).config(RuntimeConfig {
        workspace_root: workspace_root.clone(),
        trusted_extension_pins: vec![
            TrustedExtensionPin::new("alpha", Some(alpha_hash)),
            TrustedExtensionPin::new("beta", Some(beta_hash)),
        ],
        ..Default::default()
    });
    let host = Arc::new(runtime_host_with_discovery(builder, workspace_root.clone()));

    let first = host
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();
    assert!(first.published);
    assert_eq!(
        host.extension_manager
            .active_snapshot()
            .resolved_packages
            .iter()
            .map(|pkg| pkg.descriptor.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha"]
    );

    write_lifecycle_manifest(
        &workspace_root.join(".gestalt/extensions/beta"),
        "beta",
        "Beta Extension",
        "2.0.0",
    );
    fs::remove_dir_all(workspace_root.join(".gestalt/extensions/alpha")).unwrap();

    let second = host
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();
    assert!(second.published);
    let active = host.extension_manager.active_snapshot();
    assert_eq!(
        active
            .resolved_packages
            .iter()
            .map(|pkg| pkg.descriptor.id.as_str())
            .collect::<Vec<_>>(),
        ["beta"]
    );
    assert_eq!(active.resolved_packages[0].descriptor.version, "2.0.0");
    assert_ne!(first.candidate_fingerprint, second.candidate_fingerprint);
}

#[tokio::test]
async fn reload_reports_optional_activation_diagnostics() {
    let temp = TempTree::new("gestalt-runtime-reload-optional");
    let workspace_root = temp.path().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();

    let package = lifecycle_package_with_options(
        &workspace_root.join(".gestalt/extensions/optional"),
        "optional-ext",
        "Optional Extension",
        "1.0.0",
        true,
        "definitely-missing-binary",
    );

    let builder = test_builder(workspace_root.clone()).config(RuntimeConfig {
        workspace_root: workspace_root.clone(),
        trusted_extension_pins: vec![TrustedExtensionPin::new(
            package.descriptor.id.clone(),
            package.manifest_hash.clone(),
        )],
        ..Default::default()
    });
    let host = Arc::new(runtime_host_with_discovery(builder, workspace_root.clone()));

    let report = host
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();

    assert!(!report.diagnostics.is_empty());
    assert!(report
        .validation_errors
        .iter()
        .any(|message| { message.contains("Optional lifecycle component failed to launch") }));
}

fn test_builder(workspace_root: PathBuf) -> AgentRuntimeBuilder {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            workspace_root,
            trusted_extension_pins: Vec::new(),
            ..Default::default()
        })
}

fn runtime_host_with_discovery(
    builder: AgentRuntimeBuilder,
    workspace_root: PathBuf,
) -> RuntimeHost {
    let config = builder.config.clone();
    let event_bus = builder.event_bus.clone();
    let approval_broker = Arc::new(gestalt_runtime::activation::HostApprovalBroker::new());
    let extension_source: Arc<dyn gestalt_runtime::activation::ExtensionSource> =
        Arc::new(DiscoverySource::new(
            ExtensionDiscovery::new(workspace_root.clone(), None),
            Vec::new(),
        ));
    let registry_snapshot = builder.registry.snapshot();
    let extension_snapshot = RuntimeExtensionSnapshot::from_registry_snapshot(
        gestalt_runtime::extension::RuntimeGeneration(0),
        registry_snapshot,
        builder.tools.clone().unwrap(),
        #[cfg(feature = "mcp")]
        Arc::new(gestalt_runtime::mcp::McpRegistry::new(
            workspace_root.clone(),
            std::collections::HashMap::new(),
        )),
    );
    let extension_manager = Arc::new(ExtensionManager::new(
        Arc::new(extension_snapshot),
        event_bus.clone(),
        Arc::new(gestalt_runtime::extension::LocalProcessLauncher),
        HostLaunchContext::from_runtime_config(&config, event_bus.clone()),
    ));

    RuntimeHost {
        workspace_root,
        config,
        extension_manager,
        session_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        event_bus,
        artifact_store: Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
        approval_broker,
        extension_source,
        builder,
    }
}

fn write_lifecycle_manifest(
    dir: &Path,
    package_id: &str,
    package_name: &str,
    version: &str,
) -> String {
    let script_path = dir.join("lifecycle.py");
    let command = script_path.display().to_string();
    write_lifecycle_manifest_with_options(dir, package_id, package_name, version, false, &command)
}

fn write_lifecycle_manifest_with_options(
    dir: &Path,
    package_id: &str,
    package_name: &str,
    version: &str,
    optional: bool,
    command: &str,
) -> String {
    fs::create_dir_all(dir).unwrap();
    let script_path = dir.join("lifecycle.py");
    fs::write(&script_path, lifecycle_script(package_id)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
    }

    let manifest = format!(
        r#"
manifest_version = 2

[package]
id = "{package_id}"
name = "{package_name}"
version = "{version}"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"
optional = {optional}

[components.entrypoint]
command = "{command}"
args = []
"#,
        optional = optional,
        command = command,
    );
    fs::write(dir.join("gestalt.extension.toml"), &manifest).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(manifest.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn lifecycle_manifest_hash(
    dir: &Path,
    package_id: &str,
    package_name: &str,
    version: &str,
) -> String {
    let script_path = dir.join("lifecycle.py");
    let manifest = format!(
        r#"
manifest_version = 2

[package]
id = "{package_id}"
name = "{package_name}"
version = "{version}"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "{}"
args = []
"#,
        script_path.display()
    );
    let mut hasher = Sha256::new();
    hasher.update(manifest.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn lifecycle_package(
    dir: &Path,
    package_id: &str,
    package_name: &str,
    version: &str,
) -> ResolvedExtensionPackage {
    let manifest_hash = write_lifecycle_manifest(dir, package_id, package_name, version);
    let content = fs::read_to_string(dir.join("gestalt.extension.toml")).unwrap();
    let manifest = gestalt_runtime::extension::ExtensionManifestV2::parse(&content).unwrap();
    let mut package = ResolvedExtensionPackage::from_v2_manifest(manifest, package_id).unwrap();
    package.manifest_hash = Some(manifest_hash);
    package
}

fn lifecycle_package_with_options(
    dir: &Path,
    package_id: &str,
    package_name: &str,
    version: &str,
    optional: bool,
    command: &str,
) -> ResolvedExtensionPackage {
    let manifest_hash = write_lifecycle_manifest_with_options(
        dir,
        package_id,
        package_name,
        version,
        optional,
        command,
    );
    let content = fs::read_to_string(dir.join("gestalt.extension.toml")).unwrap();
    let manifest = gestalt_runtime::extension::ExtensionManifestV2::parse(&content).unwrap();
    let mut package = ResolvedExtensionPackage::from_v2_manifest(manifest, package_id).unwrap();
    package.manifest_hash = Some(manifest_hash);
    package
}

fn lifecycle_script(package_id: &str) -> String {
    let component_id = format!("component:{package_id}:{package_id}:lifecycle");
    format!(
        r#"#!/usr/bin/env python3
import json
import sys

component_id = "{component_id}"
while True:
    line = sys.stdin.readline()
    if not line:
        break
    req = json.loads(line)
    method = req.get("method")
    req_id = req.get("id")
    if method == "initialize":
        result = {{"negotiated_version": "2.0", "supports_cancellation": True}}
    elif method == "capabilities/describe":
        result = [{{"component_id": component_id, "capability": "context_provider", "priority": 0, "timeout_ms": 250, "failure_mode": "fail_open", "data_scope": "current_turn"}}]
    elif method == "shutdown":
        result = {{}}
    else:
        result = {{}}
    sys.stdout.write(json.dumps({{"jsonrpc": "2.0", "result": result, "id": req_id}}) + "\n")
    sys.stdout.flush()
"#
    )
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new(prefix: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
