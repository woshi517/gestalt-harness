use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};
use glob::Pattern;
use std::fs;

use gestalt_core::{ToolContext, ToolError};

pub fn validate_existing_path(input_path: &str, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    reject_secret_path(input_path)?;
    let resolved = resolve_input_path(input_path, ctx);
    let canonical = resolved
        .canonicalize()
        .map_err(ToolError::ExecutionFailed)?;
    ensure_inside_workspace(&canonical, ctx)?;
    Ok(canonical)
}

pub fn validate_write_path(input_path: &str, ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    reject_secret_path(input_path)?;
    let resolved = resolve_input_path(input_path, ctx);
    reject_parent_escape(&resolved)?;

    if resolved.exists() {
        let canonical = resolved
            .canonicalize()
            .map_err(ToolError::ExecutionFailed)?;
        ensure_inside_workspace(&canonical, ctx)?;
        return Ok(canonical);
    }

    let existing_ancestor = first_existing_ancestor(&resolved)?;
    let canonical_ancestor = existing_ancestor
        .canonicalize()
        .map_err(ToolError::ExecutionFailed)?;
    ensure_inside_workspace(&canonical_ancestor, ctx)?;

    Ok(resolved)
}

pub fn validate_child_dir(
    input_path: Option<&str>,
    ctx: &ToolContext,
) -> Result<PathBuf, ToolError> {
    if let Some(path) = input_path {
        return validate_existing_path(path, ctx);
    }

    let canonical = ctx
        .working_dir
        .canonicalize()
        .map_err(ToolError::ExecutionFailed)?;
    ensure_inside_workspace(&canonical, ctx)?;
    Ok(canonical)
}

fn resolve_input_path(input_path: &str, ctx: &ToolContext) -> PathBuf {
    let path = Path::new(input_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.working_dir.join(path)
    }
}

fn first_existing_ancestor(path: &Path) -> Result<PathBuf, ToolError> {
    let mut current = path
        .parent()
        .ok_or_else(|| invalid_input("path has no parent"))?
        .to_path_buf();
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| invalid_input("no existing path ancestor"))?
            .to_path_buf();
    }
    Ok(current)
}

fn ensure_inside_workspace(path: &Path, ctx: &ToolContext) -> Result<(), ToolError> {
    if let Some(root) = &ctx.workspace_root {
        let canonical_root = root.canonicalize().map_err(ToolError::ExecutionFailed)?;
        if !path.starts_with(&canonical_root) {
            return Err(ToolError::PathNotAllowed(path.display().to_string()));
        }
    }

    Ok(())
}

fn reject_parent_escape(path: &Path) -> Result<(), ToolError> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(ToolError::PathNotAllowed(path.display().to_string()));
    }
    Ok(())
}

fn reject_secret_path(path: &str) -> Result<(), ToolError> {
    let lower = path.to_ascii_lowercase();
    let file_name = Path::new(path)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if file_name == ".env"
        || file_name.starts_with(".env.")
        || has_extension(&file_name, "key")
        || has_extension(&file_name, "pem")
        || lower.contains("secret")
        || lower.contains("credential")
    {
        return Err(ToolError::PathNotAllowed(path.to_string()));
    }

    Ok(())
}

fn has_extension(path: &str, extension: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

fn invalid_input(reason: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool_name: "path".to_string(),
        reason: reason.into(),
    }
}

pub fn is_secret_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    file_name == ".env"
        || file_name.starts_with(".env.")
        || has_extension(&file_name, "key")
        || has_extension(&file_name, "pem")
        || lower.contains("secret")
        || lower.contains("credential")
        || lower.contains("secrets/")
        || lower.contains("/secrets/")
        || lower.contains("/secret/")
}

pub fn is_hidden_descendant(relative_path: &Path) -> bool {
    relative_path.components().any(|comp| {
        if let std::path::Component::Normal(name) = comp {
            name.to_string_lossy().starts_with('.')
        } else {
            false
        }
    })
}

#[derive(Debug, Clone)]
pub struct GitignorePattern {
    pattern: Pattern,
    is_bare: bool,
    is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct GitignoreFilter {
    patterns: Vec<GitignorePattern>,
}

impl GitignoreFilter {
    pub fn parse_lines(content: &str) -> Vec<GitignorePattern> {
        let mut patterns = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut p = trimmed.to_string();
            let is_anchored = p.starts_with('/');
            if is_anchored {
                p.remove(0);
            }
            let mut is_dir = false;
            if p.ends_with('/') {
                p.pop();
                is_dir = true;
            }
            let is_bare = !is_anchored && !p.contains('/');
            if let Ok(pat) = Pattern::new(&p) {
                patterns.push(GitignorePattern {
                    pattern: pat,
                    is_bare,
                    is_dir,
                });
            }
        }
        patterns
    }

    pub fn new(workspace_root: &Path) -> Self {
        let gitignore_path = workspace_root.join(".gitignore");
        let patterns = if let Ok(content) = fs::read_to_string(gitignore_path) {
            Self::parse_lines(&content)
        } else {
            Vec::new()
        };
        Self { patterns }
    }

    pub fn from_patterns(raw_patterns: &[String]) -> Self {
        let mut patterns = Vec::new();
        for pattern in raw_patterns {
            let trimmed = pattern.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut p = trimmed.to_string();
            let is_anchored = p.starts_with('/');
            if is_anchored {
                p.remove(0);
            }
            let mut is_dir = false;
            if p.ends_with('/') {
                p.pop();
                is_dir = true;
            }
            let is_bare = !is_anchored && !p.contains('/');
            if let Ok(pat) = Pattern::new(&p) {
                patterns.push(GitignorePattern {
                    pattern: pat,
                    is_bare,
                    is_dir,
                });
            }
        }
        Self { patterns }
    }

