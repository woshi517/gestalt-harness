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
        hasher.update(component.supports_cancellation.to_string().as_bytes());
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
        hasher.update(format!("{:?}", component.trust).as_bytes());
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
type LaunchResultSender =
    tokio::sync::broadcast::Sender<std::result::Result<Arc<ExtensionProcessInstance>, String>>;
type SingleFlightMap = Arc<std::sync::Mutex<HashMap<ReuseKey, LaunchResultSender>>>;

#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionRuntimeComponent {
    pub id: ComponentInstanceId,
    pub kind: ComponentKind,
    pub optional: bool,
    pub entrypoint_command: String,
    pub entrypoint_args: Vec<String>,
    pub config: Value,
    pub grants_fingerprint: String,
    pub trust: crate::extension_trust::ExtensionTrust,
    pub protocol_fingerprint: Option<String>,
    pub package_version: String,
    pub manifest_hash: Option<String>,
    pub executable_hash: Option<String>,
    pub dependency_lock_hash: Option<String>,
    pub permissions: crate::manifest::Permissions,
    pub grants: crate::extension::ExtensionGrantConfig,
    pub package_source_root: Option<std::path::PathBuf>,
    pub supports_cancellation: bool,
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

    pub leases: Arc<std::sync::Mutex<HashMap<RuntimeGeneration, usize>>>,
    pub retired_generations:
        Arc<std::sync::Mutex<HashMap<RuntimeGeneration, Arc<RuntimeExtensionSnapshot>>>>,
    pub managed_resources:
        Arc<RwLock<HashMap<ReuseKey, crate::activation::ManagedExtensionResource>>>,
    pub single_flights: SingleFlightMap,
    pub host_context: crate::activation::HostLaunchContext,
}

