use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityFailureMode {
    FailClosed,
    FailOpen,
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityDataScope {
    None,
    ToolRequest,
    CurrentTurn,
    ProjectedContext,
    RuntimeEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedCapabilityDescriptor {
    pub component_id: String,
    pub priority: i32,
    pub timeout: Duration,
    pub failure_mode: CapabilityFailureMode,
    pub data_scope: CapabilityDataScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextProviderRegistration {
    pub descriptor: TypedCapabilityDescriptor,
    pub stability: gestalt_core::ContextStability,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextProviderPlan {
    pub registrations: Arc<[ContextProviderRegistration]>,
}

impl ContextProviderPlan {
    pub fn new(registrations: Vec<ContextProviderRegistration>) -> Self {
        Self {
            registrations: Arc::from(registrations),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyGuardRegistration {
    pub descriptor: TypedCapabilityDescriptor,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyGuardPlan {
    pub registrations: Arc<[PolicyGuardRegistration]>,
}

impl PolicyGuardPlan {
    pub fn new(registrations: Vec<PolicyGuardRegistration>) -> Self {
        Self {
            registrations: Arc::from(registrations),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRouterRegistration {
    pub descriptor: TypedCapabilityDescriptor,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRouterPlan {
    pub registrations: Arc<[TurnRouterRegistration]>,
}

impl TurnRouterPlan {
    pub fn new(registrations: Vec<TurnRouterRegistration>) -> Self {
        Self {
            registrations: Arc::from(registrations),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalVerifierRegistration {
    pub descriptor: TypedCapabilityDescriptor,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExternalVerifierPlan {
    pub registrations: Arc<[ExternalVerifierRegistration]>,
}

impl ExternalVerifierPlan {
    pub fn new(registrations: Vec<ExternalVerifierRegistration>) -> Self {
        Self {
            registrations: Arc::from(registrations),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventObserverRegistration {
    pub descriptor: TypedCapabilityDescriptor,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventObserverPlan {
    pub registrations: Arc<[EventObserverRegistration]>,
}

impl EventObserverPlan {
    pub fn new(registrations: Vec<EventObserverRegistration>) -> Self {
        Self {
            registrations: Arc::from(registrations),
        }
    }
}
