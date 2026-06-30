use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gestalt_runtime::{
    extension::{
        compute_complete_fingerprint, ComponentKind, ExtensionManifestV2,
        ExtensionPackageDescriptor, ResolvedExtensionPackage,
    },
    ExtensionDiscovery, ExtensionTrust,
};

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
fn discovery_discovers_v2_packages_in_deterministic_order() {
    let root = TempTree::new("gestalt-runtime-package-discovery");
    let workspace = root.path().join("workspace");
    let global = root.path().join("global");

    write_v2_extension(
        &workspace.join(".gestalt/extensions/02-project-b"),
        "project-b",
        "Project B",
    );
    write_v2_extension(
        &workspace.join(".gestalt/extensions/01-project-a"),
        "project-a",
        "Project A",
    );
    write_v2_extension(
        &global.join("extensions/01-global-a"),
        "global-a",
        "Global A",
    );
    write_v2_extension(
        &global.join("extensions/02-global-duplicate-project"),
        "project-a",
        "Duplicate Project A",
    );
    write_v2_extension(
        &root.path().join("explicit-dir"),
        "explicit-dir",
        "Explicit Dir",
    );
    let explicit_file = root.path().join("explicit-file.toml");
    write_manifest_file(
        &explicit_file,
        v2_manifest("explicit-file", "Explicit File", "python", &["-m", "example"]),
    );

    let discovery = ExtensionDiscovery::new(workspace, Some(global));
    let discovered = discovery
        .discover_packages(&[root.path().join("explicit-dir"), explicit_file.clone()])
        .unwrap();
    let ids = discovered
        .iter()
        .map(|ext| ext.package.descriptor.id.as_str())
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
fn discovery_rejects_v1_explicit_manifests() {
    let root = TempTree::new("gestalt-runtime-package-discovery-v1");
    let explicit = root.path().join("legacy.toml");
    write_manifest_file(
        &explicit,
        r#"
id = "legacy-package"
name = "Legacy Package"
version = "1.0.0"
manifest_version = "1"
protocol_version = "1.0"
runtime = "stdio"

[entrypoint]
command = "python"
args = ["-m", "legacy"]

[capabilities]
"#
        .to_string(),
    );

    let discovery = ExtensionDiscovery::new(root.path().join("workspace"), None);
    let err = discovery.discover_packages(&[explicit]).unwrap_err();
    assert!(err.to_string().contains("manifest_version"));
}

#[test]
fn package_fingerprint_changes_for_entrypoint_args() {
    let left = v2_package("python", &["-m", "review.lifecycle"], "primary");
    let right = v2_package("python", &["-m", "review.other"], "primary");

    let left_fp = compute_complete_fingerprint("registry-a", &[left]);
    let right_fp = compute_complete_fingerprint("registry-a", &[right]);

    assert_ne!(left_fp, right_fp);
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

fn v2_package(command: &str, args: &[&str], instance_id: &str) -> ResolvedExtensionPackage {
    let manifest = ExtensionManifestV2::parse(&v2_manifest(
        "legacy-package",
        "Legacy Package",
        command,
        args,
    ))
    .unwrap();
    ResolvedExtensionPackage::from_v2_manifest(manifest, instance_id).unwrap()
}

fn v2_manifest(id: &str, name: &str, command: &str, args: &[&str]) -> String {
    let args_toml = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(", ");
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
command = "{command}"
args = [{args_toml}]
"#
    )
}

fn write_v2_extension(dir: &Path, id: &str, name: &str) {
    fs::create_dir_all(dir).unwrap();
    write_manifest_file(&dir.join("gestalt.extension.toml"), v2_manifest(id, name, "python", &["-m", "example"]));
}

fn write_manifest_file(path: &Path, content: String) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
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
