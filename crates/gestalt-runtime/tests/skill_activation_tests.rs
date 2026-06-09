//! Tests for the runtime integration of the skill activation engine.
//!
//! These tests exercise the wiring between the deterministic
//! `ActivationEngine` and the runtime: skill state, contributor registration,
//! per-turn resolution, and lifecycle event publication.

use std::collections::HashMap;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use gestalt_core::message::ContentBlock;
use gestalt_core::session::Session;
use gestalt_core::policy::{PolicyDecision, PolicyEngine, PolicyRequest};
use gestalt_core::tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema};
use gestalt_core::tool_descriptor::ToolNamespace;
use gestalt_runtime::composition_hooks::{
    CompositionHooks, HookOutcome, RuntimeContextHookAdapter,
};
use gestalt_runtime::event_bus::{RuntimeEvent, RuntimeEventBus};
use gestalt_runtime::skill_contributor::SkillContributorState;
use gestalt_runtime::{ComposedToolCatalog, RuntimePolicyEngine, ToolCatalogPlanner, ToolProfile};
use gestalt_skills::{
    SkillDescriptor, SkillIndex, SkillSource, SkillTrustLevel,
};

fn make_descriptor(name: &str, description: &str, _body: &str) -> SkillDescriptor {
    SkillDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        skill_root: PathBuf::from("/tmp"),
        manifest_path: PathBuf::from("/tmp/SKILL.md"),
        manifest_hash: format!("hash-for-{name}"),
        trust_level: SkillTrustLevel::Workspace,
        source: SkillSource::WorkspaceLocal,
        license: None,
        compatibility: None,
        metadata: HashMap::new(),
        allowed_tools: None,
    }
}

fn make_index(descs: Vec<SkillDescriptor>) -> SkillIndex {
    SkillIndex::new(descs)
}

#[derive(Clone)]
struct MockTool {
    name: String,
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "test tool"
    }

    fn schema(&self) -> ToolSchema {
        serde_json::json!({
            "name": self.name,
            "input_schema": {"type": "object", "properties": {}}
        })
    }

    fn risk(&self, _input: &serde_json::Value) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::error::ToolError> {
        Ok(ToolOutput::Text {
            content: String::new(),
        })
    }
}

#[derive(Clone)]
struct MockToolCatalog {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|tool| tool.schema()).collect()
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

#[derive(Clone)]
struct AllowAllPolicy;

