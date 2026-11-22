#![forbid(unsafe_code)]
//! Append-only, content-addressed `SessionTree` DAG and JSONL serializer.
//!
//! Operates strictly in memory on nodes and JSONL strings; `gestalt-runtime`
//! owns file handles, advisory `.lock` files, and atomic disk flushes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::effect::{EffectEnvelope, LoopConfig, PauseReason, StepResult, Usage};
use crate::error::KernelError;
use crate::sha256::compute_node_digest;
use crate::steering::{SteeringKind, SteeringSource};

/// Canonical `SessionTree` JSONL schema version supported by this kernel.
pub const SESSION_SCHEMA_VERSION: u32 = 1;

/// Completion status recorded on a `turn_end` node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    /// Turn finished normally via `LoopAction::Done`.
    Completed,
    /// Turn was cancelled preemptively via `SteeringKind::Abort`.
    Aborted,
    /// Turn terminated due to an unrecoverable error or policy block.
    Failed,
}

/// Branching operation recorded on a `branch_head` node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BranchAction {
    /// Operator or client forked a new conversation branch (`/fork`).
    Fork,
    /// Operator rewound active head to an earlier node (`/rewind`).
    Rewind,
    /// `Effect::Spawn` created an isolated child subagent branch.
    SpawnChild {
        /// Parent `Effect::Spawn` envelope ID that launched this branch.
        parent_effect_id: String,
    },
}

/// Typed representation of all 11 canonical `SessionTree` node kinds plus
/// user-space `custom:<ns>` extensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum NodePayload {
    /// Root node opening a session (`schema_version: 1`).
    SessionOpen {
        /// Schema version (must equal [`SESSION_SCHEMA_VERSION`]).
        schema_version: u32,
        /// Canonical workspace root path string.
        workspace_root: String,
        /// Creation timestamp in milliseconds.
        created_at_ms: u64,
    },
    /// User-facing Turn root recording prompt and blob-snapshotted artifacts.
    TurnStart {
        /// Turn identifier.
        turn_id: String,
        /// User prompt or trigger payload.
        prompt: Value,
        /// Map of `{ relative_artifact_path -> sha256_blob_digest }` for hermetic replay.
        artifact_snapshots: BTreeMap<String, String>,
    },
    /// Phase opening node recording exact loop/decorator/builder digests for replay.
    PhaseOpen {
        /// Phase identifier.
        phase_id: String,
        /// Optional preset ID if opened from a named `Preset`.
        preset_id: Option<String>,
        /// Base `AgentLoop` identifier.
        base_loop_id: String,
        /// Deterministic digest of the base `AgentLoop`.
        loop_digest: String,
        /// Ordered deterministic digests of applied `LoopDecorator`s.
        decorator_digests: Vec<String>,
        /// Active `ContextBuilder` identifier.
        builder_id: String,
        /// Deterministic digest of the active `ContextBuilder`.
        builder_digest: String,
        /// Ordered deterministic digests of applied `ContextLayer`s.
        #[serde(default)]
        builder_layer_digests: Vec<String>,
        /// Effective `LoopConfig` for this Phase.
        config: LoopConfig,
    },
    /// Phase sealing node emitted on `Effect::Switch` or Turn end.
    PhaseSeal {
        /// Sealed Phase identifier.
        phase_id: String,
        /// Typed handoff JSON passed to the next Phase (or `null`).
        handoff: Value,
    },
    /// Recorded `EffectEnvelope` (plus optional external worker `state_token`).
    Effect {
        /// Dispatched or evaluated effect envelope.
        #[serde(flatten)]
        envelope: EffectEnvelope,
        /// Opaque external worker state token persisted for crash recovery and replay.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_token: Option<Value>,
    },
    /// Recorded `StepResult` paired 1:1 with an `EffectEnvelope`.
    StepResult(StepResult),
    /// Durable `SteeringSignal` logged immediately on enqueue.
    Steering {
        /// Signal ID.
        id: String,
        /// Steering kind (`Steer`, `UrgentSteer`, `FollowUp`, `Abort`).
        kind: SteeringKind,
        /// Signal instruction or cancellation reason.
        content: Value,
        /// Entity that emitted the signal.
        source: SteeringSource,
    },
    /// Recorded `Effect::Pause` suspension.
    Pause {
        /// Unique pause dialog identifier.
        pause_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// Semantic pause reason.
        reason: PauseReason,
        /// Structured dialog payload.
        payload: Value,
    },
    /// Recorded client resolution for an earlier `Pause` node.
    PauseResolved {
        /// Correlated pause identifier.
        pause_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// Resolution payload returned by the client.
        resolution: Value,
    },
    /// Terminal node closing a Turn with status, output, and total usage.
    TurnEnd {
        /// Turn identifier.
        turn_id: String,
        /// Terminal Turn status.
        status: TurnStatus,
        /// Final Turn output JSON.
        output: Value,
        /// Total accumulated Turn usage.
        usage: Usage,
    },
    /// Branching audit node for `/fork`, `/rewind`, or `Effect::Spawn`.
    BranchHead {
        /// Branch identifier.
        branch_id: String,
        /// Node ID this branch points to.
        target_node_id: String,
        /// Specific branching operation.
        action: BranchAction,
    },
    /// User-space namespaced node (serialized as `"custom:<namespace>"`).
    Custom {
        /// Namespace suffix (e.g., `"std:compaction"`).
        namespace: String,
        /// Arbitrary user-space JSON payload.
        data: Value,
    },
}

