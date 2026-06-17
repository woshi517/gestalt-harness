use crate::error::Result;
use gestalt_core::context::{
    ContextOmission, ContextPipeline, ContextSourceRef, PromptAssemblyStrategy, PromptCachePlan,
    PromptSegment, PromptSegmentKind, PromptSnapshot, TokenBudget,
};
use gestalt_core::message::Message;
use gestalt_core::ContextStability;
use sha2::Digest as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ContextPatch {
    pub message: Message,
    pub stability: ContextStability,
    pub source: Option<ContextSourceRef>,
    pub omissions: Vec<ContextOmission>,
}

impl ContextPatch {
    pub fn new(message: Message, stability: ContextStability) -> Self {
        Self {
            message,
            stability,
            source: None,
            omissions: Vec::new(),
        }
    }

    pub fn new_with_metadata(
        message: Message,
        stability: ContextStability,
        source: Option<ContextSourceRef>,
        omissions: Vec<ContextOmission>,
    ) -> Self {
        Self {
            message,
            stability,
            source,
            omissions,
        }
    }
}

#[async_trait::async_trait]
pub trait ContextContributor: Send + Sync {
    fn name(&self) -> &str;
    fn stability(&self) -> ContextStability;
    async fn contribute(&self, workspace_root: &Path) -> Result<gestalt_core::message::Message>;

    fn source(&self, _workspace_root: &Path, _content: &str) -> Option<ContextSourceRef> {
        None
    }

    fn omissions(&self, _workspace_root: &Path) -> Vec<ContextOmission> {
        Vec::new()
    }
}

pub struct RuntimeContextPipeline {
    pub base: Arc<dyn ContextPipeline>,
    pub patch_store: Arc<Mutex<Vec<ContextPatch>>>,
    pub current_checkpoint: Arc<Mutex<Option<gestalt_trace::CompactionCheckpoint>>>,
}

#[async_trait::async_trait]
impl ContextPipeline for RuntimeContextPipeline {
    fn process(&self, history: &[Message], budget: &TokenBudget) -> Vec<Message> {
        let messages = self.base.process(history, budget);
        let patches = self.patch_store.lock().unwrap().clone();
        Self::compose_messages(messages, &patches)
    }

    fn version(&self) -> &str {
        self.base.version()
    }

    fn build_packet(
        &self,
        history: &[Message],
        budget: &TokenBudget,
    ) -> gestalt_core::context::ContextPacket {
        let mut packet = self.base.build_packet(history, budget);
        let patches = self.patch_store.lock().unwrap().clone();
        if !patches.is_empty() {
            let (messages, stable_prefix_len) =
                Self::compose_messages_with_prefix(packet.messages.clone(), &patches);
            packet = Self::rebuild_packet(packet, messages, stable_prefix_len);
            for patch in &patches {
                if let Some(src) = &patch.source {
                    packet.sources.push(src.clone());
                }
                packet.omissions.extend(patch.omissions.clone());
            }
        }
        packet
    }