#[async_trait]
impl PolicyEngine for AllowAllPolicy {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

#[derive(Default)]
struct NoopCompositionHooks;

#[async_trait::async_trait]
impl CompositionHooks for NoopCompositionHooks {
    async fn before_context_build(
        &self,
        _ctx: &gestalt_runtime::composition_hooks::BeforeContextBuildCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn after_context_build(
        &self,
        _ctx: &gestalt_runtime::composition_hooks::AfterContextBuildCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn before_tool_policy(
        &self,
        _ctx: &gestalt_runtime::composition_hooks::BeforeToolPolicyCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn after_tool_result(
        &self,
        _ctx: &gestalt_runtime::composition_hooks::AfterToolResultCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn prepare_next_turn(
        &self,
        _ctx: &gestalt_runtime::composition_hooks::PrepareNextTurnCtx,
    ) -> gestalt_runtime::error::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }
    async fn on_event(
        &self,
        _ctx: &gestalt_runtime::composition_hooks::OnEventCtx,
    ) -> gestalt_runtime::error::Result<()> {
        Ok(())
    }
}

fn fresh_session() -> Session {
    use gestalt_core::context::TokenBudget;
    use gestalt_core::session::SessionConfig;
    use gestalt_core::tool::ToolContext;
    use gestalt_core::snapshot::WorkspaceSnapshot;
    use chrono::Utc;
    Session::new(
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
            working_dir: std::env::temp_dir(),
            workspace_root: Some(std::env::temp_dir()),
            timeout: std::time::Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 100,
            artifact_dir: None,
            current_tool_call_id: None,
        },
        gestalt_core::session::ExecutionMode::Confirm,
        WorkspaceSnapshot {
            workspace_root: std::env::temp_dir(),
            git_sha: None,
            git_dirty: None,
            untracked_count: None,
            content_hash: "test-hash".to_string(),
            captured_at: Utc::now(),
        },
    )
}

#[tokio::test]
async fn test_resolve_active_emits_skill_activated_event() {
    let descs = vec![make_descriptor("pdf", "Process PDF documents and forms.", "PDF body")];
    let bus = RuntimeEventBus::new();
    let state = Arc::new(std::sync::Mutex::new(
        SkillContributorState::new(descs.clone(), vec![]).with_event_bus(bus.clone()),
    ));

    // The trigger should auto-activate the "pdf" skill because its description
    // contains the word "PDF" which appears in the task hint.
    let mut guard = state.lock().unwrap();
    let (_resolved, diff) = guard.resolve_active(Some("Please extract text from a PDF"));
    assert_eq!(_resolved, vec!["pdf".to_string()]);
    assert_eq!(diff.newly_active.len(), 1);
    assert_eq!(diff.newly_active[0].0, "pdf");
    guard.publish_diff(&diff);
    drop(guard);

    let history = bus.history();
    let activations: Vec<&RuntimeEvent> = history
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::SkillActivated { .. }))
        .collect();
    assert_eq!(activations.len(), 1, "expected exactly one SkillActivated event");
}

#[tokio::test]
async fn test_resolve_active_emits_skill_deactivated_event() {
    let descs = vec![
        make_descriptor("pdf", "Process PDF documents and forms.", "PDF body"),
    ];
    let bus = RuntimeEventBus::new();
    // Start with "pdf" already active (as if user explicitly activated it on
    // a prior turn). The skill's description matches the task, so the engine
    // would re-activate it. We then construct a second resolve call with a
    // task that does NOT match the trigger, simulating a turn that no
    // longer needs the skill. Because the user never marked it explicit, the
    // resolved set drops it and a deactivation event is emitted.
    let state = Arc::new(std::sync::Mutex::new(
        SkillContributorState::new(descs, vec!["pdf".to_string()])
            .with_event_bus(bus.clone()),
    ));

    let mut guard = state.lock().unwrap();
    // Turn 1: task matches "pdf" trigger; stays active.
    let (_r1, d1) = guard.resolve_active(Some("Process a PDF for me"));
    guard.publish_diff(&d1);
    // Turn 2: task does not match "pdf" trigger; deactivate by clearing the
    // set first, then resolve.
    guard.active.remove("pdf");
    let (_r2, d2) = guard.resolve_active(Some("Tell me a joke about goats"));
    guard.publish_diff(&d2);
    drop(guard);

    let history = bus.history();
    let deactivations: Vec<&RuntimeEvent> = history
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::SkillDeactivated { .. }))
        .collect();
    assert!(!deactivations.is_empty(), "expected at least one deactivation event");
    let names: Vec<String> = deactivations
        .iter()
        .map(|e| match e {
            RuntimeEvent::SkillDeactivated { skill_name, .. } => skill_name.clone(),
            _ => unreachable!(),
        })
        .collect();
    assert!(names.contains(&"pdf".to_string()));
}

#[tokio::test]
async fn test_build_active_instructions_emits_skill_rejected_on_load_failure() {
    // Build a descriptor whose manifest_path does not exist; load should fail.
    let desc = SkillDescriptor {
        name: "broken".to_string(),
        description: "Broken skill".to_string(),
        skill_root: PathBuf::from("/this/path/does/not/exist"),
        manifest_path: PathBuf::from("/this/path/does/not/exist/SKILL.md"),
        manifest_hash: "broken-hash".to_string(),
        trust_level: SkillTrustLevel::Workspace,
        source: SkillSource::WorkspaceLocal,
        license: None,
        compatibility: None,
        metadata: HashMap::new(),
        allowed_tools: None,
    };
    let bus = RuntimeEventBus::new();
    let state = Arc::new(std::sync::Mutex::new(
        SkillContributorState::new(vec![desc], vec!["broken".to_string()])
            .with_event_bus(bus.clone()),
    ));

    let mut guard = state.lock().unwrap();
    let _instructions = guard.build_active_instructions();
    drop(guard);

    let history = bus.history();
    let rejections: Vec<&RuntimeEvent> = history
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::SkillRejected { .. }))
        .collect();
    assert_eq!(
        rejections.len(),
        1,
        "expected exactly one SkillRejected event for the failed load"
    );
}

#[tokio::test]
async fn test_resource_recorder_emits_skill_resource_accessed() {
    let bus = RuntimeEventBus::new();
    let state = Arc::new(std::sync::Mutex::new(
        SkillContributorState::new(vec![], vec![]).with_event_bus(bus.clone()),
    ));
    let recorder = {
        let guard = state.lock().unwrap();
        guard.resource_recorder()
    };
    let recorder = recorder.expect("recorder should be available when bus is set");
    recorder("pdf", "references/PDF_TOOLS.md");

    let history = bus.history();
    let accesses: Vec<&RuntimeEvent> = history
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::SkillResourceAccessed { .. }))
        .collect();
    assert_eq!(accesses.len(), 1);
}

