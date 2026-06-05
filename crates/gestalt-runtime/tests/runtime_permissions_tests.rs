use gestalt_runtime::{
    check_network_permission, check_path_permission, check_shell_permission, Capabilities,
    Entrypoint, ExtensionManifest, Permissions, RuntimeEventBus,
};
use std::path::Path;

fn dummy_manifest() -> ExtensionManifest {
    ExtensionManifest {
        id: "test-ext".to_string(),
        name: "Test Extension".to_string(),
        version: "0.1.0".to_string(),
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "echo".to_string(),
            args: vec![],
        },
        capabilities: Capabilities {
            tools: true,
            hooks: false,
            context: false,
        },
        permissions: Permissions {
            allow_network: vec!["github.com".to_string()],
            allow_workspace_read: true,
            allow_workspace_write: false,
            allow_shell: false,
            allow_all_paths: false,
            allowed_paths: vec!["/tmp/test-allowed-path".to_string()],
        },
        tools: vec![],
        hooks: vec![],
        context_injectors: vec![],
    }
}

#[test]
fn test_permissions_paths() {
    let manifest = dummy_manifest();
    let event_bus = RuntimeEventBus::new();
    let workspace = Path::new("/workspace");

    // Path inside workspace read
    let res = check_path_permission(
        &manifest,
        workspace,
        Path::new("/workspace/src/lib.rs"),
        false,
        &event_bus,
    );
    assert!(res.is_ok());

    // Path inside workspace write (denied)
    let res = check_path_permission(
        &manifest,
        workspace,
        Path::new("/workspace/src/lib.rs"),
        true,
        &event_bus,
    );
    assert!(res.is_err());

    // Allowed path outside workspace
    let res = check_path_permission(
        &manifest,
        workspace,
        Path::new("/tmp/test-allowed-path/file.txt"),
        false,
        &event_bus,
    );
    assert!(res.is_ok());

    // Denied path outside workspace
    let res = check_path_permission(
        &manifest,
        workspace,
        Path::new("/etc/passwd"),
        false,
        &event_bus,
    );
    assert!(res.is_err());
}

#[test]
fn test_permissions_network() {
    let manifest = dummy_manifest();
    let event_bus = RuntimeEventBus::new();

    // Allowed host
    let res = check_network_permission(&manifest, "github.com", &event_bus);
    assert!(res.is_ok());

    // Denied host
    let res = check_network_permission(&manifest, "google.com", &event_bus);
    assert!(res.is_err());
}

#[test]
fn test_permissions_shell() {
    let mut manifest = dummy_manifest();
    let event_bus = RuntimeEventBus::new();

    // Denied shell
    let res = check_shell_permission(&manifest, &event_bus);
    assert!(res.is_err());

    // Allowed shell
    manifest.permissions.allow_shell = true;
    let res = check_shell_permission(&manifest, &event_bus);
    assert!(res.is_ok());
}