impl NodePayload {
    /// Convert this typed payload into its canonical `(kind_string, payload_json)` pair.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Serialization`] if serialization fails.
    pub fn to_kind_and_value(&self) -> Result<(String, Value), KernelError> {
        match self {
            Self::Custom { namespace, data } => {
                let kind = if namespace.starts_with("custom:") {
                    namespace.clone()
                } else {
                    format!("custom:{namespace}")
                };
                Ok((kind, data.clone()))
            }
            Self::StepResult(result) => {
                Ok(("step_result".to_string(), serde_json::to_value(result)?))
            }
            other => {
                let mut val = serde_json::to_value(other)?;
                let kind = val
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| KernelError::Serialization("missing node kind tag".to_string()))?
                    .to_string();
                let payload = val
                    .as_object_mut()
                    .and_then(|obj| obj.remove("payload"))
                    .unwrap_or(Value::Null);
                Ok((kind, payload))
            }
        }
    }

    /// Reconstruct a typed [`NodePayload`] from a raw `(kind, payload)` pair.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Serialization`] if the payload does not match the kind schema.
    pub fn from_kind_and_value(kind: &str, payload: &Value) -> Result<Self, KernelError> {
        if let Some(ns) = kind.strip_prefix("custom:") {
            return Ok(Self::Custom {
                namespace: ns.to_string(),
                data: payload.clone(),
            });
        }
        if kind == "step_result" {
            let res: StepResult = serde_json::from_value(payload.clone())?;
            return Ok(Self::StepResult(res));
        }
        let tagged = serde_json::json!({ "kind": kind, "payload": payload });
        Ok(serde_json::from_value(tagged)?)
    }
}

/// Canonical JSONL envelope for a single node in [`SessionTree`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionNode {
    /// Unique node identifier.
    pub id: String,
    /// Parent node ID (`None` only for the root `session_open` node).
    pub parent_id: Option<String>,
    /// Active Turn ID, if inside a Turn.
    pub turn_id: Option<String>,
    /// Active Phase ID, if inside a Phase.
    pub phase_id: Option<String>,
    /// Deterministic timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Canonical or `custom:<ns>` node kind string.
    pub kind: String,
    /// Structured JSON payload.
    pub payload: Value,
    /// Content-addressed SHA-256 digest over `(parent_id, kind, canonical_json(payload))`.
    pub digest: String,
}

impl SessionNode {
    /// Decode the node's raw `(kind, payload)` into a typed [`NodePayload`].
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Serialization`] if the payload schema is invalid.
    pub fn decode_payload(&self) -> Result<NodePayload, KernelError> {
        NodePayload::from_kind_and_value(&self.kind, &self.payload)
    }

    /// Serialize this node as a single compact JSONL line (without trailing newline).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Serialization`] if serialization fails.
    pub fn to_jsonl_line(&self) -> Result<String, KernelError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Read-only chronological root-to-head view of the active branch in `SessionTree`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionSlice {
    nodes: Vec<SessionNode>,
}

impl SessionSlice {
    /// Construct a [`SessionSlice`] from an ordered root-to-head vector of nodes.
    #[must_use]
    pub fn from_nodes(nodes: Vec<SessionNode>) -> Self {
        Self { nodes }
    }

    /// Return all nodes on the branch from root (`session_open`) to the active head.
    #[must_use]
    pub fn nodes(&self) -> &[SessionNode] {
        &self.nodes
    }

