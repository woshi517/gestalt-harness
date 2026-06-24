use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;

use crate::config::{ExtensionLimitsConfig, ExtensionTimeoutsConfig};
use crate::error::{Result, RuntimeError};
use crate::event_bus::RuntimeEventBus;
use crate::manifest::ExtensionManifest;
use crate::process_extension::{ProcessExtension, ProcessExtensionBroker};

use super::{
    ComponentInstanceId, ComponentKind, ExtensionInventory, ExtensionLauncher,
    ExtensionProcessInstance, ExtensionProcessState, RuntimeExtensionSnapshot, RuntimeGeneration,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComponentFingerprint(pub String);

impl ComponentFingerprint {
    pub fn from_component(component: &ExtensionRuntimeComponent) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(component.id.canonical_id().as_bytes());
        hasher.update(b"|");
        hasher.update(format!("{:?}", component.kind).as_bytes());
        hasher.update(b"|");
        hasher.update(component.optional.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(component.entrypoint_command.as_bytes());
        for arg in &component.entrypoint_args {
            hasher.update(b"\0");
            hasher.update(arg.as_bytes());
        }
        hasher.update(b"|");
        hasher.update(
            serde_json::to_string(&component.config)
                .unwrap_or_default()
                .as_bytes(),
        );
        hasher.update(b"|");
        hasher.update(component.grants_fingerprint.as_bytes());
        hasher.update(b"|");
        hasher.update(component.trust_fingerprint.as_bytes());
        if let Some(protocol) = &component.protocol_fingerprint {
            hasher.update(b"|");
            hasher.update(protocol.as_bytes());
        }
        Self(format!("{:x}", hasher.finalize()))
    }
}

pub type ReuseKey = (ComponentInstanceId, ComponentFingerprint);

#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionRuntimeComponent {
    pub id: ComponentInstanceId,
    pub kind: ComponentKind,
    pub optional: bool,
    pub entrypoint_command: String,
    pub entrypoint_args: Vec<String>,
    pub config: Value,
    pub grants_fingerprint: String,
    pub trust_fingerprint: String,
    pub protocol_fingerprint: Option<String>,
}

impl ExtensionRuntimeComponent {
    pub fn reuse_key(&self) -> ReuseKey {
        (self.id.clone(), ComponentFingerprint::from_component(self))
    }
}

pub struct ExtensionManager {
    inventory: Arc<RwLock<ExtensionInventory>>,
    process_instances: Arc<RwLock<HashMap<ReuseKey, Arc<ExtensionProcessInstance>>>>,
    active_snapshot: Arc<RwLock<Arc<RuntimeExtensionSnapshot>>>,
    pub reload_mutex: Arc<AsyncMutex<()>>,
    pub event_bus: RuntimeEventBus,
    pub launcher: Arc<dyn ExtensionLauncher>,
}

impl ExtensionManager {
    pub fn new(
        initial_snapshot: Arc<RuntimeExtensionSnapshot>,
        event_bus: RuntimeEventBus,
        launcher: Arc<dyn ExtensionLauncher>,
    ) -> Self {
        Self {
            inventory: Arc::new(RwLock::new(ExtensionInventory::default())),
            process_instances: Arc::new(RwLock::new(HashMap::new())),
            active_snapshot: Arc::new(RwLock::new(initial_snapshot)),
            reload_mutex: Arc::new(AsyncMutex::new(())),
            event_bus,
            launcher,
        }
    }

    pub fn with_inventory(self, inventory: ExtensionInventory) -> Self {
        if let Ok(mut guard) = self.inventory.write() {
            *guard = inventory;
        }
        self
    }

    pub fn inventory(&self) -> ExtensionInventory {
        self.inventory
            .read()
            .map(|inventory| inventory.clone())
            .unwrap_or_default()
    }

    pub fn active_snapshot(&self) -> Arc<RuntimeExtensionSnapshot> {
        self.active_snapshot.read().map_or_else(
            |_| panic!("extension manager active snapshot lock poisoned"),
            |snapshot| snapshot.clone(),
        )
    }

    pub fn current_generation(&self) -> RuntimeGeneration {
        self.active_snapshot().generation
    }

    pub fn process_instances(&self) -> Vec<Arc<ExtensionProcessInstance>> {
        self.process_instances
            .read()
            .map(|instances| instances.values().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn launch_legacy_process_extension(
        &self,
        manifest: ExtensionManifest,
        timeouts: ExtensionTimeoutsConfig,
        limits: ExtensionLimitsConfig,
        is_trusted: bool,
    ) -> Result<Arc<ProcessExtension>> {
        let component = ExtensionRuntimeComponent {
            id: ComponentInstanceId::new(&manifest.id, "default", "legacy-process"),
            kind: ComponentKind::LegacyProcess,
            optional: false,
            entrypoint_command: manifest.entrypoint.command.clone(),
            entrypoint_args: manifest.entrypoint.args.clone(),
            config: serde_json::Value::Null,
            grants_fingerprint: format!("{:?}", manifest.permissions),
            trust_fingerprint: is_trusted.to_string(),
            protocol_fingerprint: manifest.protocol_version.clone(),
        };
        let reuse_key = component.reuse_key();
        if let Some(existing) = self
            .process_instances
            .read()
            .ok()
            .and_then(|instances| instances.get(&reuse_key).cloned())
        {
            if existing.state() == ExtensionProcessState::Ready {
                // The legacy ProcessExtension wrapper still owns broker calls;
                // reuse is tracked here for snapshot decisions and future MCP reuse.
            }
        }

        let process = Arc::new(ExtensionProcessInstance::new(component.id.canonical_id()));
        let broker = ProcessExtensionBroker::spawn(
            manifest.clone(),
            self.event_bus.clone(),
            timeouts,
            limits,
            is_trusted,
        )
        .await?;
        process.transition_to(ExtensionProcessState::Ready);
        let mut instances = self
            .process_instances
            .write()
            .map_err(|_| RuntimeError::Extension("process instance lock poisoned".to_string()))?;
        instances.insert(reuse_key, process);
        Ok(Arc::new(ProcessExtension::new(manifest, Arc::new(broker))))
    }

    pub fn publish_snapshot(&self, snapshot: Arc<RuntimeExtensionSnapshot>) -> Result<()> {
        let mut guard = self
            .active_snapshot
            .write()
            .map_err(|_| RuntimeError::Extension("active snapshot lock poisoned".to_string()))?;
        *guard = snapshot;
        Ok(())
    }
}