    async fn prepare_context(
        &self,
        history: &[Message],
        budget: &TokenBudget,
        provider: &dyn gestalt_core::provider::Provider,
        request_template: &gestalt_core::provider::ProviderRequest,
        model: &str,
        session_id: &str,
        run_id: &str,
        turn_id: usize,
        policy: &gestalt_core::ContextManagementPolicy,
        artifacts_dir: Option<&std::path::Path>,
        emit: &mut (dyn FnMut(
            gestalt_core::event::AgentEvent,
        ) -> std::result::Result<(), gestalt_core::error::HarnessError>
                  + Send),
    ) -> std::result::Result<gestalt_core::context::ContextPacket, gestalt_core::error::HarnessError>
    {
        let accountant =
            gestalt_context::accounting::ContextAccountant::new(budget, policy, history);
        let usable_limit = accountant.usable_limit();

        let packet = if policy.enabled && budget.model_limit > 0 && usable_limit > 0 {
            self.build_management_packet(history, budget)
        } else {
            self.build_packet(history, budget)
        };

        // Bypass pipeline when disabled or when there is no meaningful budget headroom.
        // A usable_limit of 0 means model_limit is too small to absorb reserved_output +
        // buffer_tokens; running the pipeline would produce spurious exhaustion errors.
        if !policy.enabled || budget.model_limit == 0 || usable_limit == 0 {
            let manifest = self.build_manifest(
                &packet,
                session_id,
                run_id,
                turn_id,
                policy,
                None,
                Vec::new(),
                0,
            );
            if let Some(dir) = artifacts_dir {
                gestalt_trace::persist_manifest(&manifest, dir, policy.durability)?;
            }
            return Ok(packet);
        }

        let packet_request_estimate =
            self.request_token_estimate(provider, request_template, &packet)?;
        emit(gestalt_core::event::AgentEvent::ContextPressure {
            usable_limit,
            current_estimate: packet_request_estimate,
        })?;

        if packet_request_estimate <= usable_limit {
            let manifest = self.build_manifest(
                &packet,
                session_id,
                run_id,
                turn_id,
                policy,
                None,
                Vec::new(),
                0,
            );
            if let Some(dir) = artifacts_dir {
                gestalt_trace::persist_manifest(&manifest, dir, policy.durability)?;
            }
            return Ok(packet);
        }

        // 1. Tool result clearing
        let tool_budget = accountant.tool_result_budget();
        let (cleared_history, clear_actions) =
            gestalt_context::tool_clearing::clear_eligible_tool_results(
                history,
                usable_limit,
                tool_budget,
                policy.keep_recent_turns,
                policy.keep_recent_tokens,
            );

        let cleared_packet = self.build_management_packet(&cleared_history, budget);
        let cleared_tokens = packet
            .token_estimate
            .saturating_sub(cleared_packet.token_estimate);
        if !clear_actions.is_empty() {
            emit(gestalt_core::event::AgentEvent::ContextClearing {
                cleared_count: clear_actions.len(),
                cleared_tokens,
            })?;
        }

        if self.request_token_estimate(provider, request_template, &cleared_packet)? <= usable_limit
        {
            let checkpoint_ref = self.current_checkpoint.lock().unwrap().as_ref().map(|cp| {
                let serialized = serde_json::to_string(cp).unwrap_or_default();
                let mut hasher = sha2::Sha256::new();
                hasher.update(serialized.as_bytes());
                let hash = format!("{:x}", hasher.finalize());
                gestalt_core::context::CheckpointRef {
                    checkpoint_id: cp.checkpoint_id.clone(),
                    range: cp.history_range,
                    hash,
                }
            });
            let history_start_idx = self.get_history_start_idx(&cleared_packet, &cleared_history);
            let manifest = self.build_manifest(
                &cleared_packet,
                session_id,
                run_id,
                turn_id,
                policy,
                checkpoint_ref,
                clear_actions,
                history_start_idx,
            );
            if let Some(dir) = artifacts_dir {
                gestalt_trace::persist_manifest(&manifest, dir, policy.durability)?;
            }
            return Ok(cleared_packet);
        }

        // 2. Compaction
        let last_checkpoint_end_idx = match &*self.current_checkpoint.lock().unwrap() {
            Some(cp) => cp.history_range.end,
            None => 0,
        };
        let recent_protected_start = gestalt_context::tool_clearing::find_recent_protected_start(
            &cleared_history,
            policy.keep_recent_turns,
            policy.keep_recent_tokens,
        );

        let compactor_input_limit = usable_limit;
        let target_limit = accountant.compaction_target();
        let min_tokens_to_compact = cleared_packet.token_estimate.saturating_sub(target_limit);
        let compaction_range = gestalt_context::compaction::plan_compaction_range(
            &cleared_history,
            last_checkpoint_end_idx,
            recent_protected_start,
            compactor_input_limit,
            min_tokens_to_compact,
        );

        if let Some(range) = compaction_range {
            emit(gestalt_core::event::AgentEvent::ContextCompactionStarted { range })?;

            let history_to_compact = &cleared_history[range.start..range.end];
            let canonical_history_to_compact = &history[range.start..range.end];
            let range_serialized =
                serde_json::to_string(canonical_history_to_compact).unwrap_or_default();
            let mut range_hasher = sha2::Sha256::new();
            range_hasher.update(range_serialized.as_bytes());
            let history_range_hash = format!("{:x}", range_hasher.finalize());

            let prev_checkpoint = self.current_checkpoint.lock().unwrap().clone();
            let compactor_res = crate::compaction::run_compactor(
                provider,
                model,
                history_to_compact,
                range,
                history_range_hash.clone(),
                self.base.version().to_string(),
                prev_checkpoint.as_ref(),
            )
            .await;

            match compactor_res {
                Ok(checkpoint) => {
                    if let Err(val_err) =
                        gestalt_context::checkpoint_validation::validate_checkpoint(
                            &checkpoint,
                            &cleared_history,
                            range,
                            &history_range_hash,
                        )
                    {
                        let err_msg = format!("Checkpoint validation failed: {:?}", val_err);
                        emit(gestalt_core::event::AgentEvent::ContextManagementFailed {
                            error: err_msg.clone(),
                        })?;
                        return Err(gestalt_core::error::HarnessError::Context(
                            gestalt_core::error::ContextError::PipelineFailed(err_msg),
                        ));
                    }

                    *self.current_checkpoint.lock().unwrap() = Some(checkpoint.clone());

                    if let Some(dir) = artifacts_dir {
                        gestalt_trace::persist_checkpoint(&checkpoint, dir, policy.durability)?;
                    }

                    emit(gestalt_core::event::AgentEvent::ContextCompacted {
                        checkpoint_id: checkpoint.checkpoint_id.clone(),
                        range,
                    })?;

                    let checkpoint_msg = Message::System {
                        content: checkpoint.render_markdown(),
                    };
                    let mut compacted_history = vec![checkpoint_msg];
                    compacted_history.extend(cleared_history[range.end..].iter().cloned());

                    let compacted_packet = self.build_management_packet(&compacted_history, budget);

                    let checkpoint_ref = Some(gestalt_core::context::CheckpointRef {
                        checkpoint_id: checkpoint.checkpoint_id.clone(),
                        range: checkpoint.history_range,
                        hash: checkpoint.history_range_hash.clone(),
                    });

                    let history_start_idx =
                        self.get_history_start_idx(&compacted_packet, &compacted_history);

                    let manifest = self.build_manifest_with_checkpoint_offset(
                        &compacted_packet,
                        session_id,
                        run_id,
                        turn_id,
                        policy,
                        checkpoint_ref,
                        clear_actions,
                        history_start_idx,
                        range.end,
                    );

                    if let Some(dir) = artifacts_dir {
                        gestalt_trace::persist_manifest(&manifest, dir, policy.durability)?;
                    }

                    if self.request_token_estimate(provider, request_template, &compacted_packet)?
                        > usable_limit
                    {
                        emit(gestalt_core::event::AgentEvent::ContextExhaustion {
                            details: format!(
                                "Token estimate {} exceeds usable limit {} even after compaction.",
                                compacted_packet.token_estimate, usable_limit
                            ),
                        })?;
                        return Err(gestalt_core::error::HarnessError::Context(
                            gestalt_core::error::ContextError::Exhausted(format!(
                                "Token estimate {} exceeds limit {}",
                                compacted_packet.token_estimate, usable_limit
                            )),
                        ));
                    }

                    return Ok(compacted_packet);
                }
                Err(err) => {
                    let err_msg = format!("Compaction model call failed: {}", err);
                    emit(gestalt_core::event::AgentEvent::ContextManagementFailed {
                        error: err_msg.clone(),
                    })?;
                    return Err(gestalt_core::error::HarnessError::Context(
                        gestalt_core::error::ContextError::PipelineFailed(err_msg),
                    ));
                }
            }
        }

        if self.request_token_estimate(provider, request_template, &cleared_packet)? > usable_limit
        {
            emit(gestalt_core::event::AgentEvent::ContextExhaustion {
                details: format!(
                    "Token estimate {} exceeds usable limit {} and no compaction range could be planned.",
                    cleared_packet.token_estimate, usable_limit
                ),
            })?;
            return Err(gestalt_core::error::HarnessError::Context(
                gestalt_core::error::ContextError::Exhausted(format!(
                    "Token estimate {} exceeds limit {}",
                    cleared_packet.token_estimate, usable_limit
                )),
            ));
        }

        let checkpoint_ref = self.current_checkpoint.lock().unwrap().as_ref().map(|cp| {
            let serialized = serde_json::to_string(cp).unwrap_or_default();
            let mut hasher = sha2::Sha256::new();
            hasher.update(serialized.as_bytes());
            let hash = format!("{:x}", hasher.finalize());
            gestalt_core::context::CheckpointRef {
                checkpoint_id: cp.checkpoint_id.clone(),
                range: cp.history_range,
                hash,
            }
        });
        let history_start_idx = self.get_history_start_idx(&cleared_packet, &cleared_history);
        let manifest = self.build_manifest(
            &cleared_packet,
            session_id,
            run_id,
            turn_id,
            policy,
            checkpoint_ref,
            clear_actions,
            history_start_idx,
        );
        if let Some(dir) = artifacts_dir {
            gestalt_trace::persist_manifest(&manifest, dir, policy.durability)?;
        }

        Ok(cleared_packet)
    }
}

