#![allow(deprecated)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gestalt_core::{
    approval::AutoApprovalProvider,
    event::{AgentEvent, StopReason},
    message::{ContentBlock, Message},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{ExecutionMode, Session, SessionConfig},
    tool::{Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema},
};
use gestalt_runtime::{
    extension::RuntimeGeneration,
    lifecycle::{
        CapabilityDataScope, CapabilityDescriptorV2, CapabilityFailureMode, ContextProviderPlan,
        ContextProviderRegistration, EventObserverPlan, EventObserverRegistration,
        ExternalVerifierPlan, ExternalVerifierRegistration, InitializeRequestV2,
        InitializeResponseV2, LifecycleCapabilityKind, LifecycleClient, LifecycleInvokeRequestV2,
        LifecycleInvokeResponseV2, PolicyGuardPlan, PolicyGuardRegistration, TurnRouteDecision,
        TurnRouterPlan, TurnRouterRegistration, TypedCapabilityDescriptor,
    },
    AgentRuntimeBuilder, RuntimeConfig,
};
use serde_json::json;
use std::time::Duration;

struct ToolUseThenEndProvider {
    turn: Mutex<usize>,
}

#[async_trait::async_trait]
impl Provider for ToolUseThenEndProvider {
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
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: true,
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
        request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        let mut turn = self.turn.lock().unwrap();
        let current_turn = *turn;
        *turn += 1;

        let events = if current_turn == 0 {
            let _ = request;
            vec![
                AgentEvent::ToolCallStreamed {
                    id: "call-1".to_string(),
                    name: "test-tool".to_string(),
                    input_delta: "{}".to_string(),
                },
                AgentEvent::Stop {
                    reason: StopReason::ToolUse,
                },
            ]
        } else {
            vec![AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }]
        };

        let stream = futures::stream::iter(
            events
                .into_iter()
                .map(Ok::<_, gestalt_core::error::HarnessError>),
        );
        Ok(Box::pin(stream))
    }
}

struct TestTool;

#[async_trait::async_trait]
impl Tool for TestTool {
    fn name(&self) -> &str {
        "test-tool"
    }

    fn description(&self) -> &str {
        "test tool"
    }

    fn schema(&self) -> ToolSchema {
        serde_json::from_value(serde_json::json!({
            "name": "test-tool",
            "description": "test tool",
            "input_schema": { "type": "object", "properties": {} }
        }))
        .unwrap()
    }

    fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
        gestalt_core::tool::RiskLevel::Low
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::error::ToolError> {
        Ok(ToolOutput::Text {
            content: "ok".to_string(),
        })
    }
}

struct TestToolCatalog {
    tool: Arc<dyn Tool>,
}

impl ToolCatalog for TestToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        vec![self.tool.schema()]
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if name == self.tool.name() {
            Some(self.tool.clone())
        } else {
            None
        }
    }
}

struct RecordingLifecycleClient {
    calls: Arc<Mutex<Vec<LifecycleCapabilityKind>>>,
}

#[async_trait::async_trait]
impl LifecycleClient for RecordingLifecycleClient {
    async fn initialize(
        &self,
        _request: InitializeRequestV2,
    ) -> gestalt_runtime::Result<InitializeResponseV2> {
        Ok(InitializeResponseV2 {
            negotiated_version: "2.0".to_string(),
            supports_cancellation: true,
        })
    }

