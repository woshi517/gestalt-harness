#![forbid(unsafe_code)]
//! Unified `Extension` surface, composable `AgentLoop` / `LoopDecorator` /
//! `ContextBuilder` traits, declarative `UISurface` specs, and `ArtifactStoreView`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::effect::{BudgetSnapshot, Effect, LoopConfig, PauseReason, SideEffectClass, StepResult};
use crate::error::{ExtensionError, KernelError};
use crate::policy::PolicyRule;
use crate::steering::SteeringSignal;
use crate::tree::SessionSlice;

/// Capabilities exposed by a target model provider to `ContextBuilder`s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Maximum context window size in tokens.
    pub max_context_tokens: u64,
    /// Maximum output generation tokens.
    pub max_output_tokens: u64,
    /// Whether the target provider supports prefix prompt caching.
    pub supports_prompt_caching: bool,
    /// Whether the target provider emits structured reasoning traces.
    pub supports_reasoning: bool,
    /// Whether the target provider accepts multimodal media blocks.
    pub supports_vision: bool,
}

/// Snapshot of a single workspace/harness artifact stored in [`ArtifactStoreView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEntry {
    /// Workspace-relative artifact path.
    pub path: String,
    /// UTF-8 artifact content.
    pub content: String,
    /// Content-addressed SHA-256 blob digest.
    pub blob_digest: String,
}

/// Read-only view of workspace/harness artifacts backed by live files during
/// normal runs or `.gestalt/blobs/<sha256>` snapshots during offline replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactStoreView {
    entries: BTreeMap<String, ArtifactEntry>,
}

impl ArtifactStoreView {
    /// Construct an [`ArtifactStoreView`] from a map of relative path to [`ArtifactEntry`].
    #[must_use]
    pub fn from_entries(entries: BTreeMap<String, ArtifactEntry>) -> Self {
        Self { entries }
    }

    /// Look up an artifact entry by workspace-relative path.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&ArtifactEntry> {
        self.entries.get(path)
    }

    /// Return all stored artifact entries.
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, ArtifactEntry> {
        &self.entries
    }

    /// Export `{ relative_path -> sha256_blob_digest }` for `turn_start.artifact_snapshots`.
    #[must_use]
    pub fn snapshots(&self) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .map(|(k, v)| (k.clone(), v.blob_digest.clone()))
            .collect()
    }
}

/// Declarative descriptor for a registered tool slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique tool slot name.
    pub name: String,
    /// Human-readable description shown to models/operators.
    pub description: String,
    /// JSON Schema for tool arguments.
    pub parameters_schema: Value,
    /// Default side-effect classification enforced by `PolicyGate`.
    pub default_side_effect: SideEffectClass,
    /// Optional name of the external extension providing this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_extension: Option<String>,
}

/// Declarative descriptor for a registered slash command slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Command name without leading slash (e.g., `"goal"`).
    pub name: String,
    /// Short description for help listings.
    pub description: String,
    /// Usage synopsis (e.g., `"/goal <criteria>"`).
    pub usage: String,
    /// Optional name of the external extension providing this command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_extension: Option<String>,
}

/// Declarative renderer specification for structured `RunEvent::ToolUi` payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UIRendererSpec {
    /// Unique renderer identifier.
    pub id: String,
    /// Tool or event kind this renderer handles.
    pub target_kind: String,
    /// Display title.
    pub title: String,
    /// Optional JSON schema describing expected `data` payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

/// Declarative persistent sidebar/inspector panel specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UIPanelSpec {
    /// Unique panel identifier.
    pub id: String,
    /// Panel display title.
    pub title: String,
    /// Preferred host layout position (e.g., `"right"`, `"bottom"`).
    pub position: String,
    /// Optional JSON schema for panel state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

