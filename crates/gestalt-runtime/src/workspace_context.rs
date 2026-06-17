use std::path::{Path, PathBuf};
use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use gestalt_core::{
    event::PolicyStatus,
    policy::{PolicyEngine, PolicyRequest},
    session::ExecutionMode,
    tool::RiskLevel,
    ContextStability,
};
use gestalt_core::context::{ContextSourceRef, ContextOmission};
use gestalt_core::message::Message;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextSnapshotMode {
    #[default]
    Session,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemorySelectionStrategy {
    Full,
    #[default]
    Budgeted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryWriteMode {
    Disabled,
    #[default]
    Proposal,
}

pub fn default_workspace_path() -> PathBuf {
    PathBuf::from(".gestalt/workspace.md")
}

pub fn default_memory_path() -> PathBuf {
    PathBuf::from(".gestalt/memory.md")
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceContextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContextSnapshotMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct MemoryContextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<MemorySelectionStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<ContextSnapshotMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_mode: Option<MemoryWriteMode>,
}

#[derive(Debug, Error)]
pub enum WorkspaceContextError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Required file missing: {source_kind} at {path}")]
    RequiredMissing {
        source_kind: String,
        path: PathBuf,
    },

    #[error("Source too large: {source_kind} at {path} ({bytes} bytes exceeds {max_bytes} max)")]
    OversizedBytes {
        source_kind: String,
        path: PathBuf,
        bytes: usize,
        max_bytes: usize,
    },

    #[error("Source too large: {source_kind} at {path} ({tokens} tokens exceeds {max_tokens} max)")]
    OversizedTokens {
        source_kind: String,
        path: PathBuf,
        tokens: usize,
        max_tokens: usize,
    },

    #[error("Path traversal escape: {path} is outside workspace root {root}")]
    PathEscape {
        path: PathBuf,
        root: PathBuf,
    },

    #[error("Permission denied for path {path}: {reason}")]
    PermissionDenied {
        path: PathBuf,
        reason: String,
    },

    #[error("Invalid memory format: {reason}")]
    InvalidFormat {
        reason: String,
    },

    #[error("Memory write conflict: expected hash {expected_hash}, actual hash {actual_hash}")]
    MemoryWriteConflict {
        expected_hash: String,
        actual_hash: String,
    },

    #[error("Memory writes are disabled by configuration")]
    MemoryWriteDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub id: String,
    pub section: String,
    pub content: String,
    pub pinned: bool,
    pub source_order: usize,
    pub content_hash: String,
}

pub fn estimate_text_tokens(text: &str) -> usize {
    text.len().saturating_add(3) / 4 + 1
}

pub fn extract_or_generate_id(content: &str, section: &str) -> (String, String) {
    let marker = "<!-- gestalt-memory-id:";
    if let Some(start_idx) = content.find(marker) {
        let after_marker = &content[start_idx + marker.len()..];
        if let Some(end_idx) = after_marker.find("-->") {
            let id = after_marker[..end_idx].trim().to_string();
            let before = &content[..start_idx];
            let after = &after_marker[end_idx + "-->".len()..];
            let cleaned = format!("{}{}", before, after).trim().to_string();
            return (id, cleaned);
        }
    }

    let normalized: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(section.as_bytes());
    hasher.update(b"|");
    hasher.update(normalized.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let generated_id = format!("mem_{}", &hash[..16]);

    (generated_id, content.to_string())
}

pub fn parse_memory_markdown(
    content: &str,
    pinned_section: &str,
) -> Result<Vec<MemoryEntry>, WorkspaceContextError> {
    let mut entries = Vec::new();
    let mut current_section = "General".to_string();
    let mut source_order = 0;

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.starts_with("## ") {
            current_section = line.strip_prefix("## ").unwrap_or(line).trim().to_string();
            i += 1;
            continue;
        } else if line.starts_with("# ") {
            i += 1;
            continue;
        }

        if line.starts_with("- ") || line.starts_with("* ") {
            let marker = if line.starts_with("- ") { "- " } else { "* " };
            let mut entry_lines = vec![line[marker.len()..].to_string()];

            i += 1;
            while i < lines.len() {
                let next_line = lines[i];
                let trimmed = next_line.trim();
                if next_line.starts_with("  ") || next_line.starts_with("\t") || trimmed.is_empty() {
                    if trimmed.starts_with("## ") || trimmed.starts_with("# ") || trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                        break;
                    }
                    entry_lines.push(trimmed.to_string());
                    i += 1;
                } else {
                    break;
                }
            }

            let entry_content = entry_lines.join("\n").trim().to_string();
            if entry_content.is_empty() {
                continue;
            }

            let (id, cleaned_content) = extract_or_generate_id(&entry_content, &current_section);

            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(cleaned_content.as_bytes());
            let content_hash = format!("{:x}", hasher.finalize());

            let pinned = current_section.eq_ignore_ascii_case(pinned_section);

            entries.push(MemoryEntry {
                id,
                section: current_section.clone(),
                content: cleaned_content,
                pinned,
                source_order,
                content_hash,
            });
            source_order += 1;
        } else {
            i += 1;
        }
    }

    Ok(entries)
}

