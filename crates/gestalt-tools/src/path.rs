use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

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