/// Declarative interactive dialog specification for `Effect::Pause`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UIDialogSpec {
    /// Unique dialog identifier.
    pub id: String,
    /// Pause reason this dialog handles.
    pub pause_reason: PauseReason,
    /// Dialog title.
    pub title: String,
    /// Optional JSON schema for dialog input/response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

/// Declarative UI action button/shortcut binding to a steering signal or command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UIActionSpec {
    /// Unique action identifier.
    pub id: String,
    /// Button/menu label.
    pub label: String,
    /// Slash command or steering signal template emitted on activation.
    pub command_or_signal: String,
    /// Optional keyboard shortcut hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
}

/// Protocol-neutral UI sub-contract inside an [`Extension`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UISurface {
    /// Custom tool/event card renderers keyed by renderer ID.
    #[serde(default)]
    pub renderers: BTreeMap<String, UIRendererSpec>,
    /// Persistent inspector/sidebar panels keyed by panel ID.
    #[serde(default)]
    pub panels: BTreeMap<String, UIPanelSpec>,
    /// Interactive dialogs for `Effect::Pause` keyed by dialog ID.
    #[serde(default)]
    pub dialogs: BTreeMap<String, UIDialogSpec>,
    /// UI-triggered actions keyed by action ID.
    #[serde(default)]
    pub actions: BTreeMap<String, UIActionSpec>,
}

impl UISurface {
    /// Merge another [`UISurface`] into `self` (later registrations override by key).
    pub fn merge(&mut self, other: Self) {
        self.renderers.extend(other.renderers);
        self.panels.extend(other.panels);
        self.dialogs.extend(other.dialogs);
        self.actions.extend(other.actions);
    }
}

/// Composable preset wiring a base loop, ordered decorators, context builder,
/// and `LoopConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// Base `AgentLoopFactory` slot ID.
    pub base_loop: String,
    /// Ordered `LoopDecorator` slot IDs wrapping `base_loop`.
    #[serde(default)]
    pub decorators: Vec<String>,
    /// Active `ContextBuilder` slot ID.
    pub builder: String,
    /// Effective `LoopConfig` for Phases opened with this preset.
    #[serde(default)]
    pub config: LoopConfig,
}

/// Descriptor for an out-of-process loop, decorator, builder, or policy
/// returned by an external worker's `extension/describe` handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalComponentSpec {
    /// Component slot ID.
    pub id: String,
    /// Deterministic version/source digest recorded on `phase_open`.
    pub digest: String,
}

/// Complete capability descriptor returned by an out-of-process extension over
/// stdio JSON-RPC `extension/describe`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExternalExtensionSpec {
    /// Unique external extension name.
    pub name: String,
    /// Loops hosted out-of-process by this worker.
    #[serde(default)]
    pub loops: Vec<ExternalComponentSpec>,
    /// Decorators hosted out-of-process by this worker.
    #[serde(default)]
    pub decorators: Vec<ExternalComponentSpec>,
    /// Presets declared by this extension.
    #[serde(default)]
    pub presets: BTreeMap<String, Preset>,
    /// Context builders hosted out-of-process by this worker.
    #[serde(default)]
    pub builders: Vec<ExternalComponentSpec>,
    /// Tools hosted by this worker.
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// Slash commands hosted by this worker.
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    /// Policy rule IDs hosted by this worker.
    #[serde(default)]
    pub policies: Vec<ExternalComponentSpec>,
    /// Declarative UI surface specs contributed by this worker.
    #[serde(default)]
    pub ui: UISurface,
}

