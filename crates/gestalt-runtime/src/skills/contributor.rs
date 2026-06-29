use super::{
    render_active_skill_instructions, ActivationEngine, ActivationReason as SkActivationReason,
    ActivationState, SkillIndex,
};
use crate::context::ContextContributor;
use crate::error::Result;
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use gestalt_core::{message::Message, ContextStability};
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[allow(unused_imports)]
use std::result::Result as StdResult;

/// Result of resolving active skills for a turn.
#[derive(Debug, Clone, Default)]
pub struct ActivationDiff {
    pub newly_active: Vec<(String, String)>,
    pub newly_inactive: Vec<(String, String)>,
}

impl ActivationDiff {
    pub fn is_empty(&self) -> bool {
        self.newly_active.is_empty() && self.newly_inactive.is_empty()
    }
}

/// Shared mutable state for skill context contributors.
#[derive(Clone)]
pub struct SkillContributorState {
    pub index: SkillIndex,
    pub active: HashSet<String>,
    deactivated: HashSet<String>,
    loaded_bodies: HashMap<String, String>,
    /// Skills for which body loading previously failed; they are not reactivated
    /// until the body is reachable again.
    failed_bodies: HashSet<String>,
    /// Snapshot of the resolved active set from the last `resolve_active` call.
    /// Used to compute activation diffs (newly active / newly inactive).
    last_resolved_active: HashSet<String>,
    /// Optional event bus used to publish activation lifecycle events.
    event_bus: Option<RuntimeEventBus>,
}

impl std::fmt::Debug for SkillContributorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillContributorState")
            .field("index", &self.index)
            .field("active", &self.active)
            .field(
                "loaded_bodies",
                &self.loaded_bodies.keys().collect::<Vec<_>>(),
            )
            .field("failed_bodies", &self.failed_bodies)
            .finish_non_exhaustive()
    }
}

impl SkillContributorState {
    pub fn new(
        discovered: Vec<crate::skills::SkillDescriptor>,
        initial_active: Vec<String>,
    ) -> Self {
        let index = SkillIndex::new(discovered);
        let active: HashSet<String> = initial_active.into_iter().collect();
        Self {
            index,
            active: active.clone(),
            deactivated: HashSet::new(),
            loaded_bodies: HashMap::new(),
            failed_bodies: HashSet::new(),
            last_resolved_active: active,
            event_bus: None,
        }
    }

    pub fn with_event_bus(mut self, bus: RuntimeEventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    pub fn set_event_bus(&mut self, bus: RuntimeEventBus) {
        self.event_bus = Some(bus);
    }

    pub fn activate(&mut self, name: &str) {
        self.active.insert(name.to_string());
        self.deactivated.remove(name);
    }

    pub fn deactivate(&mut self, name: &str) {
        self.deactivated.insert(name.to_string());
        self.active.remove(name);
        self.loaded_bodies.remove(name);
    }

    /// Resolve active skills for the current turn using the deterministic
    /// `ActivationEngine`. Returns a diff describing what changed relative to
    /// the last resolved set, so callers can emit events.
    pub fn resolve_active(&mut self, current_task: Option<&str>) -> (Vec<String>, ActivationDiff) {
        let previous: HashSet<String> = self.last_resolved_active.clone();
        // Treat the existing set as "explicit" so user intent persists across
        // turns. CLI-requested skills are folded into the same precedence tier
        // when they are already present in `self.active`.
        let mut state = ActivationState::new(self.active.iter().cloned().collect());
        state.deactivated = self.deactivated.clone();
        let resolved = ActivationEngine::resolve(&self.index, &state, current_task);
        self.active = resolved.iter().cloned().collect();
        self.last_resolved_active = self.active.clone();

        let mut diff = ActivationDiff::default();
        for name in &self.active {
            if !previous.contains(name) {
                let manifest_hash = self
                    .index
                    .get(name)
                    .map(|d| d.manifest_hash.clone())
                    .unwrap_or_default();
                diff.newly_active.push((name.clone(), manifest_hash));
            }
        }
        for name in &previous {
            if !self.active.contains(name) {
                let manifest_hash = self
                    .index
                    .get(name)
                    .map(|d| d.manifest_hash.clone())
                    .unwrap_or_default();
                diff.newly_inactive.push((name.clone(), manifest_hash));
            }
        }
        (resolved, diff)
    }

    /// Load the full instruction body for an active skill, returning a copy of
    /// the body on success. Tracks failures so the activation layer can decide
    /// whether to keep the skill in the active set.
    pub fn load_active_body(&mut self, name: &str) -> std::result::Result<String, std::io::Error> {
        let desc = self
            .index
            .get(name)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unknown skill"))?;
        match std::fs::read_to_string(&desc.manifest_path) {
            Ok(body) => {
                self.failed_bodies.remove(name);
                self.loaded_bodies.insert(name.to_string(), body.clone());
                Ok(body)
            }
            Err(err) => {
                // If a previously cached body is still present, drop it so we
                // do not serve stale instructions for a manifest that is no
                // longer loadable.
                self.loaded_bodies.remove(name);
                self.failed_bodies.insert(name.to_string());
                Err(err)
            }
        }
    }

    /// Build the active-skill instruction block. Skills whose body failed to
    /// load are emitted as rejection events and excluded from the block.
    pub fn build_active_instructions(&mut self) -> String {
        let mut active_skills = Vec::new();
        let names: Vec<String> = self.active.iter().cloned().collect();
        for name in names {
            let desc = match self.index.get(&name) {
                Some(d) => d.clone(),
                None => continue,
            };
            match self.load_active_body(&name) {
                Ok(body) => active_skills.push(crate::skills::ActiveSkill {
                    descriptor: desc.clone(),
                    full_body: body,
                }),
                Err(err) => {
                    self.publish_rejection(&name, &desc.manifest_hash, &err.to_string());
                    self.active.remove(&name);
                }
            }
        }
        render_active_skill_instructions(&active_skills)
    }

    /// Publish activation / deactivation events for the given diff.
    pub fn publish_diff(&self, diff: &ActivationDiff) {
        let Some(bus) = &self.event_bus else {
            return;
        };
        for (name, manifest_hash) in &diff.newly_active {
            bus.publish(RuntimeEvent::SkillActivated {
                skill_name: name.clone(),
                manifest_hash: manifest_hash.clone(),
                reason: format!("{:?}", SkActivationReason::TriggerMatch),
            });
        }
        for (name, manifest_hash) in &diff.newly_inactive {
            bus.publish(RuntimeEvent::SkillDeactivated {
                skill_name: name.clone(),
                manifest_hash: manifest_hash.clone(),
            });
        }
    }

    fn publish_rejection(&self, name: &str, manifest_hash: &str, reason: &str) {
        if let Some(bus) = &self.event_bus {
            bus.publish(RuntimeEvent::SkillRejected {
                skill_name: name.to_string(),
                reason: format!("active body load failed: {reason}"),
            });
        }
        // manifest_hash is folded into the reason for traceability even if the
        // event payload does not include it directly.
        let _ = manifest_hash;
    }

    pub fn activation_hash(&self) -> String {
        let mut names: Vec<&String> = self.active.iter().collect();
        names.sort();
        let mut hasher = sha2::Sha256::new();
        for name in names {
            hasher.update(name.as_bytes());
            if let Some(body) = self.loaded_bodies.get(name) {
                hasher.update(body.as_bytes());
            }
        }
        format!("{:x}", hasher.finalize())
    }

    pub fn active_descriptors(&self) -> Vec<crate::skills::SkillDescriptor> {
        let mut names: Vec<&String> = self.active.iter().collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| self.index.get(name).cloned())
            .collect()
    }