    /// Return the active head node, if the slice is non-empty.
    #[must_use]
    pub fn head(&self) -> Option<&SessionNode> {
        self.nodes.last()
    }

    /// Return the number of nodes on this branch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Return `true` if the slice contains no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(crate) fn push_custom_preview(
        &mut self,
        turn_id: &str,
        phase_id: &str,
        now_ms: u64,
        seq: usize,
        namespace: &str,
        data: Value,
    ) {
        let kind = if namespace.starts_with("custom:") {
            namespace.to_string()
        } else {
            format!("custom:{namespace}")
        };
        let parent_id = self.nodes.last().map(|n| n.id.clone());
        self.nodes.push(SessionNode {
            id: format!("custom_preview_{seq}"),
            parent_id,
            turn_id: Some(turn_id.to_string()),
            phase_id: Some(phase_id.to_string()),
            timestamp_ms: now_ms,
            kind,
            payload: data,
            digest: String::new(),
        });
    }
}

/// Content-addressed, append-only DAG storing full session history and branches.
#[derive(Debug, Clone, Default)]
pub struct SessionTree {
    nodes: Vec<SessionNode>,
    index: BTreeMap<String, usize>,
    head_id: Option<String>,
    branches: BTreeMap<String, String>,
}

impl SessionTree {
    /// Create an empty [`SessionTree`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the active branch head node ID.
    #[must_use]
    pub fn head_id(&self) -> Option<&str> {
        self.head_id.as_deref()
    }

    /// Set the active head pointer to an existing node ID (e.g., switching branches).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::UnknownNode`] if `node_id` is not in the tree.
    pub fn set_head(&mut self, node_id: &str) -> Result<(), KernelError> {
        if !self.index.contains_key(node_id) {
            return Err(KernelError::UnknownNode(node_id.to_string()));
        }
        self.head_id = Some(node_id.to_string());
        Ok(())
    }

    /// Look up a node by its `id`.
    #[must_use]
    pub fn get(&self, node_id: &str) -> Option<&SessionNode> {
        self.index.get(node_id).and_then(|&idx| self.nodes.get(idx))
    }

    /// Return all nodes in insertion order.
    #[must_use]
    pub fn all_nodes(&self) -> &[SessionNode] {
        &self.nodes
    }

    /// Return the registered branch heads (`branch_id -> head_node_id`).
    #[must_use]
    pub fn branches(&self) -> &BTreeMap<String, String> {
        &self.branches
    }

    /// Append a typed [`NodePayload`] as a child of the current active head.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::DuplicateNodeId`] if `node_id` already exists or
    /// [`KernelError::Serialization`] if payload encoding fails.
    pub fn append(
        &mut self,
        node_id: String,
        turn_id: Option<String>,
        phase_id: Option<String>,
        timestamp_ms: u64,
        payload: &NodePayload,
    ) -> Result<&SessionNode, KernelError> {
        let parent = self.head_id.clone();
        self.append_with_parent(node_id, parent, turn_id, phase_id, timestamp_ms, payload)
    }

    /// Append a typed [`NodePayload`] with an explicit `parent_id` (used for
    /// `/rewind` and isolated `Spawn` child branches).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::DuplicateNodeId`] if `node_id` already exists,
    /// [`KernelError::UnknownNode`] if `parent_id` is `Some` and missing, or
    /// [`KernelError::Serialization`] if payload encoding fails.
    pub fn append_with_parent(
        &mut self,
        node_id: String,
        parent_id: Option<String>,
        turn_id: Option<String>,
        phase_id: Option<String>,
        timestamp_ms: u64,
        payload: &NodePayload,
    ) -> Result<&SessionNode, KernelError> {
        if self.index.contains_key(&node_id) {
            return Err(KernelError::DuplicateNodeId(node_id));
        }
        if let Some(ref pid) = parent_id
            && !self.index.contains_key(pid)
        {
            return Err(KernelError::UnknownNode(pid.clone()));
        }
        let (kind, raw_payload) = payload.to_kind_and_value()?;
        let digest = compute_node_digest(parent_id.as_deref(), &kind, &raw_payload);
        let node = SessionNode {
            id: node_id.clone(),
            parent_id,
            turn_id,
            phase_id,
            timestamp_ms,
            kind,
            payload: raw_payload,
            digest,
        };
        let idx = self.nodes.len();
        self.nodes.push(node);
        self.index.insert(node_id.clone(), idx);
        self.head_id = Some(node_id);
        Ok(&self.nodes[idx])
    }

