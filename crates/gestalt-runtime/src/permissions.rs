use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::manifest::ExtensionManifest;
use std::path::{Component, Path, PathBuf};

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

    let canonical_workspace = resolve_path_for_permission_check(workspace_root);

    // We do absolute representation to prevent traversal issues
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };

    let canonical_path = resolve_path_for_permission_check(&abs_path);

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
        let canonical_allowed = resolve_path_for_permission_check(allowed_path);
        if canonical_path.starts_with(&canonical_allowed) {
            return Ok(());
        }
    }

    Err(format!(
        "Access to path '{:?}' is not allowed by manifest permissions",
        path
    ))
}

fn resolve_path_for_permission_check(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    resolve_from_existing_ancestor(path)
}

fn resolve_from_existing_ancestor(path: &Path) -> PathBuf {
    let mut ancestor = path;
    while !ancestor.exists() {
        let Some(parent) = ancestor.parent() else {
            return path.to_path_buf();
        };
        ancestor = parent;
    }

    let Ok(mut resolved) = ancestor.canonicalize() else {
        return path.to_path_buf();
    };

    let remainder = path
        .strip_prefix(ancestor)
        .expect("existing ancestor must be a prefix of the resolved path");
    resolved.push(remainder);
    normalize_path(&resolved)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
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
