use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use gestalt_core::ToolContext;

pub(super) fn temp_workspace(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("gestalt-tools-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create temp workspace");
    root
}

pub(super) fn ctx(root: &Path) -> ToolContext {
    ToolContext {
        working_dir: root.to_path_buf(),
        workspace_root: Some(root.to_path_buf()),
        timeout: Duration::from_secs(2),
        allow_network: false,
        environment: HashMap::new(),
        max_output_bytes: 128,
        artifact_dir: None,
        current_tool_call_id: None,
        ignore_patterns: Vec::new(),
    }
}