    /// Fork a new branch from `from_node_id` (or current head if `None`) and
    /// make it the active head.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::UnknownNode`] if no target node exists.
    pub fn fork(
        &mut self,
        branch_id: String,
        from_node_id: Option<&str>,
        audit_node_id: String,
        timestamp_ms: u64,
    ) -> Result<&SessionNode, KernelError> {
        let target = from_node_id
            .map(ToString::to_string)
            .or_else(|| self.head_id.clone())
            .ok_or_else(|| KernelError::UnknownNode("empty tree".to_string()))?;
        if !self.index.contains_key(&target) {
            return Err(KernelError::UnknownNode(target));
        }
        let payload = NodePayload::BranchHead {
            branch_id: branch_id.clone(),
            target_node_id: target.clone(),
            action: BranchAction::Fork,
        };
        let new_head = {
            let node = self.append_with_parent(
                audit_node_id.clone(),
                Some(target),
                None,
                None,
                timestamp_ms,
                &payload,
            )?;
            node.id.clone()
        };
        self.branches.insert(branch_id, new_head);
        self.get(&audit_node_id)
            .ok_or(KernelError::UnknownNode(audit_node_id))
    }

    /// Rewind the active head to `target_node_id` by appending a `BranchHead`
    /// audit node rooted at `target_node_id` without mutating prior history.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::UnknownNode`] if `target_node_id` is not in the tree.
    pub fn rewind(
        &mut self,
        branch_id: String,
        target_node_id: &str,
        audit_node_id: String,
        timestamp_ms: u64,
    ) -> Result<&SessionNode, KernelError> {
        if !self.index.contains_key(target_node_id) {
            return Err(KernelError::UnknownNode(target_node_id.to_string()));
        }
        let payload = NodePayload::BranchHead {
            branch_id: branch_id.clone(),
            target_node_id: target_node_id.to_string(),
            action: BranchAction::Rewind,
        };
        let new_head = {
            let node = self.append_with_parent(
                audit_node_id.clone(),
                Some(target_node_id.to_string()),
                None,
                None,
                timestamp_ms,
                &payload,
            )?;
            node.id.clone()
        };
        self.branches.insert(branch_id, new_head);
        self.get(&audit_node_id)
            .ok_or(KernelError::UnknownNode(audit_node_id))
    }

    /// Root an isolated child subagent branch at `parent_node_id` without
    /// permanently moving the caller's branch head once restored.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::UnknownNode`] if `parent_node_id` is missing.
    pub fn create_spawn_branch(
        &mut self,
        branch_id: String,
        parent_node_id: &str,
        parent_effect_id: String,
        audit_node_id: String,
        timestamp_ms: u64,
    ) -> Result<&SessionNode, KernelError> {
        if !self.index.contains_key(parent_node_id) {
            return Err(KernelError::UnknownNode(parent_node_id.to_string()));
        }
        let payload = NodePayload::BranchHead {
            branch_id: branch_id.clone(),
            target_node_id: parent_node_id.to_string(),
            action: BranchAction::SpawnChild { parent_effect_id },
        };
        let head = {
            let node = self.append_with_parent(
                audit_node_id.clone(),
                Some(parent_node_id.to_string()),
                None,
                None,
                timestamp_ms,
                &payload,
            )?;
            node.id.clone()
        };
        self.branches.insert(branch_id, head);
        self.get(&audit_node_id)
            .ok_or(KernelError::UnknownNode(audit_node_id))
    }

    /// Build a chronological root-to-head [`SessionSlice`] for `head_id`
    /// (or the active head if `None`).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::UnknownNode`] if the target head or an ancestor link is missing.
    pub fn slice_to(&self, head_id: Option<&str>) -> Result<SessionSlice, KernelError> {
        let Some(start) = head_id.or(self.head_id.as_deref()) else {
            return Ok(SessionSlice::default());
        };
        let mut chain = Vec::new();
        let mut cursor = Some(start.to_string());
        while let Some(ref id) = cursor {
            let node = self
                .get(id)
                .ok_or_else(|| KernelError::UnknownNode(id.clone()))?;
            chain.push(node.clone());
            cursor = node.parent_id.clone();
        }
        chain.reverse();
        Ok(SessionSlice::from_nodes(chain))
    }