impl RuntimeContextPipeline {
    fn build_management_packet(
        &self,
        history: &[Message],
        budget: &TokenBudget,
    ) -> gestalt_core::context::ContextPacket {
        let mut unbounded_budget = budget.clone();
        unbounded_budget.model_limit = usize::MAX;
        unbounded_budget.reserved_output = 0;
        unbounded_budget.used_system = 0;
        unbounded_budget.used_history = 0;
        unbounded_budget.used_sources = 0;
        unbounded_budget.used_tools = 0;
        unbounded_budget.used_memory = 0;
        unbounded_budget.minimum_turn_budget = 0;
        self.build_packet(history, &unbounded_budget)
    }

    fn request_token_estimate(
        &self,
        provider: &dyn gestalt_core::provider::Provider,
        request_template: &gestalt_core::provider::ProviderRequest,
        packet: &gestalt_core::context::ContextPacket,
    ) -> std::result::Result<usize, gestalt_core::error::HarnessError> {
        provider.count_request_tokens(&gestalt_core::provider::ProviderRequest {
            messages: packet.messages.clone(),
            cache_plan: packet.cache_plan.clone(),
            ..request_template.clone()
        })
    }

    fn compose_messages(base_messages: Vec<Message>, patches: &[ContextPatch]) -> Vec<Message> {
        Self::compose_messages_with_prefix(base_messages, patches).0
    }