pub struct WorkspaceContextLoader {
    workspace_root: PathBuf,
    policy: Option<Arc<dyn PolicyEngine>>,
}

impl WorkspaceContextLoader {
    pub fn new(workspace_root: PathBuf, policy: Option<Arc<dyn PolicyEngine>>) -> Self {
        Self { workspace_root, policy }
    }

    pub async fn load_workspace_instructions(
        &self,
        config: &WorkspaceContextConfig,
    ) -> Result<Option<String>, WorkspaceContextError> {
        if !config.enabled.unwrap_or(true) {
            return Ok(None);
        }

        let path = config.path.clone().unwrap_or_else(default_workspace_path);
        let resolved = self.resolve_and_validate_path(&path, "workspace instructions").await?;

        if !resolved.exists() {
            if config.required.unwrap_or(false) {
                return Err(WorkspaceContextError::RequiredMissing {
                    source_kind: "workspace instructions".to_string(),
                    path: resolved,
                });
            }
            return Ok(None);
        }

        let content = std::fs::read_to_string(&resolved)?;

        let max_bytes = config.max_bytes.unwrap_or(131072);
        if content.len() > max_bytes {
            return Err(WorkspaceContextError::OversizedBytes {
                source_kind: "workspace instructions".to_string(),
                path: resolved,
                bytes: content.len(),
                max_bytes,
            });
        }

        let tokens = estimate_text_tokens(&content);
        let max_tokens = config.max_tokens.unwrap_or(12000);
        if tokens > max_tokens {
            return Err(WorkspaceContextError::OversizedTokens {
                source_kind: "workspace instructions".to_string(),
                path: resolved,
                tokens,
                max_tokens,
            });
        }

        Ok(Some(content))
    }

    pub async fn load_memory(
        &self,
        config: &MemoryContextConfig,
    ) -> Result<Option<String>, WorkspaceContextError> {
        if !config.enabled.unwrap_or(true) {
            return Ok(None);
        }

        let path = config.path.clone().unwrap_or_else(default_memory_path);
        let resolved = self.resolve_and_validate_path(&path, "workspace memory").await?;

        if !resolved.exists() {
            if config.required.unwrap_or(false) {
                return Err(WorkspaceContextError::RequiredMissing {
                    source_kind: "workspace memory".to_string(),
                    path: resolved,
                });
            }
            return Ok(None);
        }

        let content = std::fs::read_to_string(&resolved)?;

        let max_bytes = config.max_bytes.unwrap_or(524288);
        if content.len() > max_bytes {
            return Err(WorkspaceContextError::OversizedBytes {
                source_kind: "workspace memory".to_string(),
                path: resolved,
                bytes: content.len(),
                max_bytes,
            });
        }



        Ok(Some(content))
    }

