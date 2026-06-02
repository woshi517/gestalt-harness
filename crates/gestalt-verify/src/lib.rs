use async_trait::async_trait;
use gestalt_core::error::HarnessError;
pub use gestalt_core::event::{
    AgentEvent, FindingSeverity, VerificationFinding, VerificationStatus,
};
use std::fmt::Write as _;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub mod verifiers;
pub use verifiers::{
    CommandVerifier, FileExistsVerifier, MarkdownStructureVerifier, NoSecretsVerifier,
    PatchAppliesVerifier,
};

#[derive(Debug, Clone)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct VerifyContext {
    pub workspace_root: PathBuf,
    pub run_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VerifyResult {
    pub status: VerificationStatus,
    pub findings: Vec<VerificationFinding>,
    pub report: Option<String>,
}

#[async_trait]
pub trait Verifier: Send + Sync {
    fn name(&self) -> &str;
    fn applies_to(&self, artifact: &ArtifactRef, ctx: &VerifyContext) -> bool;
    async fn verify(
        &self,
        artifact: &ArtifactRef,
        ctx: &VerifyContext,
    ) -> Result<VerifyResult, HarnessError>;
}

pub struct VerifierRegistry {
    verifiers: Vec<Box<dyn Verifier>>,
}

impl Default for VerifierRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl VerifierRegistry {
    pub fn new() -> Self {
        Self {
            verifiers: Vec::new(),
        }
    }

    pub fn register(&mut self, verifier: Box<dyn Verifier>) {
        self.verifiers.push(verifier);
    }

    pub async fn run_all(
        &self,
        artifact: &ArtifactRef,
        ctx: &VerifyContext,
    ) -> Vec<(String, VerifyResult)> {
        let mut results = Vec::new();
        for verifier in &self.verifiers {
            if verifier.applies_to(artifact, ctx) {
                match verifier.verify(artifact, ctx).await {
                    Ok(res) => results.push((verifier.name().to_string(), res)),
                    Err(err) => {
                        results.push((
                            verifier.name().to_string(),
                            VerifyResult {
                                status: VerificationStatus::Failed,
                                findings: vec![VerificationFinding {
                                    severity: FindingSeverity::Error,
                                    message: format!("Verifier error: {err}"),
                                    location: None,
                                }],
                                report: Some(format!("Error: {err}")),
                            },
                        ));
                    }
                }
            }
        }
        results
    }
}

pub struct VerificationToolHook {
    pub registry: VerifierRegistry,
    pending_inputs: Mutex<HashMap<String, serde_json::Value>>,
}

impl VerificationToolHook {
    pub fn new(registry: VerifierRegistry) -> Self {
        Self {
            registry,
            pending_inputs: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl gestalt_core::hook::ToolHook for VerificationToolHook {
    async fn before_tool_execution(
        &self,
        session: &gestalt_core::session::Session,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<Vec<AgentEvent>, gestalt_core::error::HarnessError> {
        if matches!(tool_name, "write" | "patch") {
            if let Some(tool_call_id) = session.tool_ctx.current_tool_call_id.as_ref() {
                if let Ok(mut pending) = self.pending_inputs.lock() {
                    pending.insert(tool_call_id.clone(), input.clone());
                }
            }
        }
        Ok(vec![])
    }

    async fn after_tool_execution(
        &self,
        session: &gestalt_core::session::Session,
        tool_name: &str,
        _result: &gestalt_core::tool::ToolExecutionResult,
    ) -> Result<Vec<AgentEvent>, gestalt_core::error::HarnessError> {
        if !(tool_name == "write" || tool_name == "patch") {
            return Ok(vec![]);
        }

        let Some(tool_call_id) = session.tool_ctx.current_tool_call_id.as_ref() else {
            return Ok(vec![]);
        };

        let input = self
            .pending_inputs
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(tool_call_id));

        let Some(input) = input else {
            return Ok(vec![]);
        };

        let Some(target_path) = input.get("path").and_then(serde_json::Value::as_str) else {
            return Ok(vec![]);
        };

        let path = resolve_tool_path(&session.tool_ctx.working_dir, target_path);
        let artifact_ref = ArtifactRef {
            path,
            mime_type: mime_type_for_path(target_path),
        };
        let workspace_root = session
            .tool_ctx
            .workspace_root
            .clone()
            .unwrap_or_else(|| session.tool_ctx.working_dir.clone());
        let run_dir = session
            .tool_ctx
            .artifact_dir
            .clone()
            .unwrap_or_else(|| session.tool_ctx.working_dir.clone());
        let ctx = VerifyContext {
            workspace_root,
            run_dir,
        };

        let verifier_results = self.registry.run_all(&artifact_ref, &ctx).await;
        let mut events = Vec::new();
        for (name, res) in verifier_results {
            let report = if res.findings.is_empty() {
                res.report
            } else {
                let mut rep = format!("Verifier: {name}\nFindings:\n");
                for f in &res.findings {
                    let _ = writeln!(
                        rep,
                        "- [{:?}] {}{}",
                        f.severity,
                        f.message,
                        f.location
                            .as_ref()
                            .map_or(String::new(), |loc| format!(" at {loc}"))
                    );
                }
                if let Some(r) = res.report {
                    let _ = write!(rep, "\nReport:\n{r}");
                }
                Some(rep)
            };

            events.push(AgentEvent::VerificationResult {
                status: res.status,
                checks: res.findings.len(),
                failed: res
                    .findings
                    .iter()
                    .filter(|f| matches!(f.severity, FindingSeverity::Error))
                    .count(),
                report,
                findings: Some(res.findings),
            });
        }
        Ok(events)
    }
}

fn resolve_tool_path(working_dir: &Path, input_path: &str) -> PathBuf {
    let path = Path::new(input_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    }
}

fn has_extension_ignore_ascii_case(path: &str, candidates: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            candidates
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
}

fn mime_type_for_path(path: &str) -> String {
    if has_extension_ignore_ascii_case(path, &["md", "markdown"]) {
        "text/markdown".to_string()
    } else if has_extension_ignore_ascii_case(path, &["json"]) {
        "application/json".to_string()
    } else if has_extension_ignore_ascii_case(path, &["patch", "diff"]) {
        "text/x-diff".to_string()
    } else {
        "text/plain".to_string()
    }
}