/// Kernel-owned execution context passed to `AgentLoop::step`.
///
/// Passed as `&mut dyn LoopContext` so loops remain completely decoupled from
/// `KernelRuntime` internals and never access system clocks or OS RNG.
pub trait LoopContext {
    /// Active session/run identifier.
    fn run_id(&self) -> &str;
    /// Active Turn identifier.
    fn turn_id(&self) -> &str;
    /// Active Phase identifier.
    fn phase_id(&self) -> &str;
    /// Typed handoff JSON passed into this Phase (or `Value::Null`).
    fn handoff(&self) -> &Value;
    /// Effective kernel-bounded `LoopConfig` for this Phase.
    fn config(&self) -> &LoopConfig;
    /// Read-only chronological slice of the active `SessionTree` branch.
    fn session(&self) -> &SessionSlice;
    /// Read-only artifact view (live or blob-snapshotted during replay).
    fn artifacts(&self) -> &ArtifactStoreView;
    /// Remaining step, token, and micro-USD budget snapshot.
    fn budget_remaining(&self) -> &BudgetSnapshot;
    /// Invoke the active Phase's configured `ContextBuilder`.
    fn build_context(&self, caps: &ModelCapabilities) -> Value;
    /// Drain pending intra-turn `SteeringSignal`s in FIFO order.
    fn drain_steering(&mut self) -> Vec<SteeringSignal>;
    /// Deterministic kernel-provided timestamp in milliseconds.
    fn now_ms(&self) -> u64;
    /// Deterministic monotonic ID generator for replay-safe user-space logic.
    fn next_id(&mut self) -> String;
}

/// Resumable sans-I/O state-machine action returned by `AgentLoop::step`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "payload", rename_all = "snake_case")]
pub enum LoopAction {
    /// Yield one or more effects to run in parallel.
    Yield(Vec<Effect>),
    /// Complete the active Phase/Turn with a typed output JSON value.
    Done(Value),
}

/// User-space control-flow state machine driving a Phase or subagent.
pub trait AgentLoop: Send {
    /// Unique loop identifier.
    fn id(&self) -> &str;
    /// Deterministic version/source digest recorded on `phase_open` for replay.
    fn digest(&self) -> &str;
    /// Advance the loop given [`LoopContext`] and optional prior [`StepResult`]s.
    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction;
}

/// Factory registered in [`ExtensionRegistry`] so every Phase and `Spawn`
/// receives a fresh [`AgentLoop`] instance.
pub trait AgentLoopFactory: Send + Sync {
    /// Unique loop identifier.
    fn id(&self) -> &str;
    /// Deterministic version/source digest.
    fn digest(&self) -> &str;
    /// Instantiate a new [`AgentLoop`] for the given [`LoopConfig`].
    fn create(&self, config: &LoopConfig, registry: &ExtensionRegistry) -> Box<dyn AgentLoop>;
}

/// Composable cross-cutting behavior wrapping any `Box<dyn AgentLoop>`.
pub trait LoopDecorator: Send + Sync {
    /// Unique decorator identifier.
    fn id(&self) -> &str;
    /// Deterministic version/source digest.
    fn digest(&self) -> &str;
    /// Wrap `inner` with this decorator's behavior.
    fn wrap(
        &self,
        inner: Box<dyn AgentLoop>,
        config: &LoopConfig,
        registry: &ExtensionRegistry,
    ) -> Box<dyn AgentLoop>;
}

/// Pure function projecting a [`SessionSlice`] and [`ArtifactStoreView`] into
/// a target model's input JSON.
pub trait ContextBuilder: Send + Sync {
    /// Unique builder identifier.
    fn id(&self) -> &str;
    /// Deterministic version/source digest.
    fn digest(&self) -> &str;
    /// Project session history and snapshotted artifacts into `ModelInput` JSON.
    fn build(
        &self,
        session: &SessionSlice,
        artifacts: &ArtifactStoreView,
        caps: &ModelCapabilities,
    ) -> Value;
}