    pub async fn resolve_and_validate_path(
        &self,
        path: &Path,
        _source: &str,
    ) -> Result<PathBuf, WorkspaceContextError> {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };

        let canonical_resolved = if resolved.exists() {
            resolved.canonicalize()?
        } else if let Some(parent) = resolved.parent() {
            if parent.exists() {
                parent.canonicalize()?.join(resolved.file_name().unwrap_or_default())
            } else {
                resolved.clone()
            }
        } else {
            resolved.clone()
        };

        let canonical_workspace = self.workspace_root.canonicalize().unwrap_or_else(|_| self.workspace_root.clone());

        let is_inside = canonical_resolved.starts_with(&canonical_workspace);
        if !is_inside {
            if let Some(ref policy) = self.policy {
                let request = PolicyRequest {
                    tool_call_id: "".to_string(),
                    tool_name: "builtin:read".to_string(),
                    namespace: gestalt_core::tool_descriptor::ToolNamespace::BuiltIn,
                    annotations: Default::default(),
                    input: json!({ "path": canonical_resolved.to_string_lossy().to_string() }),
                    risk: RiskLevel::Low,
                    mode: ExecutionMode::Confirm,
                    working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    workspace_root: Some(self.workspace_root.clone()),
                    user_approved: false,
                };
                let decision = policy.evaluate(request).await;
                if decision.status == PolicyStatus::Denied {
                    return Err(WorkspaceContextError::PermissionDenied {
                        path: canonical_resolved,
                        reason: decision.reason.unwrap_or_else(|| "Denied by path policies".to_string()),
                    });
                }
            } else {
                return Err(WorkspaceContextError::PathEscape {
                    path: canonical_resolved,
                    root: canonical_workspace,
                });
            }
        }

        Ok(resolved)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceContextSnapshot {
    pub workspace_instructions_hash: Option<String>,
    pub workspace_instructions_path: Option<PathBuf>,
    pub memory_hash: Option<String>,
    pub memory_path: Option<PathBuf>,
    pub selected_memory_ids: Vec<String>,
    pub config_max_tokens_workspace: Option<usize>,
    pub config_max_tokens_memory: Option<usize>,
    pub parsing_version: String,
}

impl WorkspaceContextSnapshot {
    pub fn compute_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let serialized = serde_json::to_string(self).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

pub struct WorkspaceInstructionContributor {
    content: String,
    source_ref: ContextSourceRef,
}

impl WorkspaceInstructionContributor {
    pub fn new(content: String, source_ref: ContextSourceRef) -> Self {
        Self { content, source_ref }
    }
}

#[async_trait::async_trait]
impl crate::context::ContextContributor for WorkspaceInstructionContributor {
    fn name(&self) -> &str {
        "00_workspace_instructions"
    }

    fn stability(&self) -> ContextStability {
        ContextStability::SessionStatic
    }

    async fn contribute(&self, _workspace_root: &Path) -> crate::error::Result<Message> {
        Ok(Message::System {
            content: format!("workspace.md\n\n{}", self.content),
        })
    }

    fn source(&self, _workspace_root: &Path, _content: &str) -> Option<ContextSourceRef> {
        Some(self.source_ref.clone())
    }
}

pub struct MarkdownMemoryContributor {
    content: String,
    source_ref: ContextSourceRef,
    omissions: Vec<ContextOmission>,
}

impl MarkdownMemoryContributor {
    pub fn new(content: String, source_ref: ContextSourceRef, omissions: Vec<ContextOmission>) -> Self {
        Self { content, source_ref, omissions }
    }
}

#[async_trait::async_trait]
impl crate::context::ContextContributor for MarkdownMemoryContributor {
    fn name(&self) -> &str {
        "01_markdown_memory"
    }

    fn stability(&self) -> ContextStability {
        ContextStability::SessionStatic
    }

    async fn contribute(&self, _workspace_root: &Path) -> crate::error::Result<Message> {
        Ok(Message::System {
            content: format!("memory.md\n\n{}", self.content),
        })
    }

