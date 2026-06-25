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
    ComponentInstanceId, ComponentKind, ExtensionInstanceHealth, ExtensionInstanceHealthStatus,
    ExtensionInventory, ExtensionLauncher, ExtensionProcessInstance, ExtensionProcessState,
    RuntimeExtensionSnapshot, RuntimeGeneration,
};

pub fn compute_dependency_lock_hash(source_root: &std::path::Path) -> Option<String> {
    let lockfiles = [
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "poetry.lock",
        "uv.lock",
        "requirements.txt",
    ];
    for lf in &lockfiles {
        let path = source_root.join(lf);
        if path.exists() {
            if let Ok(content) = std::fs::read(&path) {
                let mut hasher = Sha256::new();
                hasher.update(&content);
                return Some(format!("{:x}", hasher.finalize()));
            }
        }
    }
    None
}

pub fn compute_executable_hash(
    source_root: &std::path::Path,
    entrypoint_command: &str,
    entrypoint_args: &[String],
) -> Option<String> {
    let local_cmd = source_root.join(entrypoint_command);
    if local_cmd.exists() && local_cmd.is_file() {
        if let Ok(content) = std::fs::read(&local_cmd) {
            let mut hasher = Sha256::new();
            hasher.update(&content);
            return Some(format!("{:x}", hasher.finalize()));
        }
    }
    for arg in entrypoint_args {
        let path = source_root.join(arg);
        if path.exists() && path.is_file() {
            if let Ok(content) = std::fs::read(&path) {
                let mut hasher = Sha256::new();
                hasher.update(&content);
                return Some(format!("{:x}", hasher.finalize()));
            }
        }
    }
    None
}

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
        hasher.update(b"|");
        hasher.update(component.package_version.as_bytes());
        if let Some(mh) = &component.manifest_hash {
            hasher.update(b"|manifest:");
            hasher.update(mh.as_bytes());
        }
        if let Some(eh) = &component.executable_hash {
            hasher.update(b"|exec:");
            hasher.update(eh.as_bytes());
        }
        if let Some(lh) = &component.dependency_lock_hash {
            hasher.update(b"|lock:");
            hasher.update(lh.as_bytes());
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
    pub package_version: String,
    pub manifest_hash: Option<String>,
    pub executable_hash: Option<String>,
    pub dependency_lock_hash: Option<String>,
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

    pub async fn launch_process(
        &self,
        component: &ExtensionRuntimeComponent,
    ) -> Result<Arc<ExtensionProcessInstance>> {
        let reuse_key = component.reuse_key();
        if let Some(existing) = self
            .process_instances
            .read()
            .ok()
            .and_then(|instances| instances.get(&reuse_key).cloned())
        {
            match existing.state() {
                ExtensionProcessState::Stopped | ExtensionProcessState::Failed => {}
                _ => return Ok(existing),
            }
        }

        let process = self.launcher.launch(component).await?;
        let mut instances = self
            .process_instances
            .write()
            .map_err(|_| RuntimeError::Extension("process instance lock poisoned".to_string()))?;
        instances.insert(reuse_key, process.clone());
        Ok(process)
    }

    pub async fn drain_process(&self, component: &ExtensionRuntimeComponent) -> Result<()> {
        let reuse_key = component.reuse_key();
        let process = self
            .process_instances
            .read()
            .ok()
            .and_then(|instances| instances.get(&reuse_key).cloned())
            .ok_or_else(|| {
                RuntimeError::Extension(format!(
                    "No extension process tracked for '{}'",
                    component.id.canonical_id()
                ))
            })?;
        process.transition_to(ExtensionProcessState::Draining);
        Ok(())
    }

    pub async fn shutdown_process(&self, component: &ExtensionRuntimeComponent) -> Result<()> {
        let reuse_key = component.reuse_key();
        let process = {
            let mut instances = self.process_instances.write().map_err(|_| {
                RuntimeError::Extension("process instance lock poisoned".to_string())
            })?;
            instances.remove(&reuse_key)
        };

        if let Some(process) = process {
            process.transition_to(ExtensionProcessState::Stopping);
            process.shutdown().await;
        }

        Ok(())
    }

    pub async fn shutdown_all(&self) -> Result<()> {
        let processes = {
            let mut instances = self.process_instances.write().map_err(|_| {
                RuntimeError::Extension("process instance lock poisoned".to_string())
            })?;
            instances
                .drain()
                .map(|(_, process)| process)
                .collect::<Vec<_>>()
        };

        for process in processes {
            process.transition_to(ExtensionProcessState::Stopping);
            process.shutdown().await;
        }

        Ok(())
    }

    pub fn process_health(&self) -> Vec<ExtensionInstanceHealth> {
        let mut health = self
            .process_instances()
            .into_iter()
            .map(|process| {
                let (status, message) = match process.state() {
                    ExtensionProcessState::Ready => (ExtensionInstanceHealthStatus::Ready, None),
                    ExtensionProcessState::Starting
                    | ExtensionProcessState::Draining
                    | ExtensionProcessState::Stopping => (
                        ExtensionInstanceHealthStatus::Degraded,
                        Some(
                            match process.state() {
                                ExtensionProcessState::Starting => "process is starting",
                                ExtensionProcessState::Draining => "process is draining",
                                ExtensionProcessState::Stopping => "process is stopping",
                                _ => unreachable!(),
                            }
                            .to_string(),
                        ),
                    ),
                    ExtensionProcessState::Stopped | ExtensionProcessState::Failed => (
                        ExtensionInstanceHealthStatus::Failed,
                        Some(format!("process is {:?}", process.state()).to_lowercase()),
                    ),
                };
                ExtensionInstanceHealth {
                    instance_id: process.component_id.clone(),
                    status,
                    message,
                }
            })
            .collect::<Vec<_>>();
        health.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        health
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
            package_version: manifest.version.clone(),
            manifest_hash: None,
            executable_hash: None,
            dependency_lock_hash: None,
        };
        let reuse_key = component.reuse_key();
        if let Some(existing) = self
            .process_instances
            .read()
            .ok()
            .and_then(|instances| instances.get(&reuse_key).cloned())
        {
            if existing.state() == ExtensionProcessState::Ready {
                if let Some(broker) = existing.broker() {
                    return Ok(Arc::new(ProcessExtension::new(manifest, broker)));
                }
            }
        }

        let broker = Arc::new(
            ProcessExtensionBroker::spawn(
                manifest.clone(),
                self.event_bus.clone(),
                timeouts,
                limits,
                is_trusted,
            )
            .await?,
        );
        let process = Arc::new(ExtensionProcessInstance::with_broker(
            component.id.canonical_id(),
            broker.clone(),
        ));
        process.transition_to(ExtensionProcessState::Ready);
        let mut instances = self
            .process_instances
            .write()
            .map_err(|_| RuntimeError::Extension("process instance lock poisoned".to_string()))?;
        instances.insert(reuse_key, process);
        Ok(Arc::new(ProcessExtension::new(manifest, broker)))
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