/// Unified packaging contract implemented by `gestalt-std` and external Rust
/// extension crates alike (zero private kernel backdoors).
pub trait Extension: Send + Sync {
    /// Extension name.
    fn name(&self) -> &str;
    /// Register slots into [`ExtensionRegistry`].
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError`] if registration fails validation.
    fn register(&self, registry: &mut ExtensionRegistry) -> Result<(), ExtensionError>;
}

/// External IPC binding metadata stored in [`ExtensionRegistry`] when a slot
/// is hosted out-of-process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalBinding {
    /// Hosting external extension name.
    pub ext_name: String,
    /// Component slot ID.
    pub id: String,
    /// Deterministic digest reported by `extension/describe`.
    pub digest: String,
}

/// Resolved Phase execution stack produced by [`ExtensionRegistry::resolve_phase_stack`].
pub struct ResolvedPhaseStack {
    /// Optional preset ID if resolved from a named `Preset`.
    pub preset_id: Option<String>,
    /// Base loop identifier.
    pub base_loop_id: String,
    /// Base loop deterministic digest.
    pub loop_digest: String,
    /// Ordered decorator digests applied around the base loop.
    pub decorator_digests: Vec<String>,
    /// Active context builder identifier.
    pub builder_id: String,
    /// Active context builder digest.
    pub builder_digest: String,
    /// Effective Phase `LoopConfig`.
    pub config: LoopConfig,
    /// Instantiated in-process loop stack, or `None` if `external_loop` is set.
    pub in_process_loop: Option<Box<dyn AgentLoop>>,
    /// External IPC binding if the base loop runs out-of-process.
    pub external_loop: Option<ExternalBinding>,
    /// External IPC binding if the builder runs out-of-process.
    pub external_builder: Option<ExternalBinding>,
}

/// Typed slot registry populated by [`Extension::register`] and
/// [`ExtensionRegistry::register_external_describe`].
#[derive(Default)]
pub struct ExtensionRegistry {
    /// In-process `AgentLoopFactory` slots.
    pub loops: BTreeMap<String, Box<dyn AgentLoopFactory>>,
    /// In-process `LoopDecorator` slots.
    pub decorators: BTreeMap<String, Box<dyn LoopDecorator>>,
    /// Named `Preset` compositions.
    pub presets: BTreeMap<String, Preset>,
    /// In-process `ContextBuilder` slots.
    pub builders: BTreeMap<String, Box<dyn ContextBuilder>>,
    /// Additive `PolicyRule`s.
    pub policies: Vec<Box<dyn PolicyRule>>,
    /// Declarative tool descriptors.
    pub tools: BTreeMap<String, ToolSpec>,
    /// Declarative slash command descriptors.
    pub commands: BTreeMap<String, CommandSpec>,
    /// Declarative UI surface specs.
    pub ui: UISurface,
    /// Out-of-process loop bindings keyed by loop ID.
    pub external_loops: BTreeMap<String, ExternalBinding>,
    /// Out-of-process builder bindings keyed by builder ID.
    pub external_builders: BTreeMap<String, ExternalBinding>,
    /// Out-of-process decorator bindings keyed by decorator ID.
    pub external_decorators: BTreeMap<String, ExternalBinding>,
    /// Out-of-process policy rule bindings keyed by policy ID.
    pub external_policies: BTreeMap<String, ExternalBinding>,
}

impl ExtensionRegistry {
    /// Create an empty [`ExtensionRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the set of all registered tool names for `PolicyGate` validation.
    #[must_use]
    pub fn registered_tool_names(&self) -> BTreeSet<String> {
        self.tools.keys().cloned().collect()
    }

    /// Register an external out-of-process extension from its `extension/describe` payload.
    pub fn register_external_describe(&mut self, spec: ExternalExtensionSpec) {
        let ext = spec.name;
        for c in spec.loops {
            self.external_loops.insert(
                c.id.clone(),
                ExternalBinding {
                    ext_name: ext.clone(),
                    id: c.id,
                    digest: c.digest,
                },
            );
        }
        for c in spec.builders {
            self.external_builders.insert(
                c.id.clone(),
                ExternalBinding {
                    ext_name: ext.clone(),
                    id: c.id,
                    digest: c.digest,
                },
            );
        }
        for c in spec.decorators {
            self.external_decorators.insert(
                c.id.clone(),
                ExternalBinding {
                    ext_name: ext.clone(),
                    id: c.id,
                    digest: c.digest,
                },
            );
        }
        for c in spec.policies {
            self.external_policies.insert(
                c.id.clone(),
                ExternalBinding {
                    ext_name: ext.clone(),
                    id: c.id,
                    digest: c.digest,
                },
            );
        }
        self.presets.extend(spec.presets);
        for mut tool in spec.tools {
            tool.source_extension = Some(ext.clone());
            self.tools.insert(tool.name.clone(), tool);
        }
        for mut cmd in spec.commands {
            cmd.source_extension = Some(ext.clone());
            self.commands.insert(cmd.name.clone(), cmd);
        }
        self.ui.merge(spec.ui);
    }