    pub fn is_ignored(&self, relative_path: &Path) -> bool {
        let rel_str = relative_path.to_string_lossy().to_string();
        let components: Vec<String> = relative_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();

        for gp in &self.patterns {
            if gp.is_bare {
                for comp in &components {
                    if gp.pattern.matches(comp) {
                        return true;
                    }
                }
            } else {
                if gp.pattern.matches(&rel_str) {
                    return true;
                }
                let prefix = gp.pattern.as_str();
                if rel_str.starts_with(&format!("{prefix}/")) {
                    return true;
                }
            }
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct PathFilter {
    workspace_root: Option<PathBuf>,
    gitignore: Option<GitignoreFilter>,
    custom_ignore: Option<GitignoreFilter>,
    include_hidden: bool,
    respect_gitignore: bool,
    search_root: PathBuf,
}

impl PathFilter {
    pub fn new(
        ctx: &ToolContext,
        search_root: &Path,
        include_hidden: bool,
        respect_gitignore: bool,
    ) -> Self {
        let workspace_root = ctx.workspace_root.clone();
        let gitignore = if respect_gitignore {
            workspace_root.as_ref().map(|root| GitignoreFilter::new(root))
        } else {
            None
        };
        let custom_ignore = if !ctx.ignore_patterns.is_empty() {
            Some(GitignoreFilter::from_patterns(&ctx.ignore_patterns))
        } else {
            None
        };
        Self {
            workspace_root,
            gitignore,
            custom_ignore,
            include_hidden,
            respect_gitignore,
            search_root: search_root.to_path_buf(),
        }
    }

    pub fn is_visible(&self, abs_path: &Path) -> bool {
        if is_secret_path(abs_path) {
            return false;
        }

        if !self.include_hidden {
            if let Ok(rel) = abs_path.strip_prefix(&self.search_root) {
                if is_hidden_descendant(rel) {
                    return false;
                }
            }
        }

        if self.respect_gitignore {
            if let Some(ref gitignore) = self.gitignore {
                if let Some(ref ws_root) = self.workspace_root {
                    if let Ok(rel) = abs_path.strip_prefix(ws_root) {
                        if gitignore.is_ignored(rel) {
                            return false;
                        }
                    }
                }
            }
        }

        if let Some(ref custom_ignore) = self.custom_ignore {
            if let Some(ref ws_root) = self.workspace_root {
                if let Ok(rel) = abs_path.strip_prefix(ws_root) {
                    if custom_ignore.is_ignored(rel) {
                        return false;
                    }
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn test_gitignore_bare_and_anchored_patterns() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(
            root.join(".gitignore"),
            "ignored.rs\ntarget/\n/anchored.txt\nsrc/nested_ignored.rs\n",
        )
        .unwrap();

        let filter = GitignoreFilter::new(root);

        assert!(filter.is_ignored(Path::new("ignored.rs")));
        assert!(filter.is_ignored(Path::new("src/ignored.rs")));
        assert!(filter.is_ignored(Path::new("crates/lib/ignored.rs")));

        assert!(filter.is_ignored(Path::new("target")));
        assert!(filter.is_ignored(Path::new("target/debug/app")));
        assert!(filter.is_ignored(Path::new("crates/target")));
        assert!(filter.is_ignored(Path::new("crates/target/debug/app")));

        assert!(filter.is_ignored(Path::new("anchored.txt")));
        assert!(!filter.is_ignored(Path::new("src/anchored.txt")));

        assert!(filter.is_ignored(Path::new("src/nested_ignored.rs")));
        assert!(!filter.is_ignored(Path::new("nested_ignored.rs")));
        assert!(!filter.is_ignored(Path::new("crates/src/nested_ignored.rs")));

        assert!(!filter.is_ignored(Path::new("main.rs")));
        assert!(!filter.is_ignored(Path::new("src/main.rs")));
    }

    #[test]
    fn test_custom_ignore_patterns() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let ctx = ToolContext {
            working_dir: root.to_path_buf(),
            workspace_root: Some(root.to_path_buf()),
            timeout: std::time::Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 1024,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: vec![
                "custom_ignored.rs".to_string(),
                "node_modules/".to_string(),
                "/root_only.txt".to_string(),
            ],
        };

        let filter = PathFilter::new(&ctx, root, false, true);

        // custom_ignored.rs is ignored bare-name
        assert!(!filter.is_visible(&root.join("custom_ignored.rs")));
        assert!(!filter.is_visible(&root.join("src/custom_ignored.rs")));

        // node_modules/ is ignored directory
        assert!(!filter.is_visible(&root.join("node_modules")));
        assert!(!filter.is_visible(&root.join("node_modules/lib/index.js")));
        assert!(!filter.is_visible(&root.join("packages/app/node_modules/lib/index.js")));

        // root_only.txt is anchored
        assert!(!filter.is_visible(&root.join("root_only.txt")));
        assert!(filter.is_visible(&root.join("src/root_only.txt")));

        // normal files are visible
        assert!(filter.is_visible(&root.join("main.rs")));
        assert!(filter.is_visible(&root.join("src/main.rs")));
    }
}
