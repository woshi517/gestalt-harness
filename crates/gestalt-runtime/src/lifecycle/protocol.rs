pub const PROTOCOL_V2_METHOD_INITIALIZE: &str = "initialize";
pub const PROTOCOL_V2_METHOD_DESCRIBE_CAPABILITIES: &str = "capabilities/describe";
pub const PROTOCOL_V2_METHOD_INVOKE: &str = "lifecycle/invoke";
pub const PROTOCOL_V2_METHOD_SHUTDOWN: &str = "shutdown";
pub const PROTOCOL_V2_METHOD_CANCEL: &str = "$/cancelRequest";

pub const PROTOCOL_V2_METHODS: &[&str] = &[
    PROTOCOL_V2_METHOD_INITIALIZE,
    PROTOCOL_V2_METHOD_DESCRIBE_CAPABILITIES,
    PROTOCOL_V2_METHOD_INVOKE,
    PROTOCOL_V2_METHOD_SHUTDOWN,
    PROTOCOL_V2_METHOD_CANCEL,
];

pub fn negotiate_protocol_version(supported_versions: &[String]) -> Option<String> {
    if supported_versions.iter().any(|version| version == "2.0") {
        Some("2.0".to_string())
    } else if supported_versions.iter().any(|version| version == "1.0") {
        Some("1.0".to_string())
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitializeRequestV2 {
    pub supported_versions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitializeResponseV2 {
    pub negotiated_version: String,
    pub supports_cancellation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CapabilityDescriptorV2 {
    pub component_id: String,
    pub capability: LifecycleCapabilityKind,
    pub priority: i32,
    pub timeout_ms: u64,
    pub failure_mode: String,
    pub data_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCapabilityKind {
    ContextProvider,
    PolicyGuard,
    TurnRouter,
    Verifier,
    EventObserver,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LifecycleInvokeRequestV2 {
    pub component_id: String,
    pub capability: LifecycleCapabilityKind,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LifecycleInvokeResponseV2 {
    pub payload: serde_json::Value,
}