    /// Unload all slots owned by an external extension prior to `/reload <ext>`.
    pub fn unload_external_extension(&mut self, ext_name: &str) {
        self.external_loops.retain(|_, v| v.ext_name != ext_name);
        self.external_builders.retain(|_, v| v.ext_name != ext_name);
        self.external_decorators
            .retain(|_, v| v.ext_name != ext_name);
        self.external_policies.retain(|_, v| v.ext_name != ext_name);
        self.tools
            .retain(|_, v| v.source_extension.as_deref() != Some(ext_name));
        self.commands
            .retain(|_, v| v.source_extension.as_deref() != Some(ext_name));
    }

    /// Dynamically resolve a `Preset` (or direct loop ID) into a decorated
    /// Phase execution stack.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::UnresolvedExtensionSlot`] if any referenced loop,
    /// decorator, or builder is missing, or [`KernelError::PolicyRelaxationForbidden`]
    /// if `config_override` attempts to widen `allowed_tools` or `max_spawn_depth`.
    pub fn resolve_phase_stack(
        &self,
        preset_or_loop: &str,
        extra_decorators: &[String],
        config_override: Option<&LoopConfig>,
    ) -> Result<ResolvedPhaseStack, KernelError> {
        let (preset_id, base_loop_id, mut decorator_ids, builder_id, base_config) =
            if let Some(preset) = self.presets.get(preset_or_loop) {
                (
                    Some(preset_or_loop.to_string()),
                    preset.base_loop.clone(),
                    preset.decorators.clone(),
                    preset.builder.clone(),
                    preset.config.clone(),
                )
            } else {
                let default_builder = self
                    .builders
                    .keys()
                    .next()
                    .or_else(|| self.external_builders.keys().next())
                    .cloned()
                    .unwrap_or_else(|| "default".to_string());
                (
                    None,
                    preset_or_loop.to_string(),
                    Vec::new(),
                    default_builder,
                    LoopConfig::default(),
                )
            };

        for extra in extra_decorators {
            if !decorator_ids.contains(extra) {
                decorator_ids.push(extra.clone());
            }
        }

        let effective_config = merge_loop_configs(&base_config, config_override)?;

        let (builder_digest, external_builder) = if let Some(b) = self.builders.get(&builder_id) {
            (b.digest().to_string(), None)
        } else if let Some(ext_b) = self.external_builders.get(&builder_id) {
            (ext_b.digest.clone(), Some(ext_b.clone()))
        } else {
            return Err(KernelError::UnresolvedExtensionSlot {
                slot: "builder".to_string(),
                name: builder_id,
            });
        };

        if let Some(factory) = self.loops.get(&base_loop_id) {
            let mut stacked = factory.create(&effective_config, self);
            let mut decorator_digests = Vec::with_capacity(decorator_ids.len());
            for dec_id in &decorator_ids {
                let dec = self.decorators.get(dec_id).ok_or_else(|| {
                    KernelError::UnresolvedExtensionSlot {
                        slot: "decorator".to_string(),
                        name: dec_id.clone(),
                    }
                })?;
                decorator_digests.push(dec.digest().to_string());
                stacked = dec.wrap(stacked, &effective_config, self);
            }
            return Ok(ResolvedPhaseStack {
                preset_id,
                base_loop_id,
                loop_digest: factory.digest().to_string(),
                decorator_digests,
                builder_id,
                builder_digest,
                config: effective_config,
                in_process_loop: Some(stacked),
                external_loop: None,
                external_builder,
            });
        }

        if let Some(ext_loop) = self.external_loops.get(&base_loop_id) {
            return Ok(ResolvedPhaseStack {
                preset_id,
                base_loop_id,
                loop_digest: ext_loop.digest.clone(),
                decorator_digests: Vec::new(),
                builder_id,
                builder_digest,
                config: effective_config,
                in_process_loop: None,
                external_loop: Some(ext_loop.clone()),
                external_builder,
            });
        }

        Err(KernelError::UnresolvedExtensionSlot {
            slot: "loop".to_string(),
            name: base_loop_id,
        })
    }
}

