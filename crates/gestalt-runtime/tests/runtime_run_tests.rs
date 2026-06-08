use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{ContextPipeline, PromptAssemblyStrategy, PromptCachePlan, PromptSnapshot, TokenBudget},
    event::{AgentEvent, StopReason},
    hook::ContextHook,
    message::{ContentBlock, Message},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{Session, SessionConfig},
    tool::{ToolCatalog, ToolContext, ToolSchema},
};
use gestalt_runtime::{
    AfterContextBuildCtx, AgentRuntimeBuilder, CompositionHooks, ContextPatch, HookOutcome,
    RuntimeConfig, RuntimeContextHookAdapter, RuntimeContextPipeline, RuntimeEventBus, UserInput,
};

struct NoopCompositionHooks;

#[async_trait::async_trait]
impl CompositionHooks for NoopCompositionHooks {
    async fn before_context_build(
        &self,
        _context: &gestalt_runtime::BeforeContextBuildCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_context_build(
        &self,
        _context: &AfterContextBuildCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn before_tool_policy(
        &self,
        _context: &gestalt_runtime::BeforeToolPolicyCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_tool_result(
        &self,
        _context: &gestalt_runtime::AfterToolResultCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn on_event(
        &self,
        _context: &gestalt_runtime::OnEventCtx,
    ) -> gestalt_runtime::Result<()> {
        Ok(())
    }
}

struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn default_model(&self) -> &str {
        "mock-model"
    }
    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: false,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: true,
            supports_strict_schema: false,
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }
    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }
    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        let events = vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }];
        let stream = futures::stream::iter(
            events
                .into_iter()
                .map(Ok::<_, gestalt_core::error::HarnessError>),
        );
        Ok(Box::pin(stream))
    }
}

struct MockToolCatalog;
impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

struct MockContextPipeline;
impl ContextPipeline for MockContextPipeline {
    fn process(&self, _history: &[Message], _budget: &TokenBudget) -> Vec<Message> {
        Vec::new()
    }
    fn version(&self) -> &str {
        "mock-v1"
    }
}

struct MockPolicyEngine;
#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

fn build_test_runtime() -> gestalt_runtime::runtime::AgentRuntime {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_runtime_run_prompt_happy_path() {
    let runtime = build_test_runtime();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let input = UserInput {
        prompt: "hello".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: Some(tx),
        artifact_dir: None,
    };

    let res = runtime.run_prompt(input).await;
    assert!(res.is_ok());
    let run_res = res.unwrap();
    assert_eq!(run_res.stop_reason, StopReason::EndTurn);

    // Drain events to verify expected sequences
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(!events.is_empty());
    // Verify it captures workspace snapshot first
    assert!(matches!(
        events[0],
        AgentEvent::WorkspaceSnapshotCaptured { .. }
    ));
    // Verify it outputs the user message
    assert!(matches!(events[1], AgentEvent::UserMessage { .. }));
}

#[tokio::test]
async fn test_runtime_run_session_preserves_history() {
    let runtime = build_test_runtime();
    let cancel = gestalt_core::cancel::CancelToken::new();

    let snapshot = gestalt_core::snapshot::WorkspaceSnapshot {
        workspace_root: std::env::current_dir().unwrap(),
        git_sha: None,
        git_dirty: None,
        untracked_count: None,
        content_hash: "mock-hash".to_string(),
        captured_at: chrono::Utc::now(),
    };

    let mut session = Session::new(
        "test-session",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 5,
        },
        TokenBudget {
            model_limit: 100,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
        ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            workspace_root: Some(std::env::current_dir().unwrap()),
            timeout: Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 100,
            artifact_dir: None,
            current_tool_call_id: None,
        },
        gestalt_core::session::ExecutionMode::Yolo,
        snapshot,
    );

    // Prepopulate session history
    session.history.push(Message::User {
        content: vec![gestalt_core::message::ContentBlock::Text {
            text: "Initial user turn".to_string(),
        }],
    });
    session.history.push(Message::Assistant {
        content: vec![gestalt_core::message::ContentBlock::Text {
            text: "Initial assistant turn".to_string(),
        }],
    });

    let res = runtime.run_session(&mut session, &cancel, None, None).await;
    assert!(res.is_ok());

    // Verify history was preserved and not reset
    assert_eq!(session.history.len(), 3);
    if let Message::User { content } = &session.history[0] {
        if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
            assert_eq!(text, "Initial user turn");
        }
    }
    if let Message::Assistant { content } = &session.history[1] {
        if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
            assert_eq!(text, "Initial assistant turn");
        }
    }
}

