use gestalt_core::context::{ContextOmission, ContextPacket, ContextSourceRef};
use gestalt_core::{ContextCaptureMode, ContextStability, DurabilityMode, TraceError};
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
    pub mode: ContextCaptureMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// SHA-256 of the original contribution before capture policy is applied.
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_content_hash: Option<String>,
    pub source_size_bytes: usize,
    pub captured_size_bytes: usize,
}

impl CapturedContributionV1 {
    pub fn capture_once(
        contributor_id: impl Into<String>,
        mode: ContextCaptureMode,
        contribute: impl FnOnce() -> Result<String, TraceError>,
    ) -> Result<Option<Self>, TraceError> {
        Self::capture(contributor_id, contribute()?, mode)
    }

    pub fn capture(
        contributor_id: impl Into<String>,
        content: String,
        mode: ContextCaptureMode,
    ) -> Result<Option<Self>, TraceError> {
        if content.len() > MAX_CAPTURED_CONTRIBUTION_BYTES {
            return Err(TraceError::InvalidFormat {
                line: 0,
                reason: format!(
                    "captured contribution exceeds {MAX_CAPTURED_CONTRIBUTION_BYTES} bytes"
                ),
            });
        }
        if matches!(mode, ContextCaptureMode::Disabled) {
            return Ok(None);
        }

        let content_hash = hash_bytes(content.as_bytes());
        let source_size_bytes = content.len();
        let captured = match mode {
            ContextCaptureMode::Disabled => None,
            ContextCaptureMode::HashOnly => None,
            ContextCaptureMode::Redacted => Some(redact_context_content(&content)),
            ContextCaptureMode::FullForReplay => Some(content),
        };
        let captured_size_bytes = captured.as_ref().map_or(0, String::len);
        let captured_content_hash = captured
            .as_ref()
            .map(|captured| hash_bytes(captured.as_bytes()));
        let capture = Self {
            contributor_id: contributor_id.into(),
            mode,
            content: captured,
            content_hash,
            captured_content_hash,
            source_size_bytes,
            captured_size_bytes,
        };
        capture.verify_integrity()?;
        Ok(Some(capture))
    }

    pub fn capture_hash_only(
        contributor_id: impl Into<String>,
        content: String,
    ) -> Result<Self, TraceError> {
        Self::capture(contributor_id, content, ContextCaptureMode::HashOnly)?
            .ok_or_else(disabled_capture_error)
    }

    pub fn capture_redacted(
        contributor_id: impl Into<String>,
        content: String,
    ) -> Result<Self, TraceError> {
        Self::capture(contributor_id, content, ContextCaptureMode::Redacted)?
            .ok_or_else(disabled_capture_error)
    }

    pub fn capture_full_for_replay(
        contributor_id: impl Into<String>,
        content: String,
    ) -> Result<Self, TraceError> {
        Self::capture(contributor_id, content, ContextCaptureMode::FullForReplay)?
            .ok_or_else(disabled_capture_error)
    }

    pub fn replay_content(&self) -> Result<&str, TraceError> {
        self.verify_integrity()?;
        if !matches!(self.mode, ContextCaptureMode::FullForReplay) {
            return Err(TraceError::InvalidFormat {
                line: 0,
                reason: format!(
                    "captured contribution '{}' is not full_for_replay",
                    self.contributor_id
                ),
            });
        }
        self.content
            .as_deref()
            .ok_or_else(|| self.integrity_error())
    }

