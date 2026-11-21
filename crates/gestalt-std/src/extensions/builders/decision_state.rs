#![forbid(unsafe_code)]
//! System One [`ContextBuilder`] projecting `SessionSlice` into `DecisionInput`
//! (`{ state, choices, rubric }`) without constructing chat `messages[]`.

use gestalt_kernel::{
    ArtifactStoreView, ContextBuilder, ModelCapabilities, NodePayload, SessionSlice,
};
use serde_json::{Value, json};

use crate::extensions::builders::{AGENTS_MD_PATH, inspect_slice_metadata};
use crate::extensions::{DecisionChoice, DecisionInput};

/// Pure [`ContextBuilder`] (`"decision_state"`) for `ModelKind::Decision` routers.
#[derive(Debug, Clone, Default)]
pub struct DecisionStateBuilder;

impl ContextBuilder for DecisionStateBuilder {
    fn id(&self) -> &str {
        "decision_state"
    }

    fn digest(&self) -> &str {
        "std_decision_state_builder_v1"
    }

    fn build(
        &self,
        session: &SessionSlice,
        artifacts: &ArtifactStoreView,
        _caps: &ModelCapabilities,
    ) -> Value {
        let meta = inspect_slice_metadata(session);
        let mut latest_prompt = Value::Null;
        let mut recent_events = Vec::new();

        for node in session.nodes() {
            if let Ok(payload) = node.decode_payload() {
                match payload {
                    NodePayload::TurnStart { prompt, .. } => {
                        latest_prompt = prompt;
                    }
                    NodePayload::StepResult(sr) => {
                        if let Ok(v) = serde_json::to_value(sr) {
                            recent_events.push(v);
                        }
                    }
                    NodePayload::Steering { content, .. } => {
                        recent_events.push(json!({ "steering": content }));
                    }
                    _ => {}
                }
            }
        }
        if recent_events.len() > 6 {
            let tail_start = recent_events.len() - 6;
            recent_events = recent_events.split_off(tail_start);
        }

        let mut choices: Vec<DecisionChoice> = meta
            .active_config
            .as_ref()
            .and_then(|c| c.params.get("choices"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if choices.is_empty()
            && let Some(routes) = meta
                .active_config
                .as_ref()
                .and_then(|c| c.params.get("routes"))
                .and_then(Value::as_object)
        {
            for (id, spec) in routes {
                let desc = spec
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("configured route")
                    .to_string();
                choices.push(DecisionChoice {
                    id: id.clone(),
                    description: desc,
                });
            }
        }

        let rubric = meta
            .active_config
            .as_ref()
            .and_then(|c| c.params.get("rubric"))
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let state = json!({
            "prompt": latest_prompt,
            "handoff": meta.active_handoff,
            "has_agents_md": artifacts.get(AGENTS_MD_PATH).is_some(),
            "recent_results": recent_events,
        });

        let input = DecisionInput {
            state,
            choices,
            rubric,
        };
        serde_json::to_value(input).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{LoopConfig, SESSION_SCHEMA_VERSION, SessionTree};
    use std::collections::BTreeMap;

    #[test]
    fn decision_state_builder_emits_typed_state_without_chat_messages() {
        let builder = DecisionStateBuilder;
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
            Some("p1".into()),
            2,
            &NodePayload::TurnStart {
                turn_id: "t1".into(),
                prompt: json!("run cargo test"),
                artifact_snapshots: BTreeMap::new(),
            },
        )
        .expect("n1");
        tree.append(
            "n2".into(),
            Some("t1".into()),
            Some("p1".into()),
            3,
            &NodePayload::PhaseOpen {
                phase_id: "p1".into(),
                preset_id: Some("triage".into()),
                base_loop_id: "decision_router".into(),
                loop_digest: "dr1".into(),
                decorator_digests: vec![],
                builder_id: "decision_state".into(),
                builder_digest: builder.digest().into(),
                config: LoopConfig {
                    params: json!({
                        "rubric": "Route quick commands directly",
                        "routes": {
                            "run_tests": { "action": "tool", "name": "bash", "description": "Execute test suite" }
                        }
                    }),
                    ..LoopConfig::default()
                },
            },
        )
        .expect("n2");

        let val = builder.build(
            &tree.slice_to(None).expect("slice"),
            &ArtifactStoreView::default(),
            &ModelCapabilities::default(),
        );
        assert!(val.get("messages").is_none());
        assert_eq!(val["state"]["prompt"], "run cargo test");
        assert_eq!(val["choices"][0]["id"], "run_tests");
        assert_eq!(val["rubric"], "Route quick commands directly");
    }
}
