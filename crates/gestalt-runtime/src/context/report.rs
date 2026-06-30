use gestalt_core::context::{ContextOmission, ContextPacket, ContextSourceRef};
use gestalt_core::{ContextStability, DurabilityMode, TraceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub const CONTEXT_BUILD_REPORT_SCHEMA_VERSION: u32 = 1;
pub const MAX_CAPTURED_CONTRIBUTION_BYTES: usize = 256 * 1024;
pub const MAX_CAPTURED_CONTRIBUTIONS_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPressureV1 {
    Normal,
    Elevated,
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSourceReportV1 {
    pub kind: String,
    pub path_or_label: String,
    pub contributor_id: String,
    pub contributor_version: String,
    pub configuration_hash: String,
    pub output_hash: String,
    pub ordering_key: String,
    pub stability: ContextStability,
    pub trust: String,
    pub requested_authority: Option<String>,
    pub effective_authority: Option<String>,
    pub token_contribution: usize,
    pub included: bool,
    pub capture_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextOmissionReportV1 {
    pub kind: String,
    pub path_or_label: String,
    pub trust: String,
    pub token_estimate: usize,
    pub reason_code: String,
    pub source: String,
    pub affected_range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedContributionV1 {
    pub contributor_id: String,
    pub content: String,
    pub content_hash: String,
    pub size_bytes: usize,
}

impl CapturedContributionV1 {
    pub fn capture_redacted_once(
        contributor_id: impl Into<String>,
        contribute: impl FnOnce() -> Result<String, TraceError>,
    ) -> Result<Self, TraceError> {
        Self::capture_redacted(contributor_id, contribute()?)
    }

    pub fn capture_redacted(
        contributor_id: impl Into<String>,
        content: String,
    ) -> Result<Self, TraceError> {
        if content.len() > MAX_CAPTURED_CONTRIBUTION_BYTES {
            return Err(TraceError::InvalidFormat {
                line: 0,
                reason: format!(
                    "captured contribution exceeds {MAX_CAPTURED_CONTRIBUTION_BYTES} bytes"
                ),
            });
        }
        let content_hash = hash_bytes(content.as_bytes());
        Ok(Self {
            contributor_id: contributor_id.into(),
            size_bytes: content.len(),
            content,
            content_hash,
        })
    }

    pub fn replay_content(&self) -> Result<&str, TraceError> {
        let actual = hash_bytes(self.content.as_bytes());
        if self.size_bytes != self.content.len() || self.content_hash != actual {
            return Err(TraceError::InvalidFormat {
                line: 0,
                reason: format!(
                    "captured contribution '{}' failed integrity verification",
                    self.contributor_id
                ),
            });
        }
        Ok(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBuildReportV1 {
    pub v: u32,
    pub report_id: String,
    pub session_id: String,
    pub run_id: String,
    pub turn_id: usize,
    pub packet_id: String,
    pub pipeline_id: String,
    pub tokenizer_id: String,
    pub token_estimate: usize,
    pub pressure: ContextPressureV1,
    pub deterministic: bool,
    pub sources: Vec<ContextSourceReportV1>,
    pub omissions: Vec<ContextOmissionReportV1>,
    pub captured_contributions: Vec<CapturedContributionV1>,
    pub prompt_artifact_ref: Option<String>,
    pub projection_artifact_ref: Option<String>,
    pub contribution_bound_bytes: usize,
    pub aggregate_bound_bytes: usize,
}

pub struct ContextBuildReportInputV1<'a> {
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub turn_id: usize,
    pub packet: &'a ContextPacket,
    pub input_limit: usize,
    pub context_policy_fingerprint: &'a str,
    pub model_capability_fingerprint: &'a str,
    pub runtime_fingerprint: &'a str,
    pub tool_fingerprint: &'a str,
    pub workspace_snapshot_hash: Option<&'a str>,
    pub captured_contributions: Vec<CapturedContributionV1>,
    pub source_stabilities: BTreeMap<String, ContextStability>,
    pub deterministic: bool,
    pub prompt_artifact_ref: Option<String>,
    pub projection_artifact_ref: Option<String>,
}

impl ContextBuildReportV1 {
    pub fn build(input: ContextBuildReportInputV1<'_>) -> Result<Self, TraceError> {
        let captured_bytes =
            input
                .captured_contributions
                .iter()
                .try_fold(0usize, |total, capture| {
                    capture.replay_content()?;
                    total
                        .checked_add(capture.size_bytes)
                        .ok_or_else(|| TraceError::InvalidFormat {
                            line: 0,
                            reason: "captured contribution size overflow".to_string(),
                        })
                })?;
        if captured_bytes > MAX_CAPTURED_CONTRIBUTIONS_BYTES {
            return Err(TraceError::InvalidFormat {
                line: 0,
                reason: format!(
                    "captured contributions exceed {MAX_CAPTURED_CONTRIBUTIONS_BYTES} bytes"
                ),
            });
        }

        let mut sources: Vec<_> = input
            .packet
            .sources
            .iter()
            .map(|source| {
                let mut report = source_report(source);
                if let Some(stability) = input.source_stabilities.get(&report.contributor_id) {
                    report.stability = *stability;
                }
                report
            })
            .collect();
        for source in &mut sources {
            if let Some(capture) = input
                .captured_contributions
                .iter()
                .find(|capture| capture.contributor_id == source.contributor_id)
            {
                source.output_hash.clone_from(&capture.content_hash);
                source.capture_ref = Some(format!("capture:{}", capture.content_hash));
            }
        }
        sources.sort_by(|left, right| left.ordering_key.cmp(&right.ordering_key));
        let mut omissions: Vec<_> = input.packet.omissions.iter().map(omission_report).collect();
        omissions.sort_by(|left, right| {
            (&left.source, &left.reason_code).cmp(&(&right.source, &right.reason_code))
        });

        let pipeline_id = hash_json(&serde_json::json!({
            "context_policy": input.context_policy_fingerprint,
            "model_capabilities": input.model_capability_fingerprint,
            "runtime": input.runtime_fingerprint,
            "tools": input.tool_fingerprint,
            "workspace": input.workspace_snapshot_hash,
            "contributors": sources,
        }))?;
        Ok(Self {
            v: CONTEXT_BUILD_REPORT_SCHEMA_VERSION,
            report_id: input.packet.packet_hash.clone(),
            session_id: input.session_id.to_string(),
            run_id: input.run_id.to_string(),
            turn_id: input.turn_id,
            packet_id: input.packet.packet_hash.clone(),
            pipeline_id,
            tokenizer_id: input.packet.tokenizer_id.clone(),
            token_estimate: input.packet.token_estimate,
            pressure: pressure(input.packet.token_estimate, input.input_limit),
            deterministic: input.deterministic,
            sources,
            omissions,
            captured_contributions: input.captured_contributions,
            prompt_artifact_ref: input.prompt_artifact_ref,
            projection_artifact_ref: input.projection_artifact_ref,
            contribution_bound_bytes: MAX_CAPTURED_CONTRIBUTION_BYTES,
            aggregate_bound_bytes: MAX_CAPTURED_CONTRIBUTIONS_BYTES,
        })
    }

    pub fn replay_contribution(&self, contributor_id: &str) -> Result<&str, TraceError> {
        self.captured_contributions
            .iter()
            .find(|capture| capture.contributor_id == contributor_id)
            .ok_or_else(|| TraceError::InvalidFormat {
                line: 0,
                reason: format!("missing captured contribution '{contributor_id}'"),
            })?
            .replay_content()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPersistenceDiagnosticV1 {
    pub code: &'static str,
    pub message: String,
}

pub fn persist_context_build_report(
    report: &ContextBuildReportV1,
    artifacts_dir: &Path,
    durability: DurabilityMode,
) -> Result<Option<ContextPersistenceDiagnosticV1>, TraceError> {
    if matches!(durability, DurabilityMode::Disabled) {
        return Ok(None);
    }
    let result = (|| {
        fs::create_dir_all(artifacts_dir)?;
        let content = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
        fs::write(
            artifacts_dir.join(format!("context_report_{}.json", report.report_id)),
            content,
        )
    })();
    match result {
        Ok(()) => Ok(None),
        Err(error) if matches!(durability, DurabilityMode::Required) => {
            Err(TraceError::WriteFailed(error))
        }
        Err(error) => Ok(Some(ContextPersistenceDiagnosticV1 {
            code: "CONTEXT_REPORT_PERSISTENCE_FAILED",
            message: error.to_string(),
        })),
    }
}

pub fn load_context_build_report(
    report_id: &str,
    artifacts_dir: &Path,
) -> Result<ContextBuildReportV1, TraceError> {
    let path = artifacts_dir.join(format!("context_report_{report_id}.json"));
    let content = fs::read(&path).map_err(|error| TraceError::ReadFailed {
        reason: format!("failed to read {}: {error}", path.display()),
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&content).map_err(|error| TraceError::ReadFailed {
            reason: format!("failed to parse context report: {error}"),
        })?;
    if value.get("v").and_then(serde_json::Value::as_u64)
        != Some(u64::from(CONTEXT_BUILD_REPORT_SCHEMA_VERSION))
    {
        return Err(TraceError::InvalidFormat {
            line: 0,
            reason: "unsupported context report schema version".to_string(),
        });
    }
    let report: ContextBuildReportV1 =
        serde_json::from_value(value).map_err(|error| TraceError::ReadFailed {
            reason: format!("failed to parse context report: {error}"),
        })?;
    for capture in &report.captured_contributions {
        capture.replay_content()?;
    }
    Ok(report)
}

fn source_report(source: &ContextSourceRef) -> ContextSourceReportV1 {
    let contributor_id = format!("{}:{}", source.kind, source.path_or_label);
    let ordering_key = format!(
        "{}\0{}\0{}\0{}",
        source.kind,
        source.path_or_label,
        source.trust,
        source.authority.as_deref().unwrap_or_default()
    );
    ContextSourceReportV1 {
        kind: source.kind.clone(),
        path_or_label: source.path_or_label.clone(),
        contributor_version: "v1".to_string(),
        configuration_hash: hash_bytes(ordering_key.as_bytes()),
        output_hash: hash_json(source).unwrap_or_default(),
        ordering_key,
        stability: ContextStability::TurnDynamic,
        trust: source.trust.clone(),
        requested_authority: source.authority.clone(),
        effective_authority: source.authority.clone(),
        token_contribution: source.token_estimate,
        included: source.included,
        capture_ref: None,
        contributor_id,
    }
}

fn omission_report(omission: &ContextOmission) -> ContextOmissionReportV1 {
    ContextOmissionReportV1 {
        kind: omission.kind.clone(),
        path_or_label: omission.path_or_label.clone(),
        trust: omission.trust.clone(),
        token_estimate: omission.token_estimate,
        reason_code: omission.reason.clone(),
        source: format!("{}:{}", omission.kind, omission.path_or_label),
        affected_range: None,
    }
}

impl From<&ContextSourceRef> for ContextSourceReportV1 {
    fn from(source: &ContextSourceRef) -> Self {
        source_report(source)
    }
}

impl From<&ContextOmission> for ContextOmissionReportV1 {
    fn from(omission: &ContextOmission) -> Self {
        omission_report(omission)
    }
}

fn pressure(estimate: usize, limit: usize) -> ContextPressureV1 {
    if limit == 0 || estimate >= limit {
        ContextPressureV1::Exhausted
    } else if estimate >= limit.saturating_mul(4) / 5 {
        ContextPressureV1::Elevated
    } else {
        ContextPressureV1::Normal
    }
}

fn hash_json(value: &impl Serialize) -> Result<String, TraceError> {
    serde_json::to_vec(value)
        .map(|bytes| hash_bytes(&bytes))
        .map_err(|error| TraceError::InvalidFormat {
            line: 0,
            reason: format!("failed to hash context report data: {error}"),
        })
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
