use crate::message::{Message, MessageMetadata};
use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn sha256_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hash_message(message: &Message) -> String {
    serde_json::to_string(message)
        .map(|serialized| sha256_hash(&serialized))
        .unwrap_or_default()
}

fn hash_messages(messages: &[Message]) -> Vec<String> {
    messages.iter().map(hash_message).collect()
}

fn hash_message_list(messages: &[Message], created_turn: usize) -> String {
    let serialized = serde_json::to_string(messages).unwrap_or_default();
    sha256_hash(&format!("{serialized}:{created_turn}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptAssemblyStrategy {
    #[default]
    Dynamic,
    Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptSegmentKind {
    #[default]
    Conversation,
    Snapshot,
    Dynamic,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextStability {
    SessionStatic,
    ActivationStatic,
    #[default]
    TurnDynamic,
    Ephemeral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSegment {
    pub kind: PromptSegmentKind,
    pub stability: ContextStability,
    pub hash: String,
    pub message_count: usize,
}

impl PromptSegment {
    pub fn from_messages(
        kind: PromptSegmentKind,
        stability: ContextStability,
        messages: &[Message],
    ) -> Self {
        let message_hashes = hash_messages(messages);
        let hash = sha256_hash(
            &serde_json::to_string(&serde_json::json!({
                "kind": kind,
                "stability": stability,
                "message_hashes": message_hashes,
            }))
            .unwrap_or_default(),
        );

        Self {
            kind,
            stability,
            hash,
            message_count: messages.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSnapshot {
    pub snapshot_hash: String,
    pub prefix_hash: String,
    pub created_turn: usize,
    pub messages: Vec<Message>,
    pub message_hashes: Vec<String>,
}

impl PromptSnapshot {
    pub fn new(messages: Vec<Message>, created_turn: usize) -> Self {
        let prefix_hash = sha256_hash(&serde_json::to_string(&messages).unwrap_or_default());
        let message_hashes = hash_messages(&messages);
        let snapshot_hash = hash_message_list(&messages, created_turn);

        Self {
            snapshot_hash,
            prefix_hash,
            created_turn,
            messages,
            message_hashes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCacheKey {
    pub provider_id: String,
    pub api_format: crate::provider::ApiFormat,
    pub model_id: String,
    pub prompt_prefix_hash: String,
    pub tool_schema_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptCachePlan {
    pub strategy: PromptAssemblyStrategy,
    pub snapshot_hash: String,
    pub prefix_hash: String,
    pub prefix_message_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<PromptSegment>,
}

impl PromptCachePlan {
    pub fn new(strategy: PromptAssemblyStrategy, snapshot: &PromptSnapshot) -> Self {
        Self {
            strategy,
            snapshot_hash: snapshot.snapshot_hash.clone(),
            prefix_hash: snapshot.prefix_hash.clone(),
            prefix_message_count: snapshot.messages.len(),
            segments: Vec::new(),
        }
    }

    pub fn with_segments(mut self, segments: Vec<PromptSegment>) -> Self {
        self.segments = segments;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPacket {
    pub messages: Vec<Message>,
    pub packet_hash: String,
    pub pipeline_version: String,
    pub tokenizer_id: String,
    pub token_estimate: usize,
    pub sources: Vec<ContextSourceRef>,
    pub omissions: Vec<ContextOmission>,
    pub message_hashes: Vec<String>,
    #[serde(default)]
    pub prompt_assembly_strategy: PromptAssemblyStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_prefix_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<PromptSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_plan: Option<PromptCachePlan>,
    #[serde(default)]
    pub prompt_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSourceRef {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub token_estimate: usize,
    pub included: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextOmission {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub reason: String,
    pub token_estimate: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

pub type SessionId = String;
pub type MessageNamespace = String;
pub type ContextEpoch = u64;
pub type ToolUseId = String;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct MessageId {
    pub origin_session_id: SessionId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin_message_namespace: MessageNamespace,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
    pub id: MessageId,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedHistory {
    pub items: Vec<ProjectedHistoryItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectedHistoryItem {
    Canonical {
        message_id: MessageId,
        canonical_index: usize,
        message: Message,
    },
    Checkpoint {
        checkpoint_ref: CompactionCheckpointRef,
        source_range: HistoryRange,
        message: Message,
    },
    Tombstone {
        source_message_id: MessageId,
        canonical_index: usize,
        message: Message,
    },
}

impl ProjectedHistoryItem {
    pub fn message(&self) -> &Message {
        match self {
            Self::Canonical { message, .. } => message,
            Self::Checkpoint { message, .. } => message,
            Self::Tombstone { message, .. } => message,
        }
    }

    pub fn to_session_message(&self, session_id: &str) -> SessionMessage {
        match self {
            Self::Canonical {
                message_id,
                message,
                ..
            } => SessionMessage {
                id: message_id.clone(),
                message: message.clone(),
                metadata: None,
            },
            Self::Tombstone {
                source_message_id,
                message,
                ..
            } => SessionMessage {
                id: source_message_id.clone(),
                message: message.clone(),
                metadata: None,
            },
            Self::Checkpoint {
                checkpoint_ref,
                message,
                ..
            } => SessionMessage {
                id: MessageId {
                    origin_session_id: session_id.to_string(),
                    origin_message_namespace: format!(
                        "checkpoint:{}",
                        checkpoint_ref.checkpoint_id
                    ),
                    sequence: checkpoint_ref.source_range.end as u64,
                },
                message: message.clone(),
                metadata: None,
            },
        }
    }
}

impl ProjectedHistory {
    pub fn map_projected_range_to_canonical(&self, range: HistoryRange) -> HistoryRange {
        let first_item = &self.items[range.start];
        let last_item = &self.items[range.end - 1];

        let canonical_start = match first_item {
            ProjectedHistoryItem::Canonical {
                canonical_index, ..
            } => *canonical_index,
            ProjectedHistoryItem::Tombstone {
                canonical_index, ..
            } => *canonical_index,
            ProjectedHistoryItem::Checkpoint { source_range, .. } => source_range.start,
        };

        let canonical_end = match last_item {
            ProjectedHistoryItem::Canonical {
                canonical_index, ..
            } => canonical_index + 1,
            ProjectedHistoryItem::Tombstone {
                canonical_index, ..
            } => canonical_index + 1,
            ProjectedHistoryItem::Checkpoint { source_range, .. } => source_range.end,
        };

        HistoryRange::new(canonical_start, canonical_end)
    }

    pub fn to_session_messages(&self, session_id: &str) -> Vec<SessionMessage> {
        self.items
            .iter()
            .map(|item| item.to_session_message(session_id))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArtifactRef {
    #[serde(default)]
    pub run_id: String,
    #[serde(alias = "id", default)]
    pub relative_path: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompactionCheckpointRef {
    pub checkpoint_id: String,
    pub source_range: HistoryRange,
    pub source_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptSnapshotRef {
    pub snapshot_hash: String,
    pub prefix_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClearedToolResultRef {
    pub tool_use_id: ToolUseId,
    pub message_id: MessageId,
    pub output_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ContextProjectionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_checkpoint: Option<CompactionCheckpointRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cleared_tool_results: BTreeMap<ToolUseId, ClearedToolResultRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snapshot: Option<PromptSnapshotRef>,
    #[serde(default)]
    pub context_epoch: ContextEpoch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StateUpdate<T> {
    #[default]
    Unchanged,
    Set(T),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ContextStateDelta {
    #[serde(default, skip_serializing_if = "is_unchanged")]
    pub active_checkpoint: StateUpdate<CompactionCheckpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared_tool_results: Vec<ClearedToolResultRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared_tool_results_remove: Vec<ToolUseId>,
    #[serde(default, skip_serializing_if = "is_unchanged")]
    pub prompt_snapshot: StateUpdate<PromptSnapshotRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_epoch: Option<ContextEpoch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_fingerprint: Option<String>,
}

fn is_unchanged<T>(update: &StateUpdate<T>) -> bool {
    matches!(update, StateUpdate::Unchanged)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRetention {
    pub clearable: bool,
    pub reconstructible: bool,
    pub retain_errors: bool,
}

impl ToolRetention {
    #[must_use]
    pub const fn conservative_default() -> Self {
        Self {
            clearable: false,
            reconstructible: false,
            retain_errors: true,
        }
    }

    /// Derive a tool's retention policy from its trust annotations.
    ///
    /// `clearable` is the caller's decision that a result may be summarised
    /// away during context pressure (typically low-risk, read-only tools);
    /// `reconstructible` tracks whether the result can be cheaply regenerated
    /// (idempotent tools). Everything else falls back to the conservative
    /// default so that side-effecting results are never silently dropped.
    #[must_use]
    pub fn from_clearable(idempotent: bool, clearable: bool) -> Self {
        if clearable {
            Self {
                clearable: true,
                reconstructible: idempotent,
                retain_errors: true,
            }
        } else {
            Self::conservative_default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolRetentionRegistrySnapshot {
    pub policies: BTreeMap<crate::tool_descriptor::CanonicalToolId, ToolRetention>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProjectionMessageMetadata {
    pub message_id: MessageId,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_index: Option<usize>,
    #[serde(default)]
    pub is_tombstone: bool,
    #[serde(default)]
    pub is_checkpoint: bool,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionManifest {
    pub v: u32,
    pub manifest_id: String,
    pub session_id: SessionId,
    pub run_id: String,
    pub turn_id: usize,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub policy: ContextManagementPolicy,
    pub token_estimate: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_prefix_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_ref: Option<CompactionCheckpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared_results: Vec<ClearedToolResultRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted_messages: Vec<MessageId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages_metadata: Vec<ProjectionMessageMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_report_ref: Option<String>,
}

impl Default for ProjectionManifest {
    fn default() -> Self {
        Self {
            v: 1,
            manifest_id: String::new(),
            session_id: SessionId::default(),
            run_id: String::default(),
            turn_id: 0,
            timestamp: chrono::Utc::now(),
            policy: ContextManagementPolicy::default(),
            token_estimate: 0,
            stable_prefix_hash: None,
            checkpoint_ref: None,
            cleared_results: Vec::new(),
            omitted_messages: Vec::new(),
            messages_metadata: Vec::new(),
            retention_fingerprint: None,
            context_report_ref: None,
        }
    }
}

pub struct ContextPreparationRequest<'a> {
    pub history: &'a [SessionMessage],
    pub context_state: &'a ContextProjectionState,
    pub token_budget: &'a TokenBudget,
    pub provider: &'a dyn crate::provider::Provider,
    pub request_template: &'a crate::provider::ProviderRequest,
    pub model: &'a str,
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub turn_id: usize,
    pub policy: &'a ContextManagementPolicy,
    pub artifacts_dir: Option<&'a std::path::Path>,
    pub tool_retention: &'a ToolRetentionRegistrySnapshot,
    pub emit: &'a mut (dyn FnMut(crate::event::AgentEvent) -> Result<(), crate::error::HarnessError>
                 + Send),
}

#[derive(Debug, Clone)]
pub struct PreparedContext {
    pub packet: ContextPacket,
    pub manifest: ProjectionManifest,
    pub state_delta: ContextStateDelta,
}

#[derive(Debug, Clone)]
pub struct ContextPlan {
    pub history: Vec<SessionMessage>,
    pub omissions: Vec<ContextOmission>,
    pub budget_exhausted: bool,
}

pub trait ContextAssembler: Send + Sync {
    fn version(&self) -> &str;

    fn system_messages(&self) -> Vec<Message>;

    fn assemble(
        &self,
        plan: &ContextPlan,
    ) -> std::result::Result<ContextPacket, crate::error::ContextError>;
}

#[async_trait::async_trait]
pub trait ContextPipeline: Send + Sync {
    fn process(&self, history: &[SessionMessage], _budget: &TokenBudget) -> Vec<Message> {
        history.iter().map(|entry| entry.message.clone()).collect()
    }

    fn version(&self) -> &str;

    fn as_assembler(&self) -> Option<std::sync::Arc<dyn ContextAssembler>> {
        None
    }

    fn build_packet(&self, history: &[SessionMessage], budget: &TokenBudget) -> ContextPacket {
        let messages = self.process(history, budget);
        let version = self.version().to_string();
        let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
        let to_hash = format!("{serialized_messages}:{version}");
        let packet_hash = sha256_hash(&to_hash);

        let message_hashes = messages
            .iter()
            .map(|msg| {
                let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                sha256_hash(&msg_ser)
            })
            .collect();

        ContextPacket {
            messages,
            packet_hash,
            pipeline_version: version,
            tokenizer_id: "default".to_string(),
            token_estimate: 0,
            sources: Vec::new(),
            omissions: Vec::new(),
            message_hashes,
            prompt_assembly_strategy: PromptAssemblyStrategy::Dynamic,
            snapshot_hash: None,
            cache_prefix_hash: None,
            segments: Vec::new(),
            cache_plan: None,
            prompt_source: None,
        }
    }

    async fn prepare_context(
        &self,
        request: ContextPreparationRequest<'_>,
    ) -> Result<PreparedContext, crate::error::HarnessError> {
        Ok(PreparedContext {
            packet: self.build_packet(request.history, request.token_budget),
            manifest: ProjectionManifest {
                v: 1,
                manifest_id: format!("manifest-{}-{}", request.session_id, request.turn_id),
                session_id: request.session_id.to_string(),
                run_id: request.run_id.to_string(),
                turn_id: request.turn_id,
                timestamp: chrono::Utc::now(),
                policy: request.policy.clone(),
                token_estimate: 0,
                stable_prefix_hash: None,
                checkpoint_ref: request.context_state.active_checkpoint.clone(),
                cleared_results: request
                    .context_state
                    .cleared_tool_results
                    .values()
                    .cloned()
                    .collect(),
                omitted_messages: Vec::new(),
                messages_metadata: Vec::new(),
                retention_fingerprint: Some(request.tool_retention.fingerprint.clone()),
                context_report_ref: None,
            },
            state_delta: ContextStateDelta::default(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct TokenBudget {
    pub model_limit: usize,
    pub reserved_output: usize,
    pub used_system: usize,
    pub used_history: usize,
    pub used_sources: usize,
    pub used_tools: usize,
    pub used_memory: usize,
    pub minimum_turn_budget: usize,
}

impl TokenBudget {
    pub fn available_total(&self) -> usize {
        self.model_limit
            .saturating_sub(self.reserved_output)
            .saturating_sub(self.used_system)
            .saturating_sub(self.used_history)
            .saturating_sub(self.used_sources)
            .saturating_sub(self.used_tools)
            .saturating_sub(self.used_memory)
    }

    pub fn exhausted(&self) -> bool {
        self.available_total() < self.minimum_turn_budget
    }

    pub fn record_usage(&mut self, input_tokens: usize, _output_tokens: usize) {
        let non_history = self
            .used_system
            .saturating_add(self.used_sources)
            .saturating_add(self.used_tools)
            .saturating_add(self.used_memory);
        self.used_history = input_tokens.saturating_sub(non_history);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct HistoryRange {
    pub start: usize,
    pub end: usize,
}

impl HistoryRange {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    pub fn contains(&self, idx: usize) -> bool {
        idx >= self.start && idx < self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRef {
    pub checkpoint_id: String,
    pub range: HistoryRange,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClearAction {
    pub message_index: usize,
    pub message_id: MessageId,
    pub tool_use_id: String,
    pub tool_name: String,
    pub original_tokens: usize,
    pub output_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityMode {
    #[default]
    Required,
    BestEffort,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextManagementPolicy {
    pub enabled: bool,
    pub buffer_tokens: usize,
    pub keep_recent_tokens: usize,
    pub keep_recent_turns: usize,
    pub tool_result_budget_ratio: f64,
    pub compaction_target_ratio: f64,
    pub durability: DurabilityMode,
    pub profile: String,
}

impl Default for ContextManagementPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            buffer_tokens: 4096,
            keep_recent_tokens: 8192,
            keep_recent_turns: 5,
            tool_result_budget_ratio: 0.5,
            compaction_target_ratio: 0.8,
            durability: DurabilityMode::Required,
            profile: "default".to_string(),
        }
    }
}

impl ContextManagementPolicy {
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_ratio("tool_result_budget_ratio", self.tool_result_budget_ratio)?;
        validate_ratio("compaction_target_ratio", self.compaction_target_ratio)?;

        if self.profile.trim().is_empty() {
            return Err(ConfigError::InvalidValue {
                field: "context.management.profile".to_string(),
                reason: "must not be empty".to_string(),
            });
        }

        Ok(())
    }
}

fn validate_ratio(field: &str, value: f64) -> Result<(), ConfigError> {
    if !value.is_finite() {
        return Err(ConfigError::InvalidValue {
            field: format!("context.management.{field}"),
            reason: "must be finite".to_string(),
        });
    }

    if !(0.0..=1.0).contains(&value) {
        return Err(ConfigError::InvalidValue {
            field: format!("context.management.{field}"),
            reason: "must be between 0.0 and 1.0".to_string(),
        });
    }

    Ok(())
}