    /// Parse a JSONL string into a verified [`SessionTree`].
    ///
    /// If the very last non-empty line is truncated/malformed JSON (indicating an
    /// abrupt crash mid-flush), that trailing line is discarded and a warning
    /// string is returned alongside the intact prefix tree. Any corruption in an
    /// interior line or SHA-256 digest mismatch fails closed with an error.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::CorruptSessionTree`] on interior parse failures,
    /// missing parent links, unsupported schema versions, or SHA-256 mismatches.
    pub fn from_jsonl_str(input: &str) -> Result<(Self, Option<String>), KernelError> {
        let raw_lines: Vec<(usize, &str)> = input
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.trim()))
            .filter(|(_, l)| !l.is_empty())
            .collect();

        let mut tree = Self::new();
        let mut warning = None;
        let total = raw_lines.len();

        for (pos, (line_no, line)) in raw_lines.into_iter().enumerate() {
            let is_last = pos + 1 == total;
            let node: SessionNode = match serde_json::from_str(line) {
                Ok(n) => n,
                Err(err) if is_last => {
                    warning = Some(format!(
                        "discarded truncated trailing JSONL line {line_no}: {err}"
                    ));
                    break;
                }
                Err(err) => {
                    return Err(KernelError::CorruptSessionTree {
                        node_id: None,
                        line: Some(line_no),
                        reason: format!("invalid JSONL: {err}"),
                    });
                }
            };

            if tree.index.contains_key(&node.id) {
                return Err(KernelError::CorruptSessionTree {
                    node_id: Some(node.id),
                    line: Some(line_no),
                    reason: "duplicate node id".to_string(),
                });
            }
            if let Some(ref pid) = node.parent_id
                && !tree.index.contains_key(pid)
            {
                return Err(KernelError::CorruptSessionTree {
                    node_id: Some(node.id),
                    line: Some(line_no),
                    reason: format!("missing parent_id '{pid}'"),
                });
            }
            if node.kind == "session_open"
                && node.payload.get("schema_version").and_then(Value::as_u64)
                    != Some(u64::from(SESSION_SCHEMA_VERSION))
            {
                return Err(KernelError::CorruptSessionTree {
                    node_id: Some(node.id),
                    line: Some(line_no),
                    reason: "unsupported session_open schema_version".to_string(),
                });
            }
            let expected_digest =
                compute_node_digest(node.parent_id.as_deref(), &node.kind, &node.payload);
            if node.digest != expected_digest {
                return Err(KernelError::CorruptSessionTree {
                    node_id: Some(node.id),
                    line: Some(line_no),
                    reason: format!(
                        "sha256 digest mismatch (expected {expected_digest}, got {})",
                        node.digest
                    ),
                });
            }

            let idx = tree.nodes.len();
            let id = node.id.clone();
            tree.nodes.push(node);
            tree.index.insert(id.clone(), idx);
            tree.head_id = Some(id);
        }

        Ok((tree, warning))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_tree_rewind_and_truncated_tail_recovery() {
        let mut tree = SessionTree::new();
        tree.append(
            "n0".into(),
            None,
            None,
            1,
            &NodePayload::SessionOpen {
                schema_version: SESSION_SCHEMA_VERSION,
                workspace_root: "/repo".into(),
                created_at_ms: 1,
            },
        )
        .expect("n0");
        tree.append(
            "n1".into(),
            Some("t1".into()),
            None,
            2,
            &NodePayload::TurnStart {
                turn_id: "t1".into(),
                prompt: json!("hi"),
                artifact_snapshots: BTreeMap::new(),
            },
        )
        .expect("n1");
        tree.append(
            "n2".into(),
            Some("t1".into()),
            None,
            3,
            &NodePayload::Custom {
                namespace: "std:compaction".into(),
                data: json!({"summary": "ok"}),
            },
        )
        .expect("n2");

        tree.rewind("main".into(), "n1", "n3".into(), 4)
            .expect("rewind");
        let slice = tree.slice_to(None).expect("slice");
        let ids: Vec<&str> = slice.nodes().iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["n0", "n1", "n3"]);

        let mut jsonl = tree
            .all_nodes()
            .iter()
            .map(|n| n.to_jsonl_line().expect("line"))
            .collect::<Vec<_>>()
            .join("\n");
        jsonl.push_str("\n{\"id\":\"n_partial\",\"parent_id\":");

        let (recovered, warn) = SessionTree::from_jsonl_str(&jsonl).expect("recover");
        assert!(warn.is_some());
        assert_eq!(recovered.all_nodes().len(), 4);
    }
}