    fn source(&self, _workspace_root: &Path, _content: &str) -> Option<ContextSourceRef> {
        Some(self.source_ref.clone())
    }

    fn omissions(&self, _workspace_root: &Path) -> Vec<ContextOmission> {
        self.omissions.clone()
    }
}

pub fn select_memory_entries(
    entries: &[MemoryEntry],
    strategy: MemorySelectionStrategy,
    max_tokens: usize,
) -> (Vec<MemoryEntry>, Vec<ContextOmission>, usize) {
    let mut selected = Vec::new();
    let mut omissions = Vec::new();
    let mut total_tokens = 0;

    match strategy {
        MemorySelectionStrategy::Full => {
            for entry in entries {
                let rendered = format!("- {}", entry.content);
                let tokens = estimate_text_tokens(&rendered);
                total_tokens += tokens;
                selected.push(entry.clone());
            }
        }
        MemorySelectionStrategy::Budgeted => {
            let (pinned, unpinned): (Vec<_>, Vec<_>) = entries.iter().cloned().partition(|e| e.pinned);

            // Pinned entries survive trimming: they are always included regardless of the budget limit,
            // and do not get omitted.
            for entry in pinned {
                let rendered = format!("- {}", entry.content);
                let tokens = estimate_text_tokens(&rendered);
                total_tokens += tokens;
                selected.push(entry);
            }

            // Unpinned entries fill the remaining budget.
            for entry in unpinned {
                let rendered = format!("- {}", entry.content);
                let tokens = estimate_text_tokens(&rendered);
                if total_tokens + tokens <= max_tokens {
                    total_tokens += tokens;
                    selected.push(entry);
                } else {
                    omissions.push(ContextOmission {
                        kind: "memory".to_string(),
                        path_or_label: format!("mem_id:{}", entry.id),
                        trust: "trusted".to_string(),
                        reason: "budget_exhausted".to_string(),
                        token_estimate: tokens,
                        authority: Some("workspace".to_string()),
                    });
                }
            }
        }
    }

    (selected, omissions, total_tokens)
}

pub fn format_memory_markdown(selected: &[MemoryEntry]) -> String {
    let mut sections = Vec::new();
    for entry in selected {
        if !sections.contains(&entry.section) {
            sections.push(entry.section.clone());
        }
    }

    let mut markdown = String::new();
    markdown.push_str("# Memory\n");
    for section in sections {
        markdown.push_str(&format!("\n## {}\n\n", section));
        for entry in selected {
            if entry.section == section {
                markdown.push_str(&format!("- <!-- gestalt-memory-id: {} --> {}\n", entry.id, entry.content));
            }
        }
    }
    markdown
}

pub async fn load_and_snapshot_workspace_context(
    workspace_root: &Path,
    policy: Option<Arc<dyn PolicyEngine>>,
    event_bus: &crate::event_bus::RuntimeEventBus,
    workspace_config: &WorkspaceContextConfig,
    memory_config: &MemoryContextConfig,
) -> Result<
    (
        Option<WorkspaceInstructionContributor>,
        Option<MarkdownMemoryContributor>,
        WorkspaceContextSnapshot,
    ),
    WorkspaceContextError,
> {
    let loader = WorkspaceContextLoader::new(workspace_root.to_path_buf(), policy);

    let workspace_instructions_path = workspace_config.path.clone().unwrap_or_else(default_workspace_path);
    let mut workspace_instructions_hash = None;
    let mut workspace_instruction_contributor = None;

    if workspace_config.enabled.unwrap_or(true) {
        match loader.load_workspace_instructions(workspace_config).await {
            Ok(Some(content)) => {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(content.as_bytes());
                let hash_str = format!("{:x}", hasher.finalize());
                workspace_instructions_hash = Some(hash_str.clone());

                let tokens = estimate_text_tokens(&content);
                event_bus.publish_agent(gestalt_core::AgentEvent::WorkspaceContextLoaded {
                    path: workspace_instructions_path.to_string_lossy().to_string(),
                    bytes: content.len(),
                    tokens,
                });

                let source_ref = ContextSourceRef {
                    kind: "workspace".to_string(),
                    path_or_label: workspace_instructions_path.to_string_lossy().to_string(),
                    trust: "trusted".to_string(),
                    token_estimate: tokens,
                    included: true,
                    authority: Some("workspace".to_string()),
                };

                workspace_instruction_contributor = Some(WorkspaceInstructionContributor::new(content, source_ref));
                event_bus.publish_agent(gestalt_core::AgentEvent::ContextContributorResolved {
                    name: "00_workspace_instructions".to_string(),
                    stability: format!("{:?}", ContextStability::SessionStatic),
                });
            }
            Ok(None) => {
                event_bus.publish_agent(gestalt_core::AgentEvent::WorkspaceContextSkipped {
                    reason: "not_found".to_string(),
                });
            }
            Err(WorkspaceContextError::PermissionDenied { path, reason }) => {
                event_bus.publish_agent(gestalt_core::AgentEvent::WorkspaceContextRejected {
                    reason: format!("permission_denied: {}", reason),
                });
                return Err(WorkspaceContextError::PermissionDenied { path, reason });
            }
            Err(e) => {
                event_bus.publish_agent(gestalt_core::AgentEvent::WorkspaceContextLoadFailed {
                    error: e.to_string(),
                });
                return Err(e);
            }
        }
    } else {
        event_bus.publish_agent(gestalt_core::AgentEvent::WorkspaceContextSkipped {
            reason: "disabled".to_string(),
        });
    }

    let memory_path = memory_config.path.clone().unwrap_or_else(default_memory_path);
    let mut memory_hash = None;
    let mut selected_memory_ids = Vec::new();
    let mut markdown_memory_contributor = None;

    if memory_config.enabled.unwrap_or(true) {
        match loader.load_memory(memory_config).await {
            Ok(Some(content)) => {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(content.as_bytes());
                let hash_str = format!("{:x}", hasher.finalize());
                memory_hash = Some(hash_str.clone());

                let tokens = estimate_text_tokens(&content);
                event_bus.publish_agent(gestalt_core::AgentEvent::MemoryContextLoaded {
                    path: memory_path.to_string_lossy().to_string(),
                    bytes: content.len(),
                    tokens,
                    strategy: format!("{:?}", memory_config.strategy.unwrap_or(MemorySelectionStrategy::Budgeted)),
                });

                let pinned_sec = memory_config.pinned_section.clone().unwrap_or_else(|| "Facts".to_string());
                let entries = parse_memory_markdown(&content, &pinned_sec)?;

                let max_tokens = memory_config.max_tokens.unwrap_or(8000);
                let strategy = memory_config.strategy.unwrap_or(MemorySelectionStrategy::Budgeted);
                let (selected, omissions, total_tokens) = select_memory_entries(&entries, strategy, max_tokens);
                selected_memory_ids = selected.iter().map(|e| e.id.clone()).collect();

                let pinned_count = selected.iter().filter(|e| e.pinned).count();
                event_bus.publish_agent(gestalt_core::AgentEvent::MemoryEntriesSelected {
                    total_entries: entries.len(),
                    selected_entries: selected.len(),
                    pinned_entries: pinned_count,
                });

                let formatted_content = format_memory_markdown(&selected);
                let source_ref = ContextSourceRef {
                    kind: "memory".to_string(),
                    path_or_label: memory_path.to_string_lossy().to_string(),
                    trust: "trusted".to_string(),
                    token_estimate: total_tokens,
                    included: true,
                    authority: Some("workspace".to_string()),
                };

                markdown_memory_contributor = Some(MarkdownMemoryContributor::new(formatted_content, source_ref, omissions));
                event_bus.publish_agent(gestalt_core::AgentEvent::ContextContributorResolved {
                    name: "01_markdown_memory".to_string(),
                    stability: format!("{:?}", ContextStability::SessionStatic),
                });
            }
            Ok(None) => {
                event_bus.publish_agent(gestalt_core::AgentEvent::MemoryContextSkipped {
                    reason: "not_found".to_string(),
                });
            }
            Err(WorkspaceContextError::PermissionDenied { path, reason }) => {
                event_bus.publish_agent(gestalt_core::AgentEvent::MemoryContextRejected {
                    reason: format!("permission_denied: {}", reason),
                });
                return Err(WorkspaceContextError::PermissionDenied { path, reason });
            }
            Err(e) => {
                event_bus.publish_agent(gestalt_core::AgentEvent::MemoryContextLoadFailed {
                    error: e.to_string(),
                });
                return Err(e);
            }
        }
    } else {
        event_bus.publish_agent(gestalt_core::AgentEvent::MemoryContextSkipped {
            reason: "disabled".to_string(),
        });
    }

    let snapshot = WorkspaceContextSnapshot {
        workspace_instructions_hash,
        workspace_instructions_path: Some(workspace_instructions_path),
        memory_hash,
        memory_path: Some(memory_path),
        selected_memory_ids,
        config_max_tokens_workspace: workspace_config.max_tokens,
        config_max_tokens_memory: memory_config.max_tokens,
        parsing_version: "v1".to_string(),
    };

    let snapshot_hash = snapshot.compute_hash();
    event_bus.publish_agent(gestalt_core::AgentEvent::ContextSnapshotCreated {
        hash: snapshot_hash,
    });

    Ok((
        workspace_instruction_contributor,
        markdown_memory_contributor,
        snapshot,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryProposal {
    pub proposal_id: String,
    pub source_session_id: String,
    pub base_hash: String,
    pub operations: Vec<MemoryOperation>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryOperation {
    Add {
        section: String,
        content: String,
    },
    Replace {
        entry_id: String,
        content: String,
    },
    Remove {
        entry_id: String,
        reason: String,
    },
    Supersede {
        entry_id: String,
        content: String,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryProposalDecision {
    AcceptAll,
    AcceptSelected(Vec<String>),
    Reject,
}

pub async fn apply_memory_proposal(
    workspace_root: &Path,
    memory_config: &MemoryContextConfig,
    proposal: &MemoryProposal,
    decision: &MemoryProposalDecision,
    event_bus: &crate::event_bus::RuntimeEventBus,
    policy: Option<Arc<dyn PolicyEngine>>,
) -> Result<(), WorkspaceContextError> {
    if memory_config.write_mode.unwrap_or(MemoryWriteMode::Proposal) == MemoryWriteMode::Disabled {
        return Err(WorkspaceContextError::MemoryWriteDisabled);
    }

    let proposal_id = &proposal.proposal_id;
    let session_id = &proposal.source_session_id;

    event_bus.publish_agent(gestalt_core::AgentEvent::MemoryProposalCreated {
        session_id: session_id.clone(),
        proposal_id: proposal_id.clone(),
        operation_count: proposal.operations.len(),
    });

    let accepted_ops: Vec<String> = match decision {
        MemoryProposalDecision::AcceptAll => {
            proposal.operations.iter().enumerate().map(|(idx, _)| idx.to_string()).collect()
        }
        MemoryProposalDecision::AcceptSelected(selected) => selected.clone(),
        MemoryProposalDecision::Reject => {
            event_bus.publish_agent(gestalt_core::AgentEvent::MemoryProposalDecisionRecorded {
                proposal_id: proposal_id.clone(),
                decision: "rejected".to_string(),
                accepted_operations: vec![],
            });
            return Ok(());
        }
    };

    event_bus.publish_agent(gestalt_core::AgentEvent::MemoryProposalDecisionRecorded {
        proposal_id: proposal_id.clone(),
        decision: "accepted".to_string(),
        accepted_operations: accepted_ops.clone(),
    });

    if accepted_ops.is_empty() {
        return Ok(());
    }

    let loader = WorkspaceContextLoader::new(workspace_root.to_path_buf(), policy);
    let path = memory_config.path.clone().unwrap_or_else(default_memory_path);
    let resolved = loader.resolve_and_validate_path(&path, "workspace memory").await?;

    let existing_content = if resolved.exists() {
        std::fs::read_to_string(&resolved)?
    } else {
        String::new()
    };

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(existing_content.as_bytes());
    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != proposal.base_hash {
        event_bus.publish_agent(gestalt_core::AgentEvent::MemoryWriteConflict {
            path: path.to_string_lossy().to_string(),
            expected_hash: proposal.base_hash.clone(),
            actual_hash: actual_hash.clone(),
        });
        return Err(WorkspaceContextError::MemoryWriteConflict {
            expected_hash: proposal.base_hash.clone(),
            actual_hash,
        });
    }

    let pinned_sec = memory_config.pinned_section.clone().unwrap_or_else(|| "Facts".to_string());
    let mut entries = if resolved.exists() {
        parse_memory_markdown(&existing_content, &pinned_sec)?
    } else {
        Vec::new()
    };

    for op_id_str in &accepted_ops {
        let op_idx: usize = match op_id_str.parse() {
            Ok(idx) => idx,
            Err(_) => continue,
        };
        if op_idx >= proposal.operations.len() {
            continue;
        }
        let op = &proposal.operations[op_idx];
        match op {
            MemoryOperation::Add { section, content } => {
                let (id, cleaned_content) = extract_or_generate_id(content, section);
                let mut hasher = Sha256::new();
                hasher.update(cleaned_content.as_bytes());
                let content_hash = format!("{:x}", hasher.finalize());
                let pinned = section.eq_ignore_ascii_case(&pinned_sec);
                let source_order = entries.len();

                entries.push(MemoryEntry {
                    id,
                    section: section.clone(),
                    content: cleaned_content,
                    pinned,
                    source_order,
                    content_hash,
                });
            }
            MemoryOperation::Replace { entry_id, content } => {
                if let Some(entry) = entries.iter_mut().find(|e| e.id == *entry_id) {
                    let (_, cleaned_content) = extract_or_generate_id(content, &entry.section);
                    let mut hasher = Sha256::new();
                    hasher.update(cleaned_content.as_bytes());
                    entry.content_hash = format!("{:x}", hasher.finalize());
                    entry.content = cleaned_content;
                }
            }
            MemoryOperation::Remove { entry_id, .. } => {
                entries.retain(|e| e.id != *entry_id);
            }
            MemoryOperation::Supersede { entry_id, content, .. } => {
                if let Some(entry) = entries.iter_mut().find(|e| e.id == *entry_id) {
                    let (_, cleaned_content) = extract_or_generate_id(content, &entry.section);
                    let mut hasher = Sha256::new();
                    hasher.update(cleaned_content.as_bytes());
                    entry.content_hash = format!("{:x}", hasher.finalize());
                    entry.content = cleaned_content;
                }
            }
        }
    }

    let new_content = format_memory_markdown(&entries);

    let temp_dir = resolved.parent().unwrap_or(workspace_root);
    let temp_path = temp_dir.join(format!(".memory.tmp-{}", uuid::Uuid::new_v4()));

    if let Err(e) = std::fs::write(&temp_path, new_content.as_bytes()) {
        event_bus.publish_agent(gestalt_core::AgentEvent::MemoryWriteFailed {
            path: path.to_string_lossy().to_string(),
            error: e.to_string(),
        });
        let _ = std::fs::remove_file(&temp_path);
        return Err(WorkspaceContextError::Io(e));
    }

    if let Err(e) = std::fs::rename(&temp_path, &resolved) {
        event_bus.publish_agent(gestalt_core::AgentEvent::MemoryWriteFailed {
            path: path.to_string_lossy().to_string(),
            error: e.to_string(),
        });
        let _ = std::fs::remove_file(&temp_path);
        return Err(WorkspaceContextError::Io(e));
    }

    event_bus.publish_agent(gestalt_core::AgentEvent::MemoryWriteSucceeded {
        path: path.to_string_lossy().to_string(),
        bytes: new_content.len(),
    });

    Ok(())
}