    async fn describe_capabilities(&self) -> gestalt_runtime::Result<Vec<CapabilityDescriptorV2>> {
        Ok(vec![
            CapabilityDescriptorV2 {
                component_id: "component:com.example.lifecycle:primary:lifecycle".to_string(),
                capability: LifecycleCapabilityKind::ContextProvider,
                priority: 10,
                timeout_ms: 250,
                failure_mode: "fail_open".to_string(),
                data_scope: "current_turn".to_string(),
            },
            CapabilityDescriptorV2 {
                component_id: "component:com.example.lifecycle:primary:lifecycle".to_string(),
                capability: LifecycleCapabilityKind::PolicyGuard,
                priority: 9,
                timeout_ms: 250,
                failure_mode: "fail_closed".to_string(),
                data_scope: "tool_request".to_string(),
            },
            CapabilityDescriptorV2 {
                component_id: "component:com.example.lifecycle:primary:lifecycle".to_string(),
                capability: LifecycleCapabilityKind::Verifier,
                priority: 8,
                timeout_ms: 250,
                failure_mode: "fail_closed".to_string(),
                data_scope: "current_turn".to_string(),
            },
            CapabilityDescriptorV2 {
                component_id: "component:com.example.lifecycle:primary:lifecycle".to_string(),
                capability: LifecycleCapabilityKind::TurnRouter,
                priority: 7,
                timeout_ms: 250,
                failure_mode: "fail_closed".to_string(),
                data_scope: "current_turn".to_string(),
            },
            CapabilityDescriptorV2 {
                component_id: "component:com.example.lifecycle:primary:lifecycle".to_string(),
                capability: LifecycleCapabilityKind::EventObserver,
                priority: 6,
                timeout_ms: 250,
                failure_mode: "ignore".to_string(),
                data_scope: "runtime_event".to_string(),
            },
        ])
    }

    async fn invoke(
        &self,
        request: LifecycleInvokeRequestV2,
    ) -> gestalt_runtime::Result<LifecycleInvokeResponseV2> {
        self.calls.lock().unwrap().push(request.capability.clone());

        let payload = match request.capability {
            LifecycleCapabilityKind::ContextProvider => {
                serde_json::to_value(gestalt_runtime::lifecycle::ContextProviderResponse {
                    messages: vec![Message::System {
                        content: "lifecycle context".to_string(),
                    }],
                })
                .unwrap()
            }
            LifecycleCapabilityKind::PolicyGuard => serde_json::to_value(PolicyDecision::allowed(
                Some("lifecycle allowed".to_string()),
            ))
            .unwrap(),
            LifecycleCapabilityKind::Verifier => {
                serde_json::to_value(gestalt_runtime::lifecycle::ExternalVerifierReport {
                    component_id: request.component_id,
                    passed: true,
                    message: Some("verified".to_string()),
                })
                .unwrap()
            }
            LifecycleCapabilityKind::TurnRouter => {
                serde_json::to_value(TurnRouteDecision::Continue).unwrap()
            }
            LifecycleCapabilityKind::EventObserver => json!({}),
        };

        Ok(LifecycleInvokeResponseV2 { payload })
    }

    async fn shutdown(&self) -> gestalt_runtime::Result<()> {
        Ok(())
    }
}

struct AllowAllPolicyEngine;

#[async_trait::async_trait]
impl PolicyEngine for AllowAllPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