#[tokio::test]
async fn test_runtime_context_hook_persists_prompt_snapshot() {
    let temp_dir = std::env::temp_dir().join(format!(
        "gestalt-runtime-snapshot-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let artifact_dir = temp_dir.join("artifacts");
    let adapter = RuntimeContextHookAdapter {
        hooks: Arc::new(NoopCompositionHooks),
        patch_store: Arc::new(std::sync::Mutex::new(Vec::new())),
        contributors: vec![],
        workspace_root: temp_dir.clone(),
        block_reason: None,
        event_bus: RuntimeEventBus::new(),
        prompt_snapshot_state: Arc::new(std::sync::Mutex::new(None)),
    };

    let session = Session::new(
        "test-session",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 5,
        },
        TokenBudget {
            model_limit: 100,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
        ToolContext {
            working_dir: temp_dir.clone(),
            workspace_root: Some(temp_dir.clone()),
            timeout: Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 100,
            artifact_dir: Some(artifact_dir.clone()),
            current_tool_call_id: None,
        },
        gestalt_core::session::ExecutionMode::Yolo,
        gestalt_core::snapshot::WorkspaceSnapshot {
            workspace_root: temp_dir.clone(),
            git_sha: None,
            git_dirty: None,
            untracked_count: None,
            content_hash: "mock-hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );

    let stable_message = Message::System {
        content: "stable prefix".to_string(),
    };
    let dynamic_message = Message::User {
        content: vec![ContentBlock::Text {
            text: "latest turn".to_string(),
        }],
    };
    let snapshot = PromptSnapshot::new(vec![stable_message.clone()], 0);
    let cache_plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot);
    let packet = gestalt_core::context::ContextPacket {
        messages: vec![stable_message, dynamic_message],
        packet_hash: "packet-hash".to_string(),
        pipeline_version: "mock-v1".to_string(),
        tokenizer_id: "default".to_string(),
        token_estimate: 12,
        sources: vec![],
        omissions: vec![],
        message_hashes: vec![],
        prompt_assembly_strategy: PromptAssemblyStrategy::Snapshot,
        snapshot_hash: Some(snapshot.snapshot_hash.clone()),
        cache_prefix_hash: Some(snapshot.prefix_hash.clone()),
        segments: vec![],
        cache_plan: Some(cache_plan),
        prompt_source: Some("default".to_string()),
    };

    let events_first = adapter
        .after_context_build(&session, &packet)
        .await
        .expect("after_context_build succeeds");
    assert!(matches!(events_first[0], AgentEvent::PromptSnapshotCreated { .. }));
    assert!(matches!(events_first[1], AgentEvent::PromptCachePlanGenerated { .. }));

    let persisted = gestalt_trace::read_prompt_snapshot(artifact_dir.join("prompt-snapshot.json"))
        .expect("prompt snapshot persisted");
    assert_eq!(persisted.snapshot_hash, snapshot.snapshot_hash);

    let events_second = adapter
        .after_context_build(&session, &packet)
        .await
        .expect("after_context_build succeeds again");
    assert!(matches!(events_second[0], AgentEvent::PromptSnapshotReused { .. }));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_runtime_context_pipeline_keeps_cache_metadata_for_stable_patches() {
    struct SnapshotPipeline;

    impl ContextPipeline for SnapshotPipeline {
        fn process(&self, _history: &[Message], _budget: &TokenBudget) -> Vec<Message> {
            vec![Message::System {
                content: "stable system prefix".to_string(),
            }]
        }

        fn version(&self) -> &str {
            "pipeline-v1"
        }

        fn build_packet(&self, _history: &[Message], _budget: &TokenBudget) -> gestalt_core::context::ContextPacket {
            let messages = self.process(&[], &TokenBudget::default());
            let snapshot = PromptSnapshot::new(messages.clone(), 0);
            let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot);

            gestalt_core::context::ContextPacket {
                messages,
                packet_hash: "base-packet-hash".to_string(),
                pipeline_version: self.version().to_string(),
                tokenizer_id: "default".to_string(),
                token_estimate: 0,
                sources: vec![],
                omissions: vec![],
                message_hashes: vec![],
                prompt_assembly_strategy: PromptAssemblyStrategy::Snapshot,
                snapshot_hash: Some(snapshot.snapshot_hash),
                cache_prefix_hash: Some(snapshot.prefix_hash),
                segments: vec![],
                cache_plan: Some(plan),
                prompt_source: Some("default".to_string()),
            }
        }
    }

    let pipeline = RuntimeContextPipeline {
        base: Arc::new(SnapshotPipeline),
        patch_store: Arc::new(std::sync::Mutex::new(vec![ContextPatch::new(
            Message::System {
                content: "extension snapshot context".to_string(),
            },
            gestalt_core::ContextStability::SessionStatic,
        )])),
    };

    let packet = pipeline.build_packet(
        &[],
        &TokenBudget {
            model_limit: 256,
            reserved_output: 16,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
    );

    assert_eq!(packet.prompt_assembly_strategy, PromptAssemblyStrategy::Snapshot);
    assert!(packet.snapshot_hash.is_some());
    assert!(packet.cache_prefix_hash.is_some());
    assert!(packet.cache_plan.is_some());
    assert!(packet
        .segments
        .iter()
        .any(|segment| segment.kind == gestalt_core::context::PromptSegmentKind::Snapshot));
}