    fn verify_integrity(&self) -> Result<(), TraceError> {
        match (&self.mode, &self.content, &self.captured_content_hash) {
            (ContextCaptureMode::Disabled, _, _) => return Err(self.integrity_error()),
            (ContextCaptureMode::HashOnly, None, None) if self.captured_size_bytes == 0 => {}
            (
                ContextCaptureMode::Redacted | ContextCaptureMode::FullForReplay,
                Some(content),
                Some(captured_hash),
            ) if self.captured_size_bytes == content.len()
                && *captured_hash == hash_bytes(content.as_bytes()) => {}
            _ => return Err(self.integrity_error()),
        }

        if matches!(self.mode, ContextCaptureMode::FullForReplay)
            && (self.source_size_bytes != self.captured_size_bytes
                || self.captured_content_hash.as_ref() != Some(&self.content_hash))
        {
            return Err(self.integrity_error());
        }
        if matches!(self.mode, ContextCaptureMode::Redacted) {
            let content = self
                .content
                .as_deref()
                .ok_or_else(|| self.integrity_error())?;
            if redact_context_content(content) != content {
                return Err(self.integrity_error());
            }
        }
        Ok(())
    }

    fn integrity_error(&self) -> TraceError {
        TraceError::InvalidFormat {
            line: 0,
            reason: format!(
                "captured contribution '{}' failed integrity verification",
                self.contributor_id
            ),
        }
    }
}

