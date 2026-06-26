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
        manifest_version: None,
        protocol_version: None,
        runtime: "stdio".to_string(),
        entrypoint: Entrypoint {
            command: "echo".to_string(),
            args: vec![],
        },
        capabilities: Capabilities {
            tools: true,
            hooks: false,
            context: false,
            ..Default::default()
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

#[test]
fn test_permissions_effective_shell() {
    let mut manifest = dummy_manifest().permissions;
    let mut grant = gestalt_runtime::extension::ExtensionGrantConfig::default();
    let event_bus = RuntimeEventBus::new();

    // 1. Both allow
    manifest.allow_shell = true;
    grant.shell = true;
    let res = gestalt_runtime::check_shell_permission_effective(&manifest, Some(&grant), &event_bus, "test");
    assert!(res.is_ok());

    // 2. Only manifest allows
    manifest.allow_shell = true;
    grant.shell = false;
    let res = gestalt_runtime::check_shell_permission_effective(&manifest, Some(&grant), &event_bus, "test");
    assert!(res.is_err());

    // 3. Only grant allows
    manifest.allow_shell = false;
    grant.shell = true;
    let res = gestalt_runtime::check_shell_permission_effective(&manifest, Some(&grant), &event_bus, "test");
    assert!(res.is_err());
}

#[test]
fn test_permissions_effective_network_wildcard() {
    let mut manifest = dummy_manifest().permissions;
    let mut grant = gestalt_runtime::extension::ExtensionGrantConfig::default();
    let event_bus = RuntimeEventBus::new();

    // Manifest allows wildcard, grant allows example.com
    manifest.allow_network = vec!["*".to_string()];
    grant.network = vec!["example.com".to_string()];

    // example.com allowed
    let res = gestalt_runtime::check_network_permission_effective(&manifest, Some(&grant), true, "example.com", &event_bus, "test");
    assert!(res.is_ok());

    // google.com denied (since grant restricts it)
    let res = gestalt_network_check(&manifest, &grant, "google.com");
    assert!(res.is_err());

    // Grant allows wildcard, manifest allows example.com
    manifest.allow_network = vec!["example.com".to_string()];
    grant.network = vec!["*".to_string()];
    let res = gestalt_runtime::check_network_permission_effective(&manifest, Some(&grant), true, "example.com", &event_bus, "test");
    assert!(res.is_ok());
    
    let res = gestalt_network_check(&manifest, &grant, "google.com");
    assert!(res.is_err());

    // Host policy denies network
    let res = gestalt_runtime::check_network_permission_effective(&manifest, Some(&grant), false, "example.com", &event_bus, "test");
    assert!(res.is_err());
}

fn gestalt_network_check(
    manifest: &Permissions,
    grant: &gestalt_runtime::extension::ExtensionGrantConfig,
    host: &str,
) -> Result<(), String> {
    let event_bus = RuntimeEventBus::new();
    gestalt_runtime::check_network_permission_effective(manifest, Some(grant), true, host, &event_bus, "test")
}

#[test]
fn test_permissions_effective_paths_intersection() {
    let mut manifest = dummy_manifest().permissions;
    let mut grant = gestalt_runtime::extension::ExtensionGrantConfig::default();
    let event_bus = RuntimeEventBus::new();
    let workspace = Path::new("/workspace");

    // Manifest: allow_all_paths = true. Grant: allowed_paths = ["/tmp/allowed"].
    manifest.allow_all_paths = true;
    grant.allowed_paths = vec![std::path::PathBuf::from("/tmp/allowed")];

    let res = gestalt_runtime::check_path_permission_effective(&manifest, Some(&grant), workspace, Path::new("/tmp/allowed/file.txt"), false, &event_bus, "test");
    assert!(res.is_ok());

    let res = gestalt_runtime::check_path_permission_effective(&manifest, Some(&grant), workspace, Path::new("/etc/passwd"), false, &event_bus, "test");
    assert!(res.is_err()); // Restricted by grant layers

    // Read/write independence: workspace read allowed, write denied
    manifest.allow_workspace_read = true;
    manifest.allow_workspace_write = true;
    grant.workspace_read = true;
    grant.workspace_write = false;

    let res = gestalt_runtime::check_path_permission_effective(&manifest, Some(&grant), workspace, Path::new("/workspace/file.txt"), false, &event_bus, "test");
    assert!(res.is_ok());

    let res = gestalt_runtime::check_path_permission_effective(&manifest, Some(&grant), workspace, Path::new("/workspace/file.txt"), true, &event_bus, "test");
    assert!(res.is_err()); // Write denied by grant
}
