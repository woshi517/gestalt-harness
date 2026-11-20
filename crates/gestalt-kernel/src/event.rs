#![forbid(unsafe_code)]
//! Protocol-neutral `EventStream` (`RunEvent`) and Sans-I/O `DriverDirective` types.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::effect::{EffectEnvelope, LoopConfig, PauseReason, StepResult, Usage};
use crate::extension::{ArtifactEntry, LoopAction, ModelCapabilities};
use crate::tree::SessionNode;

/// Typed notification emitted by the kernel or runtime driver for TUI, GUI,
/// JSON-RPC, and ACP clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEvent {
    /// Canonical event: a new node was appended to `SessionTree`.
    NodeAppended {
        /// Newly appended node ID.
        node_id: String,
        /// Canonical or `custom:<ns>` node kind.
        kind: String,
    },
    /// Canonical event: execution switched to a new Phase.
    PhaseSwitched {
        /// Newly opened Phase ID.
        phase_id: String,
        /// Optional `Preset` ID used to open this Phase.
        preset_id: Option<String>,
        /// Base `AgentLoop` identifier.
        base_loop_id: String,
    },
    /// Canonical event: `PolicyGate` denied an effect before dispatch.
    PolicyDenied {
        /// Denied `effect_id`.
        effect_id: String,
        /// Denial reason.
        reason: String,
    },
    /// Canonical event: execution paused awaiting client/operator input.
    Paused {
        /// Unique pause identifier.
        pause_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// Semantic pause reason.
        reason: PauseReason,
        /// Dialog payload.
        payload: Value,
    },
    /// Canonical event: Turn finished cleanly.
    TurnCompleted {
        /// Completed Turn ID.
        turn_id: String,
        /// Final Turn output JSON.
        output: Value,
        /// Accumulated Turn usage.
        usage: Usage,
    },
    /// Canonical event: Turn aborted cleanly.
    TurnAborted {
        /// Aborted Turn ID.
        turn_id: String,
        /// Cancellation reason.
        reason: String,
    },
    /// Transient event (not persisted to `SessionTree`): streaming chunk from a
    /// provider or tool worker.
    StreamDelta {
        /// Active Turn ID.
        turn_id: String,
        /// Active Phase ID.
        phase_id: String,
        /// In-flight `effect_id`.
        effect_id: String,
        /// Stream channel (e.g., `"text"`, `"thought"`, `"tool_output"`).
        channel: String,
        /// Streaming delta payload.
        payload: Value,
    },
    /// Transient event (not persisted to `SessionTree`): structured UI card
    /// update targeting a registered `UISurface` renderer.
    ToolUi {
        /// Active Turn ID.
        turn_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// Registered `UIRendererSpec` identifier.
        renderer: String,
        /// Structured renderer data payload.
        data: Value,
    },
}

impl RunEvent {
    /// Return `true` for transient streaming/UI events that MUST NOT be
    /// persisted as individual `SessionTree` JSONL nodes.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::StreamDelta { .. } | Self::ToolUi { .. })
    }
}

/// In-memory FIFO sink collecting [`RunEvent`]s emitted during kernel polling.
#[derive(Debug, Clone, Default)]
pub struct EventStream {
    queue: VecDeque<RunEvent>,
}

impl EventStream {
    /// Create an empty [`EventStream`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a [`RunEvent`] onto the stream.
    pub fn emit(&mut self, event: RunEvent) {
        self.queue.push_back(event);
    }

    /// Drain all queued [`RunEvent`]s in FIFO order.
    pub fn drain(&mut self) -> Vec<RunEvent> {
        self.queue.drain(..).collect()
    }

