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
    check_path_permission_effective(
        &manifest.permissions,
        None,
        workspace_root,
        path,
        write,
        event_bus,
        &manifest.id,
    )
}

pub fn check_path_permission_effective(
    manifest: &crate::manifest::Permissions,
    grant: Option<&crate::extension::ExtensionGrantConfig>,
    workspace_root: &Path,
    path: &Path,
    write: bool,
    event_bus: &RuntimeEventBus,
    extension_id: &str,
) -> std::result::Result<(), String> {
    let result = check_path_permission_effective_impl(manifest, grant, workspace_root, path, write);
    event_bus.publish(RuntimeEvent::PermissionDecision {
        extension_id: extension_id.to_string(),
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

fn check_path_permission_effective_impl(
    manifest: &crate::manifest::Permissions,
    grant: Option<&crate::extension::ExtensionGrantConfig>,
    workspace_root: &Path,
    path: &Path,
    write: bool,
) -> std::result::Result<(), String> {
    check_manifest_path_permission(manifest, workspace_root, path, write)?;
    if let Some(grant) = grant {
        check_grant_path_permission(grant, workspace_root, path, write)?;
    }
    Ok(())
}

fn check_manifest_path_permission(
    manifest: &crate::manifest::Permissions,
    workspace_root: &Path,
    path: &Path,
    write: bool,
) -> std::result::Result<(), String> {
    if manifest.allow_all_paths {
        return Ok(());
    }

    let canonical_workspace = resolve_path_for_permission_check(workspace_root);
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    let canonical_path = resolve_path_for_permission_check(&abs_path);

    if canonical_path.starts_with(&canonical_workspace) {
        if write {
            if manifest.allow_workspace_write {
                return Ok(());
            } else {
                return Err("Workspace write permission not granted by manifest".to_string());
            }
        } else {
            if manifest.allow_workspace_read {
                return Ok(());
            } else {
                return Err("Workspace read permission not granted by manifest".to_string());
            }
        }
    }

    for allowed in &manifest.allowed_paths {
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

fn check_grant_path_permission(
    grant: &crate::extension::ExtensionGrantConfig,
    workspace_root: &Path,
    path: &Path,
    write: bool,
) -> std::result::Result<(), String> {
    let grant_allows_all = grant
        .allowed_paths
        .iter()
        .any(|p| p.to_string_lossy() == "*");
    if grant_allows_all {
        return Ok(());
    }

    let canonical_workspace = resolve_path_for_permission_check(workspace_root);
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    let canonical_path = resolve_path_for_permission_check(&abs_path);

    if canonical_path.starts_with(&canonical_workspace) {
        if write {
            if grant.workspace_write {
                return Ok(());
            } else {
                return Err("Workspace write permission not granted by instance grant".to_string());
            }
        } else {
            if grant.workspace_read {
                return Ok(());
            } else {
                return Err("Workspace read permission not granted by instance grant".to_string());
            }
        }
    }

    for allowed in &grant.allowed_paths {
        let canonical_allowed = resolve_path_for_permission_check(allowed);
        if canonical_path.starts_with(&canonical_allowed) {
            return Ok(());
        }
    }

    Err(format!(
        "Access to path '{:?}' is not allowed by instance grant permissions",
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

    let remainder = path.strip_prefix(ancestor).unwrap_or(path);
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
    check_network_permission_effective(
        &manifest.permissions,
        None,
        true,
        host,
        event_bus,
        &manifest.id,
    )
}

pub fn check_network_permission_effective(
    manifest: &crate::manifest::Permissions,
    grant: Option<&crate::extension::ExtensionGrantConfig>,
    host_allow_network: bool,
    host: &str,
    event_bus: &RuntimeEventBus,
    extension_id: &str,
) -> std::result::Result<(), String> {
    let mut manifest_ok = false;
    for allowed in &manifest.allow_network {
        if allowed == "*" || allowed == host {
            manifest_ok = true;
            break;
        }
    }

    let grant_ok = if let Some(grant) = grant {
        let mut ok = false;
        for allowed in &grant.network {
            if allowed == "*" || allowed == host {
                ok = true;
                break;
            }
        }
        ok
    } else {
        true
    };

    let granted = manifest_ok && grant_ok && host_allow_network;

    let result = if granted {
        Ok(())
    } else {
        Err(format!(
            "Network access to host '{}' is not allowed by effective permissions",
            host
        ))
    };

    event_bus.publish(RuntimeEvent::PermissionDecision {
        extension_id: extension_id.to_string(),
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
    check_shell_permission_effective(&manifest.permissions, None, event_bus, &manifest.id)
}

pub fn check_shell_permission_effective(
    manifest: &crate::manifest::Permissions,
    grant: Option<&crate::extension::ExtensionGrantConfig>,
    event_bus: &RuntimeEventBus,
    extension_id: &str,
) -> std::result::Result<(), String> {
    let grant_ok = grant.map_or(true, |g| g.shell);
    let granted = manifest.allow_shell && grant_ok;
    let result = if granted {
        Ok(())
    } else {
        Err("Shell execution permission is false".to_string())
    };

    event_bus.publish(RuntimeEvent::PermissionDecision {
        extension_id: extension_id.to_string(),
        capability: "shell".to_string(),
        permission: "execute".to_string(),
        resource: None,
        granted,
        reason: result.as_ref().err().cloned(),
    });

    result
}