#[tokio::test]
async fn test_context_hook_adapter_resolves_activation_on_before_context_build() {
    use gestalt_core::hook::ContextHook;
    let descs = vec![make_descriptor("pdf", "Process PDF documents.", "PDF body")];
    let bus = RuntimeEventBus::new();
    let state = Arc::new(std::sync::Mutex::new(
        SkillContributorState::new(descs, vec![]).with_event_bus(bus.clone()),
    ));
    let mut session = fresh_session();
    session.history.push(gestalt_core::message::Message::User {
        content: vec![ContentBlock::Text {
            text: "Please extract text from this PDF document".to_string(),
        }],
    });

    let adapter = RuntimeContextHookAdapter {
        hooks: Arc::new(NoopCompositionHooks),
        patch_store: Arc::new(std::sync::Mutex::new(Vec::new())),
        contributors: vec![],
        workspace_root: std::env::temp_dir(),
        block_reason: None,
        event_bus: bus.clone(),
        prompt_snapshot_state: Arc::new(std::sync::Mutex::new(None)),
        skill_state: Some(state.clone()),
    };

    let _ = adapter.before_context_build(&session).await;

    // After the hook runs, the active set should contain "pdf".
    let guard = state.lock().unwrap();
    assert!(guard.active.contains("pdf"));
    drop(guard);

    // The hook should have published exactly one SkillActivated event for the
    // first turn.
    let history = bus.history();
    let activations: Vec<&RuntimeEvent> = history
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::SkillActivated { .. }))
        .collect();
    assert_eq!(activations.len(), 1);
}

#[tokio::test]
async fn test_index_helpers() {
    let index = make_index(vec![make_descriptor("alpha", "Alpha skill", "Alpha body")]);
    assert!(index.contains("alpha"));
    assert!(!index.contains("missing"));
    let text = index.to_context_index();
    assert!(text.contains("alpha: Alpha skill"));
}

#[tokio::test]
async fn test_dynamic_activation_filters_tools_and_denies_off_skill_calls() {
    let pdf_skill = SkillDescriptor {
        allowed_tools: Some("Read Search".to_string()),
        ..make_descriptor("pdf-processing", "Process PDF documents and forms.", "PDF body")
    };
    let state = Arc::new(std::sync::Mutex::new(
        SkillContributorState::new(vec![pdf_skill], vec![]),
    ));
    let planner = ToolCatalogPlanner::new(ToolProfile::All).with_skill_state(state.clone());
    let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    for name in ["Read", "Search", "Bash"] {
        tools.insert(name.to_string(), Arc::new(MockTool { name: name.to_string() }));
    }
    let catalog = ComposedToolCatalog::new(Arc::new(MockToolCatalog { tools }), BTreeMap::new())
        .unwrap()
        .with_planner(planner);

    let initial_tools: Vec<String> = catalog
        .descriptors()
        .into_iter()
        .map(|desc| desc.id.name)
        .collect();
    assert_eq!(initial_tools, vec!["Bash", "Read", "Search"]);

    {
        let mut guard = state.lock().unwrap();
        let (_resolved, _diff) = guard.resolve_active(Some("Please process this PDF"));
    }

    let filtered_tools: Vec<String> = catalog
        .descriptors()
        .into_iter()
        .map(|desc| desc.id.name)
        .collect();
    assert_eq!(filtered_tools, vec!["Read", "Search"]);

    let policy = RuntimePolicyEngine {
        base: Arc::new(AllowAllPolicy),
        hooks: Arc::new(NoopCompositionHooks),
        session_id: "session-1".to_string(),
        event_bus: RuntimeEventBus::new(),
        skill_state: Some(state.clone()),
    };

    let allowed = policy
        .evaluate(PolicyRequest {
            tool_call_id: "tool-1".to_string(),
            tool_name: "Read".to_string(),
            namespace: ToolNamespace::BuiltIn,
            annotations: gestalt_core::tool_descriptor::ToolAnnotations::default(),
            input: serde_json::json!({}),
            risk: RiskLevel::Low,
            mode: gestalt_core::session::ExecutionMode::Confirm,
            working_dir: std::env::temp_dir(),
            workspace_root: None,
            user_approved: false,
        })
        .await;
    assert!(allowed.is_allowed());

    let denied = policy
        .evaluate(PolicyRequest {
            tool_call_id: "tool-2".to_string(),
            tool_name: "Bash".to_string(),
            namespace: gestalt_core::tool_descriptor::ToolNamespace::BuiltIn,
            annotations: gestalt_core::tool_descriptor::ToolAnnotations::default(),
            input: serde_json::json!({}),
            risk: RiskLevel::Low,
            mode: gestalt_core::session::ExecutionMode::Confirm,
            working_dir: std::env::temp_dir(),
            workspace_root: None,
            user_approved: false,
        })
        .await;
    assert!(!denied.is_allowed());
}