#[tokio::test]
async fn run_session_invokes_all_pinned_lifecycle_capabilities() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LifecycleClient> = Arc::new(RecordingLifecycleClient {
        calls: calls.clone(),
    });

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(ToolUseThenEndProvider {
            turn: Mutex::new(0),
        }))
        .tools(Arc::new(TestToolCatalog {
            tool: Arc::new(TestTool),
        }))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(AllowAllPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            max_turns: 3,
            context_management_policy: Some(gestalt_core::ContextManagementPolicy {
                enabled: false,
                ..Default::default()
            }),
            trusted_extension_pins: Vec::new(),
            ..Default::default()
        })
        .build()
        .unwrap();

    let component_id = "component:com.example.lifecycle:primary:lifecycle".to_string();
    let mut snapshot = runtime.extension_manager.active_snapshot().as_ref().clone();
    snapshot.generation = RuntimeGeneration(1);
    snapshot.fingerprint = gestalt_runtime::registry::RuntimeFingerprint("test-fingerprint".into());
    snapshot.context_plan = Arc::new(ContextProviderPlan::new(vec![
        ContextProviderRegistration {
            descriptor: TypedCapabilityDescriptor {
                component_id: component_id.clone(),
                priority: 10,
                timeout: Duration::from_millis(250),
                failure_mode: CapabilityFailureMode::FailOpen,
                data_scope: CapabilityDataScope::CurrentTurn,
            },
            stability: gestalt_core::ContextStability::TurnDynamic,
            source: "test".to_string(),
        },
    ]));
    snapshot.policy_plan = Arc::new(PolicyGuardPlan::new(vec![PolicyGuardRegistration {
        descriptor: TypedCapabilityDescriptor {
            component_id: component_id.clone(),
            priority: 9,
            timeout: Duration::from_millis(250),
            failure_mode: CapabilityFailureMode::FailClosed,
            data_scope: CapabilityDataScope::ToolRequest,
        },
        source: "test".to_string(),
    }]));
    snapshot.routing_plan = Arc::new(TurnRouterPlan::new(vec![TurnRouterRegistration {
        descriptor: TypedCapabilityDescriptor {
            component_id: component_id.clone(),
            priority: 8,
            timeout: Duration::from_millis(250),
            failure_mode: CapabilityFailureMode::FailClosed,
            data_scope: CapabilityDataScope::CurrentTurn,
        },
        source: "test".to_string(),
    }]));
    snapshot.verification_plan = Arc::new(ExternalVerifierPlan::new(vec![
        ExternalVerifierRegistration {
            descriptor: TypedCapabilityDescriptor {
                component_id: component_id.clone(),
                priority: 7,
                timeout: Duration::from_millis(250),
                failure_mode: CapabilityFailureMode::FailClosed,
                data_scope: CapabilityDataScope::CurrentTurn,
            },
            source: "test".to_string(),
        },
    ]));
    snapshot.observer_plan = Arc::new(EventObserverPlan::new(vec![EventObserverRegistration {
        descriptor: TypedCapabilityDescriptor {
            component_id: component_id.clone(),
            priority: 6,
            timeout: Duration::from_millis(250),
            failure_mode: CapabilityFailureMode::Ignore,
            data_scope: CapabilityDataScope::RuntimeEvent,
        },
        source: "test".to_string(),
    }]));
    snapshot.lifecycle_clients = Arc::new(HashMap::from([(component_id.clone(), client)]));
    runtime
        .extension_manager
        .publish_snapshot(Arc::new(snapshot))
        .unwrap();

    let mut session = Session::new(
        "session-1",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 128,
            temperature: None,
            max_turns: 3,
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            resolved_model: None,
        },
        Default::default(),
        ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            workspace_root: Some(std::env::current_dir().unwrap()),
            timeout: Duration::from_secs(5),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 4096,
            artifact_dir: Some({
                let dir = std::env::temp_dir().join(format!(
                    "gestalt-runtime-lifecycle-artifacts-{}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&dir).unwrap();
                dir
            }),
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        ExecutionMode::Confirm,
        gestalt_core::snapshot::WorkspaceSnapshot {
            workspace_root: std::env::current_dir().unwrap(),
            git_sha: None,
            git_dirty: None,
            untracked_count: None,
            content_hash: "test".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );
    session.append_message(Message::User {
        content: vec![ContentBlock::Text {
            text: "run lifecycle hooks".to_string(),
        }],
        metadata: None,
    });
    session.context_policy = gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    };

    let result = runtime
        .run_session(
            &mut session,
            &gestalt_core::cancel::CancelToken::new(),
            None,
            None,
        )
        .await;

    assert!(result.is_ok(), "{result:?}");
    let calls = calls.lock().unwrap();
    assert!(calls.contains(&LifecycleCapabilityKind::ContextProvider));
    assert!(calls.contains(&LifecycleCapabilityKind::PolicyGuard));
    assert!(calls.contains(&LifecycleCapabilityKind::Verifier));
    assert!(calls.contains(&LifecycleCapabilityKind::TurnRouter));
    assert!(calls.contains(&LifecycleCapabilityKind::EventObserver));
}