fn merge_loop_configs(
    base: &LoopConfig,
    override_cfg: Option<&LoopConfig>,
) -> Result<LoopConfig, KernelError> {
    let Some(ov) = override_cfg else {
        return Ok(base.clone());
    };
    let allowed_tools = match (&base.allowed_tools, &ov.allowed_tools) {
        (Some(b), Some(o)) => {
            if !o.iter().all(|t| b.contains(t)) {
                return Err(KernelError::PolicyRelaxationForbidden(
                    "phase override cannot add tools outside preset allowed_tools".to_string(),
                ));
            }
            Some(o.clone())
        }
        (Some(b), None) => Some(b.clone()),
        (None, Some(o)) => Some(o.clone()),
        (None, None) => None,
    };
    let max_spawn_depth = match (base.max_spawn_depth, ov.max_spawn_depth) {
        (Some(b), Some(o)) => {
            if o > b {
                return Err(KernelError::PolicyRelaxationForbidden(
                    "phase override cannot raise max_spawn_depth".to_string(),
                ));
            }
            Some(o)
        }
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (None, None) => None,
    };
    let budget = match (base.budget, ov.budget) {
        (Some(b), Some(o)) => {
            if !b.allows_narrowing(&o) {
                return Err(KernelError::PolicyRelaxationForbidden(
                    "phase override cannot relax preset budget".to_string(),
                ));
            }
            Some(b.tighten(&o))
        }
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (None, None) => None,
    };
    let params = if ov.params.is_null() {
        base.params.clone()
    } else {
        ov.params.clone()
    };
    Ok(LoopConfig {
        allowed_tools,
        max_spawn_depth,
        budget,
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyBuilder;
    impl ContextBuilder for DummyBuilder {
        fn id(&self) -> &str {
            "b1"
        }
        fn digest(&self) -> &str {
            "bd1"
        }
        fn build(
            &self,
            _session: &SessionSlice,
            _artifacts: &ArtifactStoreView,
            _caps: &ModelCapabilities,
        ) -> Value {
            Value::Null
        }
    }

    #[test]
    fn external_extension_register_and_unload() {
        let mut reg = ExtensionRegistry::new();
        reg.builders.insert("b1".into(), Box::new(DummyBuilder));
        reg.register_external_describe(ExternalExtensionSpec {
            name: "ext_py".into(),
            loops: vec![ExternalComponentSpec {
                id: "py_loop".into(),
                digest: "d1".into(),
            }],
            tools: vec![ToolSpec {
                name: "py_tool".into(),
                description: "desc".into(),
                parameters_schema: Value::Null,
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            }],
            ..Default::default()
        });
        assert!(reg.tools.contains_key("py_tool"));
        let stack = reg
            .resolve_phase_stack("py_loop", &[], None)
            .expect("resolve external");
        assert!(stack.external_loop.is_some());

        reg.unload_external_extension("ext_py");
        assert!(!reg.tools.contains_key("py_tool"));
        assert!(reg.resolve_phase_stack("py_loop", &[], None).is_err());
    }
}