    fn compose_messages_with_prefix(
        base_messages: Vec<Message>,
        patches: &[ContextPatch],
    ) -> (Vec<Message>, usize) {
        let stable_prefix_len = base_messages
            .iter()
            .take_while(|message| {
                !is_budget_notice(message)
                    && !is_checkpoint_message(message)
                    && matches!(message, Message::System { .. })
            })
            .count();

        let (stable_patches, unstable_patches): (Vec<_>, Vec<_>) = patches
            .iter()
            .cloned()
            .partition(|patch| is_stable(patch.stability));
        let stable_patch_count = stable_patches.len();

        let mut messages =
            Vec::with_capacity(base_messages.len() + stable_patch_count + unstable_patches.len());
        messages.extend(base_messages[..stable_prefix_len].iter().cloned());
        messages.extend(stable_patches.into_iter().map(|patch| patch.message));
        messages.extend(unstable_patches.into_iter().map(|patch| patch.message));
        messages.extend(base_messages[stable_prefix_len..].iter().cloned());

        (messages, stable_prefix_len + stable_patch_count)
    }

    fn rebuild_packet(
        mut packet: gestalt_core::context::ContextPacket,
        messages: Vec<Message>,
        stable_prefix_len: usize,
    ) -> gestalt_core::context::ContextPacket {
        use sha2::Digest;

        let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
        let to_hash = format!("{serialized_messages}:{}", packet.pipeline_version);
        let mut hasher = sha2::Sha256::new();
        hasher.update(to_hash.as_bytes());
        packet.packet_hash = format!("{:x}", hasher.finalize());

        packet.message_hashes = messages
            .iter()
            .map(|msg| {
                let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                let mut hasher = sha2::Sha256::new();
                hasher.update(msg_ser.as_bytes());
                format!("{:x}", hasher.finalize())
            })
            .collect();

        packet.messages = messages;

        if packet.cache_plan.is_some() {
            let stable_messages = packet.messages[..stable_prefix_len].to_vec();
            let snapshot = PromptSnapshot::new(stable_messages.clone(), 0);
            let mut segments = vec![PromptSegment::from_messages(
                PromptSegmentKind::Snapshot,
                ContextStability::SessionStatic,
                &stable_messages,
            )];

            let tail = &packet.messages[stable_prefix_len..];
            let (conversation_messages, ephemeral_messages) = split_tail_messages(tail);

            if !conversation_messages.is_empty() {
                segments.push(PromptSegment::from_messages(
                    PromptSegmentKind::Conversation,
                    ContextStability::TurnDynamic,
                    conversation_messages,
                ));
            }

            if !ephemeral_messages.is_empty() {
                segments.push(PromptSegment::from_messages(
                    PromptSegmentKind::Ephemeral,
                    ContextStability::Ephemeral,
                    ephemeral_messages,
                ));
            }

            let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot)
                .with_segments(segments.clone());
            packet.snapshot_hash = Some(snapshot.snapshot_hash);
            packet.cache_prefix_hash = Some(snapshot.prefix_hash);
            packet.segments = segments;
            packet.cache_plan = Some(plan);
        }