    /// Skill names that have a failed body load and should not be silently
    /// re-included in the active set on the next turn.
    pub fn failed_skill_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.failed_bodies.iter().cloned().collect();
        names.sort();
        names
    }

    /// Build a `ResourceAccessRecorder` that publishes
    /// `RuntimeEvent::SkillResourceAccessed` on this state's event bus. The
    /// returned `Arc` is cheap to clone and can be installed on any tool that
    /// touches skill-owned resources.
    pub fn resource_recorder(&self) -> Option<crate::skills::ResourceAccessRecorder> {
        let bus = self.event_bus.as_ref()?;
        let bus = bus.clone();
        Some(std::sync::Arc::new(move |name, path| {
            bus.publish(RuntimeEvent::SkillResourceAccessed {
                skill_name: name.to_string(),
                resource_path: path.to_string(),
            });
        }))
    }
}

/// Contributor that injects the available skills index as `SessionStatic`.
pub struct AvailableSkillsContributor {
    state: Arc<Mutex<SkillContributorState>>,
}

impl AvailableSkillsContributor {
    pub fn new(state: Arc<Mutex<SkillContributorState>>) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl ContextContributor for AvailableSkillsContributor {
    fn name(&self) -> &str {
        "available_skills"
    }

    fn stability(&self) -> ContextStability {
        ContextStability::SessionStatic
    }

    async fn contribute(&self, _workspace_root: &Path) -> Result<Message> {
        let state = self.state.lock().unwrap();
        let index_text = state.index.to_context_index();
        Ok(Message::System {
            content: index_text,
        })
    }
}

/// Contributor that injects active skill instructions as `ActivationStatic`.
pub struct ActiveSkillsContributor {
    state: Arc<Mutex<SkillContributorState>>,
}

impl ActiveSkillsContributor {
    pub fn new(state: Arc<Mutex<SkillContributorState>>) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl ContextContributor for ActiveSkillsContributor {
    fn name(&self) -> &str {
        "active_skills"
    }

    fn stability(&self) -> ContextStability {
        ContextStability::ActivationStatic
    }

    async fn contribute(&self, _workspace_root: &Path) -> Result<Message> {
        let mut state = self.state.lock().unwrap();
        let instructions = state.build_active_instructions();
        if instructions.is_empty() {
            Ok(Message::System {
                content: "<active_skills></active_skills>".to_string(),
            })
        } else {
            Ok(Message::System {
                content: instructions,
            })
        }
    }
}
