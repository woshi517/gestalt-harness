use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gestalt_runtime::{
    extension::{ExtensionManifestV2, ResolvedExtensionPackage},
    ExtensionDiscovery, ExtensionManifest, ExtensionTrust,
};

const FULL_V1_MANIFEST: &str = r#"
id = "baseline-ext"
name = "Baseline Extension"
version = "1.2.3"
manifest_version = "1"
protocol_version = "1.0"
runtime = "stdio"

[entrypoint]
command = "python"
args = ["-m", "baseline_ext"]

[capabilities]
tools = true
hooks = true
context = true
supports_cancellation = true

[permissions]
allow_network = ["api.example.com"]
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = ["/tmp/baseline"]

[[tools]]
name = "baseline_tool"
description = "A baseline tool"
input_schema = { type = "object", properties = { query = { type = "string" } } }
risk = "medium"
read_only = true
idempotent = true

[[hooks]]
name = "baseline_policy"
lifecycle_point = "before_tool_policy"
failure_mode = "closed"
timeout_ms = 1500

[[hooks]]
name = "baseline_result"
lifecycle_point = "after_tool_result"
failure_mode = "open"

[[context_injectors]]
name = "baseline_context"
stability = "turn_dynamic"
"#;

#[test]
fn v1_manifest_preserves_runtime_process_declarations() {
    let manifest = ExtensionManifest::parse(FULL_V1_MANIFEST).unwrap();

    manifest.validate(true).unwrap();
    assert_eq!(manifest.id, "baseline-ext");
    assert_eq!(manifest.entrypoint.command, "python");
    assert_eq!(manifest.entrypoint.args, ["-m", "baseline_ext"]);
    assert!(manifest.capabilities.tools);
    assert!(manifest.capabilities.hooks);
    assert!(manifest.capabilities.context);
    assert!(manifest.capabilities.supports_cancellation);
    assert_eq!(manifest.permissions.allow_network, ["api.example.com"]);
    assert!(manifest.permissions.allow_workspace_read);
    assert!(!manifest.permissions.allow_workspace_write);
    assert_eq!(manifest.permissions.allowed_paths, ["/tmp/baseline"]);
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(manifest.tools[0].name, "baseline_tool");
    assert_eq!(manifest.tools[0].risk.as_deref(), Some("medium"));
    assert_eq!(manifest.tools[0].read_only, Some(true));
    assert_eq!(manifest.tools[0].idempotent, Some(true));
    assert_eq!(manifest.hooks.len(), 2);
    assert_eq!(manifest.hooks[0].lifecycle_point, "before_tool_policy");
    assert_eq!(manifest.hooks[0].failure_mode.as_deref(), Some("closed"));
    assert_eq!(manifest.hooks[0].timeout_ms, Some(1500));
    assert_eq!(manifest.context_injectors.len(), 1);
    assert_eq!(manifest.context_injectors[0].name, "baseline_context");
}

#[test]
fn discovery_uses_explicit_project_global_precedence_with_deterministic_ordering() {
    let root = TempTree::new("gestalt-runtime-discovery-baseline");
    let workspace = root.path().join("workspace");
    let global = root.path().join("global");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&global).unwrap();

    write_extension(
        &workspace.join(".gestalt/extensions/02-project-b"),
        "project-b",
        "Project B",
    );
    write_extension(
        &workspace.join(".gestalt/extensions/01-project-a"),
        "project-a",
        "Project A",
    );
    write_extension(
        &global.join("extensions/01-global-a"),
        "global-a",
        "Global A",
    );
    write_extension(
        &global.join("extensions/02-global-duplicate-project"),
        "project-a",
        "Duplicate Project A",
    );
    write_extension(
        &root.path().join("explicit-dir"),
        "explicit-dir",
        "Explicit Dir",
    );
    let explicit_file = root.path().join("explicit-file.toml");
    write_manifest_file(&explicit_file, "explicit-file", "Explicit File");

    let discovery = ExtensionDiscovery::new(workspace, Some(global));
    let discovered = discovery
        .discover_all(&[root.path().join("explicit-dir"), explicit_file.clone()])
        .unwrap();
    let ids = discovered
        .iter()
        .map(|ext| ext.manifest.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        [
            "explicit-dir",
            "explicit-file",
            "project-a",
            "project-b",
            "global-a"
        ]
    );
    assert!(discovered.iter().all(|ext| ext.enabled));
    assert_eq!(discovered[1].manifest_path, explicit_file);
}

#[test]
fn discovery_records_stable_content_hashes_for_trust_baselines() {
    let root = TempTree::new("gestalt-runtime-hash-baseline");
    let manifest_path = root.path().join("explicit.toml");
    write_manifest_file(&manifest_path, "hash-ext", "Hash Extension");

    let discovery = ExtensionDiscovery::new(root.path().join("workspace"), None);
    let first = discovery
        .discover_all(std::slice::from_ref(&manifest_path))
        .unwrap();
    let second = discovery.discover_all(&[manifest_path]).unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(first[0].manifest_hash, second[0].manifest_hash);
    assert_eq!(first[0].manifest_hash.len(), 64);
    assert!(first[0]
        .manifest_hash
        .chars()
        .all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn allow_untrusted_package_remains_untrusted() {
    let manifest = ExtensionManifestV2::parse(
        r#"
manifest_version = 2

[package]
id = "trust-regression"
name = "Trust Regression"
version = "1.0.0"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python3"
args = ["-c", "print('noop')"]
"#,
    )
    .unwrap();

    let mut package = ResolvedExtensionPackage::from_v2_manifest(manifest, "instance-a").unwrap();
    package.manifest_hash =
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());

    let component = package.to_runtime_component("lifecycle").unwrap();

    assert_eq!(package.trust, ExtensionTrust::Untrusted);
    assert_eq!(component.trust, ExtensionTrust::Untrusted);
}

fn write_extension(dir: &Path, id: &str, name: &str) {
    fs::create_dir_all(dir).unwrap();
    write_manifest_file(&dir.join("gestalt.extension.toml"), id, name);
}

fn write_manifest_file(path: &Path, id: &str, name: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, minimal_manifest(id, name)).unwrap();
}

fn minimal_manifest(id: &str, name: &str) -> String {
    format!(
        r#"
id = "{id}"
name = "{name}"
version = "1.0.0"
manifest_version = "1"
protocol_version = "1.0"
runtime = "stdio"

[entrypoint]
command = "python"
args = ["-m", "example"]

[capabilities]
"#
    )
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new(prefix: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
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
