use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::manifest::ExtensionManifest;
use std::path::Path;

pub fn check_path_permission(
    manifest: &ExtensionManifest,
    workspace_root: &Path,
    path: &Path,
    write: bool,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), String> {
    let result = check_path_permission_impl(manifest, workspace_root, path, write);
    event_bus.publish(RuntimeEvent::PermissionDecision {
        extension_id: manifest.id.clone(),
        capability: "filesystem".to_string(),
        permission: if write {
            "write".to_string()
        } else {
            "read".to_string()
        },
        resource: Some(path.to_string_lossy().to_string()),
        granted: result.is_ok(),
        reason: result.as_ref().err().cloned(),
    });
    result
}

fn check_path_permission_impl(
    manifest: &ExtensionManifest,
    workspace_root: &Path,
    path: &Path,
    write: bool,
) -> std::result::Result<(), String> {
    if manifest.permissions.allow_all_paths {
        return Ok(());
    }

    let canonical_workspace = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());

    // We do absolute representation to prevent traversal issues
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };

    let canonical_path = abs_path.canonicalize().unwrap_or_else(|_| abs_path.clone());

    // Check traversal outside workspace root
    if canonical_path.starts_with(&canonical_workspace) {
        if write {
            if manifest.permissions.allow_workspace_write {
                return Ok(());
            } else {
                return Err("Workspace write permission not granted".to_string());
            }
        } else {
            if manifest.permissions.allow_workspace_read {
                return Ok(());
            } else {
                return Err("Workspace read permission not granted".to_string());
            }
        }
    }

    for allowed in &manifest.permissions.allowed_paths {
        let allowed_path = Path::new(allowed);
        let canonical_allowed = allowed_path
            .canonicalize()
            .unwrap_or_else(|_| allowed_path.to_path_buf());
        if canonical_path.starts_with(&canonical_allowed) {
            return Ok(());
        }
    }

    Err(format!(
        "Access to path '{:?}' is not allowed by manifest permissions",
        path
    ))
}

pub fn check_network_permission(
    manifest: &ExtensionManifest,
    host: &str,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), String> {
    let mut granted = false;
    for allowed in &manifest.permissions.allow_network {
        if allowed == "*" || allowed == host {
            granted = true;
            break;
        }
    }

    let result = if granted {
        Ok(())
    } else {
        Err(format!(
            "Network access to host '{}' is not allowed by manifest permissions",
            host
        ))
    };

    event_bus.publish(RuntimeEvent::PermissionDecision {
        extension_id: manifest.id.clone(),
        capability: "network".to_string(),
        permission: "connect".to_string(),
        resource: Some(host.to_string()),
        granted,
        reason: result.as_ref().err().cloned(),
    });

    result
}

pub fn check_shell_permission(
    manifest: &ExtensionManifest,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), String> {
    let granted = manifest.permissions.allow_shell;
    let result = if granted {
        Ok(())
    } else {
        Err("Shell execution permission is false".to_string())
    };

    event_bus.publish(RuntimeEvent::PermissionDecision {
        extension_id: manifest.id.clone(),
        capability: "shell".to_string(),
        permission: "execute".to_string(),
        resource: None,
        granted,
        reason: result.as_ref().err().cloned(),
    });

    result
}