    /// Return the number of buffered events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Return `true` if no events are currently buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// Directive yielded by `KernelRuntime::next_action` instructing
/// `gestalt-runtime`'s async driver what I/O or IPC action to execute next.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverDirective {
    /// Execute one or more policy-approved effects in parallel.
    DispatchEffects {
        /// Approved effect envelopes to run concurrently.
        effects: Vec<EffectEnvelope>,
    },
    /// Execute a mandatory verifier effect before `PolicyGate` decides on `pending`.
    VerifyEffect {
        /// Policy rule ID awaiting verification.
        rule_id: String,
        /// Verifier effect (e.g., `Effect::Model { kind: Decision, .. }`) to run first.
        verifier: Box<EffectEnvelope>,
        /// Pending effect held until `PolicyRule::evaluate_verification` runs.
        pending: Box<EffectEnvelope>,
    },
    /// Defer an out-of-process `loop/step` call to `gestalt-runtime`'s IPC host.
    InvokeExternalLoop {
        /// Registered external extension name.
        ext_name: String,
        /// External loop ID.
        loop_id: String,
        /// Active Phase ID.
        phase_id: String,
        /// Active Turn ID.
        turn_id: String,
        /// Typed handoff JSON for the active Phase.
        handoff: Value,
        /// Effective `LoopConfig`.
        config: LoopConfig,
        /// Opaque external worker state token from the prior step.
        state_token: Option<Value>,
        /// Prior step results (or `None` on first step).
        results: Option<Vec<StepResult>>,
    },
    /// Defer an out-of-process `builder/build` call to `gestalt-runtime`'s IPC host.
    InvokeExternalBuilder {
        /// Registered external extension name.
        ext_name: String,
        /// External builder ID.
        builder_id: String,
        /// Active branch nodes from root to head.
        session_slice: Vec<SessionNode>,
        /// Snapshotted or live artifact entries.
        artifacts: BTreeMap<String, ArtifactEntry>,
        /// Target model capabilities.
        caps: ModelCapabilities,
    },
    /// Suspend execution until the client resolves the `Pause` dialog.
    WaitForPause {
        /// Unique pause identifier.
        pause_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// Semantic pause reason.
        reason: PauseReason,
        /// Dialog payload.
        payload: Value,
    },
    /// Active Turn completed cleanly.
    TurnComplete {
        /// Completed Turn ID.
        turn_id: String,
        /// Final Turn output JSON.
        output: Value,
        /// Accumulated Turn usage.
        usage: Usage,
    },
    /// Active Turn was aborted cleanly.
    TurnAborted {
        /// Aborted Turn ID.
        turn_id: String,
        /// Cancellation reason.
        reason: String,
    },
}

/// Response fed back into `KernelRuntime::next_action` by `gestalt-runtime`.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverResponse {
    /// Initial poll immediately after `start_turn` (no prior I/O results yet).
    Poll,
    /// Results for a completed `DriverDirective::DispatchEffects` batch.
    EffectResults(Vec<StepResult>),
    /// Outcome of `DriverDirective::VerifyEffect`.
    VerificationResult {
        /// Rule ID that requested verification.
        rule_id: String,
        /// `effect_id` of the held pending effect.
        pending_effect_id: String,
        /// Verifier's `StepResult`.
        verifier_result: StepResult,
    },
    /// Outcome of `DriverDirective::InvokeExternalLoop`.
    ExternalLoopOutput {
        /// `LoopAction` returned by the external worker.
        action: LoopAction,
        /// Updated opaque state token to persist on `SessionTree`.
        state_token: Option<Value>,
    },
    /// Outcome of `DriverDirective::InvokeExternalBuilder`.
    ExternalBuilderOutput {
        /// Constructed `ModelInput` JSON returned by the external builder.
        model_input: Value,
    },
    /// Client resolution for `DriverDirective::WaitForPause`.
    PauseResolution {
        /// Correlated pause identifier.
        pause_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// User or system resolution payload.
        resolution: Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transient_events_are_distinguished_from_canonical_events() {
        let delta = RunEvent::StreamDelta {
            turn_id: "t1".into(),
            phase_id: "p1".into(),
            effect_id: "e1".into(),
            channel: "thought".into(),
            payload: json!({"text": "thinking"}),
        };
        let node = RunEvent::NodeAppended {
            node_id: "n1".into(),
            kind: "effect".into(),
        };
        assert!(delta.is_transient());
        assert!(!node.is_transient());
    }
}
