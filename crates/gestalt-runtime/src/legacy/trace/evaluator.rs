use crate::golden::GoldenTrace;
use crate::EventEnvelope;
use async_trait::async_trait;
use gestalt_core::error::HarnessError;
use gestalt_core::event::AgentEvent;
use gestalt_core::hook::SessionHook;
use gestalt_core::session::Session;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub status: EvalStatus,
    pub score: Option<f64>,
    pub feedback: Option<String>,
}

#[async_trait]
pub trait TraceEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        trace: &[EventEnvelope],
        golden: &GoldenTrace,
    ) -> Result<EvalResult, HarnessError>;
}

pub struct NoopTraceEvaluator;

#[async_trait]
impl TraceEvaluator for NoopTraceEvaluator {
    async fn evaluate(
        &self,
        _trace: &[EventEnvelope],
        _golden: &GoldenTrace,
    ) -> Result<EvalResult, HarnessError> {
        Ok(EvalResult {
            status: EvalStatus::Skipped,
            score: None,
            feedback: None,
        })
    }
}

pub struct EvaluatorHook {
    pub evaluator: Arc<dyn TraceEvaluator>,
    pub golden: Option<GoldenTrace>,
    pub flush_trigger: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl EvaluatorHook {
    pub fn new(evaluator: Arc<dyn TraceEvaluator>, golden: Option<GoldenTrace>) -> Self {
        Self {
            evaluator,
            golden,
            flush_trigger: None,
        }
    }

    pub fn with_flush_trigger(mut self, trigger: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.flush_trigger = Some(trigger);
        self
    }
}

#[async_trait]
impl SessionHook for EvaluatorHook {
    async fn on_session_start(
        &self,
        _session: &Session,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        Ok(vec![])
    }

    async fn on_session_end(
        &self,
        session: &Session,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        if let Some(ref trigger) = self.flush_trigger {
            trigger();
        }

        let Some(ref golden) = self.golden else {
            return Ok(vec![]);
        };

        let trace_path = if let Some(ref art_dir) = session.tool_ctx.artifact_dir {
            art_dir.parent().map(|p| p.join("trace.jsonl"))
        } else {
            None
        };

        if let Some(path) = trace_path {
            if path.exists() {
                if let Ok(trace) = crate::read_trace(path) {
                    if let Ok(eval_res) = self.evaluator.evaluate(&trace, golden).await {
                        let status = match eval_res.status {
                            EvalStatus::Passed => gestalt_core::event::VerificationStatus::Passed,
                            EvalStatus::Failed => gestalt_core::event::VerificationStatus::Failed,
                            EvalStatus::Skipped => gestalt_core::event::VerificationStatus::Skipped,
                        };
                        return Ok(vec![AgentEvent::VerificationResult {
                            status,
                            checks: 1,
                            failed: if eval_res.status == EvalStatus::Failed {
                                1
                            } else {
                                0
                            },
                            report: eval_res
                                .feedback
                                .or_else(|| Some(format!("Score: {:?}", eval_res.score))),
                            findings: None,
                        }]);
                    }
                }
            }
        }
        Ok(vec![])
    }
}
