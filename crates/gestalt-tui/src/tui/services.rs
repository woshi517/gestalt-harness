use crate::config::EffectiveConfig;
use crate::sessions::{self, RunManifestSummary, SessionSummary};
use crate::tui::state::{push_event, TranscriptEntry};
use gestalt_core::error::HarnessError;
use gestalt_runtime::unstable::run_manifest::RunManifest;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SessionListModel {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone)]
pub struct LineageNode {
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub lifecycle_state: String,
    pub turns: usize,
    pub depth: usize,
    pub is_last_child: bool,
    pub prefix: String,
}

#[derive(Debug, Clone)]
pub struct LineageTreeModel {
    pub session_id: String,
    pub nodes: Vec<LineageNode>,
}

pub fn load_session_list(config: &EffectiveConfig) -> Result<SessionListModel, HarnessError> {
    let report = sessions::list_sessions(config)?;
    Ok(SessionListModel {
        sessions: report.sessions,
    })
}

pub fn load_lineage_tree(
    config: &EffectiveConfig,
    session_id: &str,
) -> Result<LineageTreeModel, HarnessError> {
    let report = match sessions::inspect_session(config, session_id) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "Failed to load lineage tree for session {}: {:?}",
                session_id,
                e
            );
            return Ok(LineageTreeModel {
                session_id: session_id.to_string(),
                nodes: Vec::new(),
            });
        }
    };

    let mut id_to_run = HashMap::new();
    let mut parent_to_children: HashMap<Option<String>, Vec<&RunManifestSummary>> = HashMap::new();

    for run in &report.runs {
        id_to_run.insert(run.run_id.clone(), run);
        parent_to_children
            .entry(run.parent_run_id.clone())
            .or_default()
            .push(run);
    }

    let mut roots = Vec::new();
    for run in &report.runs {
        let is_root = match &run.parent_run_id {
            None => true,
            Some(parent_id) => !id_to_run.contains_key(parent_id),
        };
        if is_root {
            roots.push(run);
        }
    }
    roots.sort_by_key(|r| r.created_at);

    let mut nodes = Vec::new();
    let roots_len = roots.len();

    for (idx, root) in roots.iter().enumerate() {
        traverse(
            &root.run_id,
            0,
            idx == roots_len - 1,
            "",
            &parent_to_children,
            &id_to_run,
            &mut nodes,
        );
    }

    Ok(LineageTreeModel {
        session_id: session_id.to_string(),
        nodes,
    })
}

fn traverse(
    run_id: &str,
    depth: usize,
    is_last_child: bool,
    prefix: &str,
    parent_to_children: &HashMap<Option<String>, Vec<&RunManifestSummary>>,
    id_to_run: &HashMap<String, &RunManifestSummary>,
    nodes: &mut Vec<LineageNode>,
) {
    if let Some(run) = id_to_run.get(run_id) {
        nodes.push(LineageNode {
            run_id: run.run_id.clone(),
            parent_run_id: run.parent_run_id.clone(),
            created_at: run.created_at,
            lifecycle_state: run.lifecycle_state.clone(),
            turns: run.turns,
            depth,
            is_last_child,
            prefix: prefix.to_string(),
        });

        if let Some(children) = parent_to_children.get(&Some(run.run_id.clone())) {
            let mut sorted_children = children.clone();
            sorted_children.sort_by_key(|c| c.created_at);
            let len = sorted_children.len();
            let new_prefix = format!("{}{}", prefix, if is_last_child { "   " } else { "│  " });
            for (idx, child) in sorted_children.iter().enumerate() {
                traverse(
                    &child.run_id,
                    depth + 1,
                    idx == len - 1,
                    &new_prefix,
                    parent_to_children,
                    id_to_run,
                    nodes,
                );
            }
        }
    }
}

pub fn load_session_transcript(
    config: &EffectiveConfig,
    session_id: &str,
    parent_run_id: Option<&str>,
) -> Result<Vec<TranscriptEntry>, HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut manifests_map = std::collections::HashMap::new();
    let mut run_paths_map = std::collections::HashMap::new();

    if run_log_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                let manifest_path = entry.path().join("run.json");
                if manifest_path.exists() {
                    if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                        if manifest.session_id == session_id {
                            manifests_map.insert(manifest.run_id.clone(), manifest.clone());
                            run_paths_map.insert(manifest.run_id.clone(), entry.path());
                        }
                    }
                }
            }
        }
    }

    let mut path = Vec::new();
    let mut current_id = parent_run_id.map(String::from);
    while let Some(id) = current_id {
        if let Some(manifest) = manifests_map.get(&id) {
            path.push(id.clone());
            current_id = manifest.parent_run_id.clone();
        } else {
            break;
        }
    }
    path.reverse();

    let mut entries = Vec::new();
    for run_id in path {
        if let Some(run_path) = run_paths_map.get(&run_id) {
            let trace_path = run_path.join("trace.jsonl");
            if trace_path.exists() {
                if let Ok(envelopes) = gestalt_runtime::unstable::read_trace(&trace_path) {
                    for env in envelopes {
                        let event =
                            gestalt_core::AgentEvent::try_from(env.event).map_err(|err| {
                                HarnessError::Trace(gestalt_core::TraceError::InvalidFormat {
                                    line: 0,
                                    reason: format!(
                                        "trace event cannot be projected into transcript: {err}"
                                    ),
                                })
                            })?;
                        push_event(&mut entries, event);
                    }
                }
            }
        }
    }

    Ok(entries)
}
