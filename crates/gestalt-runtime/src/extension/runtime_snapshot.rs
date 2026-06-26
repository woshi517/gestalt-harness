use std::sync::Arc;

use gestalt_core::tool::ToolCatalog;

use crate::lifecycle::{
    CapabilityDataScope, CapabilityFailureMode, ContextProviderPlan, ContextProviderRegistration,
    EventObserverPlan, ExternalVerifierPlan, PolicyGuardPlan, PolicyGuardRegistration,
    TurnRouterPlan, TypedCapabilityDescriptor,
};
use crate::registry::{RuntimeFingerprint, RuntimeRegistrySnapshot};

use super::ExtensionProcessInstance;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeGeneration(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionInstanceHealthStatus {
    Ready,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInstanceHealth {
    pub instance_id: String,
    pub status: ExtensionInstanceHealthStatus,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeExtensionSnapshot {
    pub generation: RuntimeGeneration,
    pub fingerprint: RuntimeFingerprint,
    pub registry_snapshot: RuntimeRegistrySnapshot,
    pub tool_catalog: Arc<dyn ToolCatalog>,
    pub context_plan: Arc<ContextProviderPlan>,
    pub policy_plan: Arc<PolicyGuardPlan>,
    pub routing_plan: Arc<TurnRouterPlan>,
    pub verification_plan: Arc<ExternalVerifierPlan>,
    pub observer_plan: Arc<EventObserverPlan>,
    pub mcp_registry: Arc<gestalt_mcp::McpRegistry>,
    pub process_instances: Arc<[Arc<ExtensionProcessInstance>]>,
    pub package_health: Arc<[ExtensionInstanceHealth]>,
    pub diagnostics: Arc<[crate::activation::ActivationDiagnostic]>,
    pub managed_resources: Arc<[crate::activation::ManagedExtensionResource]>,
    pub negotiated_protocol: Arc<std::collections::HashMap<String, String>>,
    pub resolved_packages: Arc<[crate::extension::ResolvedExtensionPackage]>,
}

impl RuntimeExtensionSnapshot {
    pub fn from_registry_snapshot(
        generation: RuntimeGeneration,
        registry_snapshot: RuntimeRegistrySnapshot,
        tool_catalog: Arc<dyn ToolCatalog>,
        mcp_registry: Arc<gestalt_mcp::McpRegistry>,
    ) -> Self {
        let context_plan = Self::context_plan_from_registry(&registry_snapshot, false);
        let fingerprint = registry_snapshot.fingerprint.clone();
        Self {
            generation,
            fingerprint,
            registry_snapshot,
            tool_catalog,
            context_plan: Arc::new(context_plan),
            policy_plan: Arc::new(PolicyGuardPlan::default()),
            routing_plan: Arc::new(TurnRouterPlan::default()),
            verification_plan: Arc::new(ExternalVerifierPlan::default()),
            observer_plan: Arc::new(EventObserverPlan::default()),
            mcp_registry,
            process_instances: Arc::from([]),
            package_health: Arc::from([]),
            diagnostics: Arc::from([]),
            managed_resources: Arc::from([]),
            negotiated_protocol: Arc::new(std::collections::HashMap::new()),
            resolved_packages: Arc::from([]),
        }
    }

    pub fn tool_catalog(&self) -> Arc<dyn ToolCatalog> {
        self.tool_catalog.clone()
    }

    pub fn mcp_registry(&self) -> Arc<gestalt_mcp::McpRegistry> {
        self.mcp_registry.clone()
    }

    pub fn with_native_composition_plans(mut self) -> Self {
        self.context_plan = Arc::new(Self::context_plan_from_registry(
            &self.registry_snapshot,
            true,
        ));
        self.policy_plan = Arc::new(PolicyGuardPlan::new(vec![PolicyGuardRegistration {
            descriptor: TypedCapabilityDescriptor {
                component_id: "native:composition_hooks:before_tool_policy".to_string(),
                priority: 0,
                timeout: std::time::Duration::from_secs(5),
                failure_mode: CapabilityFailureMode::FailClosed,
                data_scope: CapabilityDataScope::ToolRequest,
            },
            source: "native-composition-hooks".to_string(),
        }]));
        self
    }

    pub fn with_context_plan(mut self, context_plan: ContextProviderPlan) -> Self {
        self.context_plan = Arc::new(context_plan);
        self
    }

    pub fn with_policy_plan(mut self, policy_plan: PolicyGuardPlan) -> Self {
        self.policy_plan = Arc::new(policy_plan);
        self
    }

    pub fn context_plan_from_registry(
        registry_snapshot: &RuntimeRegistrySnapshot,
        include_native_composition: bool,
    ) -> ContextProviderPlan {
        let mut registrations = registry_snapshot
            .context_contributors
            .values()
            .map(|contributor| ContextProviderRegistration {
                descriptor: TypedCapabilityDescriptor {
                    component_id: contributor
                        .extension_id
                        .clone()
                        .unwrap_or_else(|| format!("native:context:{}", contributor.name)),
                    priority: 0,
                    timeout: std::time::Duration::from_secs(15),
                    failure_mode: CapabilityFailureMode::FailOpen,
                    data_scope: CapabilityDataScope::CurrentTurn,
                },
                stability: contributor.stability,
                source: contributor.name.clone(),
            })
            .collect::<Vec<_>>();

        if include_native_composition {
            registrations.push(ContextProviderRegistration {
                descriptor: TypedCapabilityDescriptor {
                    component_id: "native:composition_hooks:before_context_build".to_string(),
                    priority: 0,
                    timeout: std::time::Duration::from_secs(15),
                    failure_mode: CapabilityFailureMode::FailOpen,
                    data_scope: CapabilityDataScope::CurrentTurn,
                },
                stability: gestalt_core::ContextStability::TurnDynamic,
                source: "native-composition-hooks".to_string(),
            });
        }

        ContextProviderPlan::new(registrations)
    }
}
