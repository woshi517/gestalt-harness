use crate::SkillError;
use std::path::{Path, PathBuf};

/// Resolve a resource path relative to a skill root, rejecting escapes.
pub fn resolve_skill_resource(
    skill_root: &Path,
    resource_path: &str,
) -> std::result::Result<PathBuf, SkillError> {
    let resolved = skill_root.join(resource_path);
    let canonical = resolved.canonicalize().or_else(|_| {
        // If the file doesn't exist yet, canonicalize fails.
        // We can still validate by normalizing the path manually.
        Ok::<_, std::io::Error>(normalize_path(&resolved))
    })?;

    let root_canonical = skill_root
        .canonicalize()
        .unwrap_or_else(|_| skill_root.to_path_buf());

    if !canonical.starts_with(&root_canonical) {
        return Err(SkillError::ResourceEscape(canonical));
    }

    Ok(canonical)
}

/// Callback type used to record that a resource access was attempted for a
/// given skill. The runtime wires this to publish a
/// `RuntimeEvent::SkillResourceAccessed` event on the event bus.
pub type ResourceAccessRecorder = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Resolve a resource and invoke the recorder on success.
///
/// This is the canonical wrapper runtime tools should use. The bare
/// `resolve_skill_resource` remains available for tests and pure-path
/// operations.
pub fn resolve_skill_resource_tracked(
    skill_name: &str,
    skill_root: &Path,
    resource_path: &str,
    recorder: Option<&ResourceAccessRecorder>,
) -> std::result::Result<PathBuf, SkillError> {
    let resolved = resolve_skill_resource(skill_root, resource_path)?;
    if let Some(rec) = recorder {
        rec(skill_name, resource_path);
    }
    Ok(resolved)
}

/// Normalize a path without requiring it to exist (removes `.` and `..`).
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::Normal(name) => result.push(name),
        }
    }
    let base = if path.has_root() {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    result.into_iter().fold(base, |mut p, c| {
        p.push(c);
        p
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_inside() {
        let root = PathBuf::from("/tmp/skill");
        let r = resolve_skill_resource(&root, "references/REF.md").unwrap();
        assert!(r.to_string_lossy().contains("references/REF.md"));
    }

    #[test]
    fn test_reject_escape() {
        let root = PathBuf::from("/tmp/skill");
        let r = resolve_skill_resource(&root, "../outside.md");
        assert!(r.is_err());
    }

    #[test]
    fn test_tracked_records_no_access_on_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let root = PathBuf::from("/tmp/skill");
        let counter = Arc::new(AtomicUsize::new(0));
        let recorder: ResourceAccessRecorder = {
            let c = counter.clone();
            Arc::new(move |_name, _path| {
                c.fetch_add(1, Ordering::SeqCst);
            })
        };
        // Use a path-escape to force a resolution failure. The bare
        // resolve_skill_resource function rejects escapes via
        // `SkillError::ResourceEscape`, so the tracked wrapper must propagate
        // the error and must NOT record an access for the rejected request.
        let resolved =
            resolve_skill_resource_tracked("test-skill", &root, "../outside.md", Some(&recorder));
        assert!(resolved.is_err());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "no access event should be recorded for an unresolved resource"
        );
    }
}
