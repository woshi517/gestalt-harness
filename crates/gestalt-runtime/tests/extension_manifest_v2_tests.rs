#![allow(deprecated)]

use gestalt_runtime::{
    extension::{
        compute_complete_fingerprint, ComponentKind, ExtensionManifestV2,
        ExtensionPackageDescriptor, ResolvedExtensionPackage,
    },
    ExtensionDiscovery, ExtensionManifest,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const V2_MANIFEST: &str = r#"
manifest_version = 2

[package]
id = "com.example.review"
name = "Review"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"
optional = false

[components.entrypoint]
command = "python"
args = ["-m", "review.lifecycle"]

[[components]]
id = "client-metadata"
kind = "client-product"
optional = true
descriptor = "client/contributions.json"

[[components]]
id = "mcp"
kind = "mcp-server"
optional = false

[components.entrypoint]
command = "node"
args = ["server.js"]
"#;

#[test]
fn manifest_v2_parses_multiple_component_kinds() {
    let manifest = ExtensionManifestV2::parse(V2_MANIFEST).unwrap();

    manifest.validate().unwrap();
    assert_eq!(manifest.package.id, "com.example.review");
    assert_eq!(manifest.package.version, "1.0.0");
    assert_eq!(manifest.components.len(), 3);
    assert_eq!(manifest.components[0].kind, ComponentKind::GestaltLifecycle);
    assert_eq!(manifest.components[1].kind, ComponentKind::ClientProduct);
    assert!(manifest.components[1].optional);
    assert_eq!(
        manifest.components[1].descriptor.as_deref(),
        Some("client/contributions.json")
    );
    assert_eq!(manifest.components[2].kind, ComponentKind::McpServer);
}

#[test]
fn manifest_v2_rejects_duplicate_component_ids() {
    let manifest = ExtensionManifestV2::parse(
        r#"
manifest_version = 2

[package]
id = "com.example.dup"
name = "Duplicate"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "worker"
kind = "gestalt-lifecycle"

[[components]]
id = "worker"
kind = "command-tool"
"#,
    )
    .unwrap();

    let err = manifest.validate().unwrap_err();
    assert!(err.contains("Duplicate component id 'worker'"));
}

#[test]
fn current_v1_manifest_normalizes_to_required_legacy_component() {
    let manifest = ExtensionManifest::parse(
        r#"
id = "legacy-ext"
name = "Legacy Extension"
version = "1.0.0"
manifest_version = "1"
protocol_version = "1.0"
runtime = "stdio"

[entrypoint]
command = "python"
args = ["-m", "legacy"]

[capabilities]
tools = true
hooks = true
context = true

[[tools]]
name = "legacy_tool"
description = "legacy tool"
input_schema = { type = "object" }

[[hooks]]
name = "legacy_hook"
lifecycle_point = "before_tool_policy"

[[context_injectors]]
name = "legacy_context"
stability = "turn_dynamic"
"#,
    )
    .unwrap();

    let package = ResolvedExtensionPackage::from_v1_manifest(manifest).unwrap();

    assert_eq!(package.descriptor.id, "legacy-ext");
    assert_eq!(package.components.len(), 1);
    let component = &package.components[0];
    assert_eq!(component.kind, ComponentKind::LegacyProcess);
    assert_eq!(component.id.component_id, "legacy");
    assert!(!component.optional);
    assert_eq!(component.tools.len(), 1);
    assert_eq!(component.hooks.len(), 1);
    assert_eq!(component.context_injectors.len(), 1);
    assert_eq!(component.entrypoint.command, "python");
}

#[test]
fn package_and_component_canonical_ids_include_instance_scope() {
    let manifest = ExtensionManifestV2::parse(V2_MANIFEST).unwrap();
    let package = ResolvedExtensionPackage::from_v2_manifest(manifest, "review-primary").unwrap();

    assert_eq!(
        package.descriptor.canonical_id(),
        "package:com.example.review"
    );
    assert_eq!(
        package.components[0].id.canonical_id(),
        "component:com.example.review:review-primary:lifecycle"
    );
}

#[test]
fn reserved_package_namespaces_remain_rejected() {
    let descriptor = ExtensionPackageDescriptor {
        id: "gestalt.internal".to_string(),
        name: "Reserved".to_string(),
        version: "1.0.0".to_string(),
    };

    let err = descriptor.validate().unwrap_err();
    assert!(err.contains("reserved namespace"));
}

#[test]
fn discovery_exposes_package_inventory_for_v1_and_v2_manifests() {
    let root = TempTree::new("gestalt-runtime-package-discovery");
    let workspace = root.path().join("workspace");
    let global = root.path().join("global");
    write_v1_extension(
        &workspace.join(".gestalt/extensions/01-v1"),
        "legacy-package",
        "Legacy Package",
    );
    write_v2_extension(
        &global.join("extensions/01-v2"),
        "com.example.v2",
        "V2 Package",
    );

    let discovery = ExtensionDiscovery::new(workspace, Some(global));
    let packages = discovery.discover_packages(&[]).unwrap();
    let ids = packages
        .iter()
        .map(|package| package.package.descriptor.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["legacy-package", "com.example.v2"]);
    assert_eq!(
        packages[0].package.components[0].kind,
        ComponentKind::LegacyProcess
    );
    assert_eq!(
        packages[1].package.components[0].kind,
        ComponentKind::GestaltLifecycle
    );
    assert!(packages.iter().all(|package| package.enabled));
}

#[test]
fn package_fingerprint_changes_for_non_file_entrypoint_arguments() {
    let left = legacy_package("python", &["-m", "review.lifecycle"], false);
    let right = legacy_package("python", &["-m", "review.other"], false);

    let left_fp = compute_complete_fingerprint("registry-a", &[left]);
    let right_fp = compute_complete_fingerprint("registry-a", &[right]);

    assert_ne!(left_fp, right_fp);
}

#[test]
fn package_fingerprint_changes_with_cancellation_declaration() {
    let left = legacy_package("python", &["-m", "review.lifecycle"], false);
    let right = legacy_package("python", &["-m", "review.lifecycle"], true);

    let left_fp = compute_complete_fingerprint("registry-a", &[left]);
    let right_fp = compute_complete_fingerprint("registry-a", &[right]);

    assert_ne!(left_fp, right_fp);
}

fn write_v1_extension(dir: &Path, id: &str, name: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("gestalt.extension.toml"),
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
args = ["-m", "legacy"]

[capabilities]
"#
        ),
    )
    .unwrap();
}

fn write_v2_extension(dir: &Path, id: &str, name: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("gestalt.extension.toml"),
        format!(
            r#"
manifest_version = 2

[package]
id = "{id}"
name = "{name}"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "lifecycle"]
"#
        ),
    )
    .unwrap();
}

fn legacy_package(
    command: &str,
    args: &[&str],
    supports_cancellation: bool,
) -> ResolvedExtensionPackage {
    let args_toml = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest = ExtensionManifest::parse(&format!(
        r#"
id = "legacy-package"
name = "Legacy Package"
version = "1.0.0"
manifest_version = "1"
protocol_version = "1.0"
runtime = "stdio"

[entrypoint]
command = "{command}"
args = [{args_toml}]

[capabilities]
tools = true
hooks = true
context = true
supports_cancellation = {supports_cancellation}

[[tools]]
name = "search"
description = "Search"
input_schema = {{ type = "object" }}
"#
    ))
    .unwrap();

    ResolvedExtensionPackage::from_v1_manifest(manifest).unwrap()
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
