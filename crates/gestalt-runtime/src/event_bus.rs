use gestalt_core::event::AgentEvent;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Agent {
        sequence_number: u64,
        event: AgentEvent,
    },
    ExtensionDiscovered {
        extension_id: String,
        manifest_path: String,
        manifest_hash: String,
    },
    ExtensionLoaded {
        extension_id: String,
    },
    ExtensionRejected {
        extension_id: String,
        reason: String,
    },
    ExtensionError {
        extension_id: String,
        message: String,
    },
    HookStarted {
        hook_name: String,
        lifecycle_point: String,
    },
    HookCompleted {
        hook_name: String,
        lifecycle_point: String,
        outcome: String,
    },
    HookFailed {
        hook_name: String,
        lifecycle_point: String,
        error: String,
    },
    ToolRegistered {
        extension_id: Option<String>,
        tool_name: String,
        schema_hash: String,
    },
    ContextInjectorRegistered {
        extension_id: Option<String>,
        injector_name: String,
    },
    PermissionDecision {
        extension_id: String,
        capability: String,
        permission: String,
        resource: Option<String>,
        granted: bool,
        reason: Option<String>,
    },
    ProcessSpawned {
        extension_id: String,
        pid: u32,
    },
    ProcessExited {
        extension_id: String,
        exit_code: Option<i32>,
    },
    ProcessKilled {
        extension_id: String,
        reason: String,
    },
    RpcRequest {
        extension_id: String,
        method: String,
        request_id: String,
    },
    RpcResponse {
        extension_id: String,
        method: String,
        request_id: String,
        success: bool,
    },
    ArtifactRouted {
        session_id: String,
        path: String,
        size_bytes: usize,
    },
    SessionSpawned {
        session_id: String,
    },
    ReloadStarted,
    ReloadCompleted,
    RuntimeError {
        message: String,
    },
    SkillDiscovered {
        skill_name: String,
        manifest_hash: String,
        source: String,
        trust_level: String,
    },
    SkillActivated {
        skill_name: String,
        manifest_hash: String,
        reason: String,
    },
    SkillDeactivated {
        skill_name: String,
        manifest_hash: String,
    },
    SkillRejected {
        skill_name: String,
        reason: String,
    },
    SkillPolicyApplied {
        skill_name: String,
        allowed_tools: Vec<String>,
    },
    SkillResourceAccessed {
        skill_name: String,
        resource_path: String,
    },
}

#[derive(Clone)]
pub struct RuntimeEventBus {
    tx: broadcast::Sender<Arc<RuntimeEvent>>,
    next_seq: Arc<std::sync::atomic::AtomicU64>,
    history: Arc<std::sync::Mutex<Vec<RuntimeEvent>>>,
}

impl RuntimeEventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self {
            tx,
            next_seq: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            history: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn publish(&self, event: RuntimeEvent) {
        {
            if let Ok(mut lock) = self.history.lock() {
                lock.push(event.clone());
            }
        }
        let _ = self.tx.send(Arc::new(event));
    }

    pub fn history(&self) -> Vec<RuntimeEvent> {
        if let Ok(lock) = self.history.lock() {
            lock.clone()
        } else {
            Vec::new()
        }
    }

    pub fn publish_agent(&self, agent_event: AgentEvent) -> u64 {
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.publish(RuntimeEvent::Agent {
            sequence_number: seq,
            event: agent_event,
        });
        seq
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>> {
        self.tx.subscribe()
    }
}

impl Default for RuntimeEventBus {
    fn default() -> Self {
        Self::new()
    }
}
