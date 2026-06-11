use gestalt_runtime::{
    check_network_permission, check_path_permission, check_shell_permission, Capabilities,
    Entrypoint, ExtensionManifest, Permissions, RuntimeEventBus,
};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

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

fn temp_workspace() -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "gestalt-runtime-permissions-{}-{}",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
fn test_permissions_reject_nonexistent_traversal_outside_workspace() {
    let mut manifest = dummy_manifest();
    manifest.permissions.allow_workspace_write = true;
    let event_bus = RuntimeEventBus::new();
    let workspace = temp_workspace();
    let outside = workspace
        .parent()
        .expect("temp workspace should have parent")
        .join("outside-target");

    let res = check_path_permission(
        &manifest,
        &workspace,
        Path::new("../outside-target/new-file.txt"),
        true,
        &event_bus,
    );
    assert!(
        res.is_err(),
        "expected traversal outside workspace to be denied"
    );
    assert!(
        !outside.join("new-file.txt").exists(),
        "permission check must not treat traversal path as workspace-local"
    );

    let _ = std::fs::remove_dir_all(&workspace);
}

#[cfg(unix)]
#[test]
fn test_permissions_reject_symlink_escape_for_nonexistent_target() {
    let mut manifest = dummy_manifest();
    manifest.permissions.allow_workspace_write = true;
    let event_bus = RuntimeEventBus::new();
    let workspace = temp_workspace();
    let outside = workspace
        .parent()
        .expect("temp workspace should have parent")
        .join(format!(
            "outside-target-{}",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
    std::fs::create_dir_all(&outside).unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, workspace.join("linked-outside")).unwrap();

    let res = check_path_permission(
        &manifest,
        &workspace,
        Path::new("linked-outside/../escaped.txt"),
        true,
        &event_bus,
    );
    assert!(res.is_err(), "expected symlink escape to be denied");

    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&outside);
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