        packet
    }

    fn get_history_start_idx(
        &self,
        packet: &gestalt_core::context::ContextPacket,
        history: &[Message],
    ) -> usize {
        packet.messages.len().saturating_sub(history.len())
    }

    fn build_manifest(
        &self,
        packet: &gestalt_core::context::ContextPacket,
        session_id: &str,
        run_id: &str,
        turn_id: usize,
        policy: &gestalt_core::ContextManagementPolicy,
        checkpoint_ref: Option<gestalt_core::context::CheckpointRef>,
        cleared_results: Vec<gestalt_core::ClearAction>,
        history_start_idx: usize,
    ) -> gestalt_trace::ProjectionManifest {
        self.build_manifest_with_checkpoint_offset(
            packet,
            session_id,
            run_id,
            turn_id,
            policy,
            checkpoint_ref,
            cleared_results,
            history_start_idx,
            0,
        )
    }

    fn build_manifest_with_checkpoint_offset(
        &self,
        packet: &gestalt_core::context::ContextPacket,
        session_id: &str,
        run_id: &str,
        turn_id: usize,
        policy: &gestalt_core::ContextManagementPolicy,
        checkpoint_ref: Option<gestalt_core::context::CheckpointRef>,
        cleared_results: Vec<gestalt_core::ClearAction>,
        history_start_idx: usize,
        compact_end_idx: usize,
    ) -> gestalt_trace::ProjectionManifest {
        let checkpoint_used = checkpoint_ref.is_some();
        let messages_metadata = packet
            .messages
            .iter()
            .enumerate()
            .map(|(idx, msg)| {
                let is_tombstone = match msg {
                    Message::ToolResult { content, .. } => content.starts_with("<tombstone"),
                    _ => false,
                };
                let is_checkpoint = match msg {
                    Message::System { content } => {
                        content.starts_with("### Session Checkpoint Summary")
                    }
                    _ => false,
                };

                let original_index = if idx >= history_start_idx {
                    let history_idx = idx - history_start_idx;
                    if checkpoint_used {
                        if history_idx == 0 {
                            None
                        } else {
                            Some(compact_end_idx + (history_idx - 1))
                        }
                    } else {
                        Some(history_idx)
                    }
                } else {
                    None
                };

                let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                let mut hasher = sha2::Sha256::new();
                hasher.update(msg_ser.as_bytes());
                let hash = format!("{:x}", hasher.finalize());

                gestalt_trace::MessageMetadataRef {
                    role: match msg {
                        Message::System { .. } => "system".to_string(),
                        Message::User { .. } => "user".to_string(),
                        Message::Assistant { .. } => "assistant".to_string(),
                        Message::ToolResult { .. } => "toolresult".to_string(),
                    },
                    original_index,
                    is_tombstone,
                    is_checkpoint,
                    hash,
                }
            })
            .collect();

        let timestamp = chrono::Utc::now();
        let policy_cloned = policy.clone();

        let manifest_partial = gestalt_trace::ProjectionManifest {
            manifest_id: String::new(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            turn_id,
            timestamp,
            policy: policy_cloned,
            token_estimate: packet.token_estimate,
            stable_prefix_hash: packet.cache_prefix_hash.clone(),
            checkpoint_ref,
            cleared_results,
            messages_metadata,
        };

        let manifest_serialized = serde_json::to_string(&manifest_partial).unwrap_or_default();
        let mut hasher = sha2::Sha256::new();
        hasher.update(manifest_serialized.as_bytes());
        let manifest_id = format!("{:x}", hasher.finalize());

        gestalt_trace::ProjectionManifest {
            manifest_id,
            ..manifest_partial
        }
    }
}

fn is_stable(stability: ContextStability) -> bool {
    matches!(
        stability,
        ContextStability::SessionStatic | ContextStability::ActivationStatic
    )
}

fn is_budget_notice(message: &Message) -> bool {
    matches!(message, Message::System { content } if content.starts_with("context budget exhausted or truncated;"))
}

fn is_checkpoint_message(message: &Message) -> bool {
    matches!(message, Message::System { content } if content.starts_with("### Session Checkpoint Summary"))
}

fn split_tail_messages(messages: &[Message]) -> (&[Message], &[Message]) {
    if matches!(messages.last(), Some(Message::System { content }) if content.starts_with("context budget exhausted or truncated;"))
    {
        messages.split_at(messages.len().saturating_sub(1))
    } else {
        (messages, &[])
    }
}