impl ExtensionManager {
    pub fn new(
        initial_snapshot: Arc<RuntimeExtensionSnapshot>,
        event_bus: RuntimeEventBus,
        launcher: Arc<dyn ExtensionLauncher>,
        host_context: crate::activation::HostLaunchContext,
    ) -> Self {
        Self {
            inventory: Arc::new(RwLock::new(ExtensionInventory::default())),
            process_instances: Arc::new(RwLock::new(HashMap::new())),
            active_snapshot: Arc::new(RwLock::new(initial_snapshot)),
            reload_mutex: Arc::new(AsyncMutex::new(())),
            event_bus,
            launcher,
            leases: Arc::new(std::sync::Mutex::new(HashMap::new())),
            retired_generations: Arc::new(std::sync::Mutex::new(HashMap::new())),
            managed_resources: Arc::new(RwLock::new(HashMap::new())),
            single_flights: Arc::new(std::sync::Mutex::new(HashMap::new())),
            host_context,
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
        host_context: &crate::activation::HostLaunchContext,
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

        // Single flight logic
        let (rx, _is_leader) = {
            let mut flights = self.single_flights.lock().unwrap();
            if let Some(tx) = flights.get(&reuse_key) {
                (Some(tx.subscribe()), false)
            } else {
                let (tx, _) = tokio::sync::broadcast::channel(1);
                flights.insert(reuse_key.clone(), tx.clone());
                (None, true)
            }
        };

        if let Some(mut rx) = rx {
            match rx.recv().await {
                Ok(Ok(proc)) => return Ok(proc),
                Ok(Err(e)) => return Err(RuntimeError::Extension(e)),
                Err(_) => {
                    return Err(RuntimeError::Extension(
                        "Single-flight launch channel closed".to_string(),
                    ))
                }
            }
        }

        let result = self.launcher.launch(component, host_context).await;

        let flights_tx = {
            let mut flights = self.single_flights.lock().unwrap();
            flights.remove(&reuse_key)
        };

        if let Some(tx) = flights_tx {
            let broadcast_val = result
                .as_ref()
                .map(|p| p.clone())
                .map_err(|e| e.to_string());
            let _ = tx.send(broadcast_val);
        }

        let process = result?;

        let mut instances = self
            .process_instances
            .write()
            .map_err(|_| RuntimeError::Extension("process instance lock poisoned".to_string()))?;
        instances.insert(reuse_key.clone(), process.clone());

        if let Ok(mut managed) = self.managed_resources.write() {
            let managed_key = reuse_key.clone();
            managed.insert(
                managed_key,
                crate::activation::ManagedExtensionResource::Process {
                    reuse_key,
                    process: process.clone(),
                },
            );
        }

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
            if let Ok(mut managed) = self.managed_resources.write() {
                managed.remove(&reuse_key);
            }
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
        if let Ok(mut managed) = self.managed_resources.write() {
            managed.clear();
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

    pub fn combined_health(
        &self,
        snapshot: &RuntimeExtensionSnapshot,
    ) -> Vec<ExtensionInstanceHealth> {
        fn health_rank(status: &ExtensionInstanceHealthStatus) -> u8 {
            match status {
                ExtensionInstanceHealthStatus::Ready => 0,
                ExtensionInstanceHealthStatus::Degraded => 1,
                ExtensionInstanceHealthStatus::Failed => 2,
            }
        }

        let mut by_instance = std::collections::BTreeMap::<String, ExtensionInstanceHealth>::new();

        for health in self.process_health() {
            by_instance.insert(health.instance_id.clone(), health);
        }

        for diagnostic in snapshot.diagnostics.iter() {
            let diagnostic_status = match diagnostic.severity {
                crate::activation::DiagnosticSeverity::Warning => {
                    ExtensionInstanceHealthStatus::Degraded
                }
                crate::activation::DiagnosticSeverity::Error => {
                    ExtensionInstanceHealthStatus::Failed
                }
            };

            by_instance
                .entry(diagnostic.component_id.canonical_id())
                .and_modify(|existing| {
                    if health_rank(&diagnostic_status) > health_rank(&existing.status) {
                        existing.status = diagnostic_status.clone();
                    }
                    existing.message = match (&existing.message, &diagnostic.message) {
                        (Some(current), next) if current == next => Some(current.clone()),
                        (Some(current), next) => Some(format!("{current}; {next}")),
                        (None, next) => Some(next.clone()),
                    };
                })
                .or_insert_with(|| ExtensionInstanceHealth {
                    instance_id: diagnostic.component_id.canonical_id(),
                    status: diagnostic_status,
                    message: Some(diagnostic.message.clone()),
                });
        }

        by_instance.into_values().collect()
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
            supports_cancellation: manifest.capabilities.supports_cancellation,
            entrypoint_command: manifest.entrypoint.command.clone(),
            entrypoint_args: manifest.entrypoint.args.clone(),
            config: serde_json::Value::Null,
            grants_fingerprint: fingerprint_json(&manifest.permissions),
            trust: if is_trusted {
                crate::extension_trust::ExtensionTrust::BuiltIn
            } else {
                crate::extension_trust::ExtensionTrust::Untrusted
            },
            protocol_fingerprint: manifest.protocol_version.clone(),
            package_version: manifest.version.clone(),
            manifest_hash: None,
            executable_hash: None,
            dependency_lock_hash: None,
            permissions: manifest.permissions.clone(),
            grants: crate::extension::ExtensionGrantConfig {
                workspace_read: true,
                workspace_write: true,
                shell: true,
                network: vec!["*".to_string()],
                allowed_paths: vec![std::path::PathBuf::from("*")],
            },
            package_source_root: None,
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
            ProcessExtensionBroker::spawn_with_grants(
                manifest.clone(),
                Some(component.grants.clone()),
                None,
                self.event_bus.clone(),
                timeouts,
                limits,
                self.host_context.allow_network,
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

    pub fn acquire_lease(self: &Arc<Self>) -> crate::activation::RuntimeSnapshotLease {
        let snapshot = self.active_snapshot();
        let gen = snapshot.generation;

        let mut leases = self.leases.lock().unwrap();
        *leases.entry(gen).or_insert(0) += 1;

        let retirement = Arc::new(crate::activation::GenerationRetirement { generation: gen });

        crate::activation::RuntimeSnapshotLease {
            snapshot,
            retirement,
            manager: Arc::downgrade(self),
        }
    }

    pub fn release_lease(&self, gen: RuntimeGeneration) {
        let mut leases = self.leases.lock().unwrap();
        if let Some(count) = leases.get_mut(&gen) {
            *count -= 1;
            if *count == 0 {
                leases.remove(&gen);
                let previous = self.retired_generations.lock().unwrap().remove(&gen);
                if let Some(prev) = previous {
                    let active = self.active_snapshot();
                    self.drain_unused_resources(&prev, &active);
                }
            }
        }
    }

    pub fn publish_snapshot(&self, snapshot: Arc<RuntimeExtensionSnapshot>) -> Result<()> {
        let previous = {
            let mut guard = self.active_snapshot.write().map_err(|_| {
                RuntimeError::Extension("active snapshot lock poisoned".to_string())
            })?;
            let prev = guard.clone();
            *guard = snapshot.clone();
            prev
        };

        let gen = previous.generation;
        let has_leases = {
            let leases = self.leases.lock().unwrap();
            leases.contains_key(&gen)
        };

        if has_leases {
            self.retired_generations
                .lock()
                .unwrap()
                .insert(gen, previous);
        } else {
            self.drain_unused_resources(&previous, &snapshot);
        }

        Ok(())
    }

    fn drain_unused_resources(
        &self,
        previous: &RuntimeExtensionSnapshot,
        active: &RuntimeExtensionSnapshot,
    ) {
        for res in previous.managed_resources.iter() {
            let reuse_key = res.reuse_key().clone();
            let is_reused = active
                .managed_resources
                .iter()
                .any(|r| r.reuse_key() == &reuse_key);
            if !is_reused {
                let res_clone = res.clone();
                let process_instances = self.process_instances.clone();
                let managed_resources = self.managed_resources.clone();
                tokio::spawn(async move {
                    match res_clone {
                        crate::activation::ManagedExtensionResource::Process {
                            process: p, ..
                        } => {
                            p.transition_to(ExtensionProcessState::Draining);
                            for _ in 0..20 {
                                if p.in_flight_calls() == 0 {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            }
                            p.transition_to(ExtensionProcessState::Stopping);
                            p.shutdown().await;
                            if let Ok(mut instances) = process_instances.write() {
                                instances.remove(&reuse_key);
                            }
                            if let Ok(mut managed) = managed_resources.write() {
                                managed.remove(&reuse_key);
                            }
                        }
                        crate::activation::ManagedExtensionResource::Mcp { .. } => {
                            // MCP draining
                        }
                        crate::activation::ManagedExtensionResource::Observer { .. } => {
                            // Observer draining
                        }
                    }
                });
            }
        }
    }
}

fn fingerprint_json<T: serde::Serialize>(value: &T) -> String {
    let mut hasher = Sha256::new();
    match serde_json::to_vec(value) {
        Ok(serialized) => hasher.update(serialized),
        Err(err) => hasher.update(err.to_string().as_bytes()),
    }
    format!("{:x}", hasher.finalize())
}