fn disabled_capture_error() -> TraceError {
    TraceError::InvalidFormat {
        line: 0,
        reason: "disabled capture does not produce a contribution".to_string(),
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
    pub capture_mode: ContextCaptureMode,
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
    pub capture_mode: ContextCaptureMode,
    pub captured_contributions: Vec<CapturedContributionV1>,
    pub source_stabilities: BTreeMap<String, ContextStability>,
    pub deterministic: bool,
    pub prompt_artifact_ref: Option<String>,
    pub projection_artifact_ref: Option<String>,
}

impl ContextBuildReportV1 {
    pub fn build(mut input: ContextBuildReportInputV1<'_>) -> Result<Self, TraceError> {
        input
            .captured_contributions
            .sort_by(|left, right| left.contributor_id.cmp(&right.contributor_id));
        if matches!(input.capture_mode, ContextCaptureMode::Disabled)
            && !input.captured_contributions.is_empty()
        {
            return Err(TraceError::InvalidFormat {
                line: 0,
                reason: "disabled context capture must not contain contributions".to_string(),
            });
        }
        for pair in input.captured_contributions.windows(2) {
            if pair[0].contributor_id == pair[1].contributor_id {
                return Err(TraceError::InvalidFormat {
                    line: 0,
                    reason: format!(
                        "duplicate captured contribution '{}'",
                        pair[0].contributor_id
                    ),
                });
            }
        }
        let captured_bytes =
            input
                .captured_contributions
                .iter()
                .try_fold(0usize, |total, capture| {
                    if capture.mode != input.capture_mode {
                        return Err(TraceError::InvalidFormat {
                            line: 0,
                            reason: format!(
                                "captured contribution '{}' uses {:?}, expected {:?}",
                                capture.contributor_id, capture.mode, input.capture_mode
                            ),
                        });
                    }
                    capture.verify_integrity()?;
                    total.checked_add(capture.source_size_bytes).ok_or_else(|| {
                        TraceError::InvalidFormat {
                            line: 0,
                            reason: "captured contribution size overflow".to_string(),
                        }
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
                source.capture_ref = Some(format!(
                    "{}:{}",
                    capture_mode_label(capture.mode),
                    capture.content_hash
                ));
            }
        }
        sources.sort_by(|left, right| {
            (
                &left.ordering_key,
                &left.output_hash,
                left.token_contribution,
                left.included,
            )
                .cmp(&(
                    &right.ordering_key,
                    &right.output_hash,
                    right.token_contribution,
                    right.included,
                ))
        });
        let mut omissions: Vec<_> = input.packet.omissions.iter().map(omission_report).collect();
        omissions.sort_by(|left, right| {
            (
                &left.source,
                &left.reason_code,
                &left.trust,
                left.token_estimate,
                &left.affected_range,
            )
                .cmp(&(
                    &right.source,
                    &right.reason_code,
                    &right.trust,
                    right.token_estimate,
                    &right.affected_range,
                ))
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
            capture_mode: input.capture_mode,
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
        capture.verify_integrity()?;
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

const fn capture_mode_label(mode: ContextCaptureMode) -> &'static str {
    match mode {
        ContextCaptureMode::Disabled => "disabled",
        ContextCaptureMode::HashOnly => "sha256",
        ContextCaptureMode::Redacted => "redacted",
        ContextCaptureMode::FullForReplay => "full",
    }
}

fn redact_context_content(content: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(content) {
        redact_json_value(&mut value, false);
        return serde_json::to_string(&value).unwrap_or_else(|_| "[REDACTED]".to_string());
    }
    redact_text_content(content)
}

fn redact_json_value(value: &mut serde_json::Value, sensitive: bool) {
    match value {
        serde_json::Value::String(content) if sensitive => {
            *content = "[REDACTED]".to_string();
        }
        serde_json::Value::String(content) => {
            *content = redact_text_content(content);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json_value(item, sensitive);
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, val) in fields {
                let child_sensitive = sensitive || is_sensitive_key(key);
                redact_json_value(val, child_sensitive);
            }
        }
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) if sensitive => {
            *value = serde_json::Value::String("[REDACTED]".to_string());
        }
        _ => {}
    }
}

fn redact_text_content(content: &str) -> String {
    content
        .lines()
        .map(redact_context_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_context_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indentation = &line[..line.len() - trimmed.len()];
    let assignment = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    for delimiter in ['=', ':'] {
        if let Some((key, _)) = assignment.split_once(delimiter) {
            if is_sensitive_key(key.trim()) {
                let export = if trimmed.starts_with("export ") {
                    "export "
                } else {
                    ""
                };
                return format!("{indentation}{export}{}{delimiter}[REDACTED]", key.trim());
            }
        }
    }

    let mut redact_next = false;
    line.split_whitespace()
        .map(|token| {
            if redact_next {
                redact_next = false;
                return "[REDACTED]".to_string();
            }
            let stripped =
                token.trim_matches(|character: char| matches!(character, '"' | '\'' | ',' | ';'));
            let lowered = stripped.to_ascii_lowercase();
            if lowered == "bearer" {
                redact_next = true;
                return token.to_string();
            }
            if let Some((key, delimiter)) = split_assignment(stripped) {
                if is_sensitive_key(key) {
                    return format!("{key}{delimiter}[REDACTED]");
                }
            }
            if stripped.ends_with(':') && is_sensitive_key(stripped.trim_end_matches(':')) {
                redact_next = true;
                return token.to_string();
            }
            if looks_like_context_secret(stripped) {
                "[REDACTED]".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_assignment(value: &str) -> Option<(&str, char)> {
    value
        .split_once('=')
        .map(|(key, _)| (key, '='))
        .or_else(|| value.split_once(':').map(|(key, _)| (key, ':')))
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key
        .trim()
        .trim_matches(|character: char| matches!(character, '"' | '\'' | '.'))
        .to_ascii_lowercase()
        .replace('-', "_");
    matches!(
        key.as_str(),
        "api_key"
            | "apikey"
            | "authorization"
            | "auth_token"
            | "token"
            | "access_token"
            | "refresh_token"
            | "id_token"
            | "secret"
            | "client_secret"
            | "password"
            | "passwd"
            | "credential"
            | "credentials"
            | "provider_credential"
            | "keychain_ref"
    ) || key.ends_with("_api_key")
        || key.ends_with("_token")
        || key.ends_with("_secret")
        || key.ends_with("_password")
        || key.ends_with("_credential")
}

fn looks_like_context_secret(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    value.starts_with("sk-")
        || value.starts_with("sk_ant_")
        || value.starts_with("sk-ant-")
        || value.starts_with("ghp_")
        || value.starts_with("github_pat_")
        || value.starts_with("xox")
        || lowered.starts_with("keychain:")
        || lowered.starts_with("keychain://")
        || is_jwt_like(value)
        || (value.contains("://") && value.contains('@'))
}

fn is_jwt_like(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(first), Some(second), Some(third), None)
            if first.len() >= 8 && second.len() >= 8 && third.len() >= 8
    )
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
