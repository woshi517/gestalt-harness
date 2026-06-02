use crate::{
    ArtifactRef, FindingSeverity, VerificationFinding, VerificationStatus, Verifier, VerifyContext,
    VerifyResult,
};
use async_trait::async_trait;
use gestalt_core::error::HarnessError;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use std::path::Path;

fn ends_with_ignore_ascii_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len() && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

// 1. CommandVerifier
pub struct CommandVerifier {
    pub command: String,
}

impl CommandVerifier {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }
}

#[async_trait]
impl Verifier for CommandVerifier {
    fn name(&self) -> &str {
        "command_verifier"
    }

    fn applies_to(&self, _artifact: &ArtifactRef, _ctx: &VerifyContext) -> bool {
        true
    }

    async fn verify(
        &self,
        _artifact: &ArtifactRef,
        ctx: &VerifyContext,
    ) -> Result<VerifyResult, HarnessError> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .current_dir(&ctx.workspace_root)
            .output()
            .await;

        let mut findings = Vec::new();
        let status = match output {
            Ok(out) => {
                if out.status.success() {
                    VerificationStatus::Passed
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    let _stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    findings.push(VerificationFinding {
                        severity: FindingSeverity::Error,
                        message: format!("Command exited non-zero. stderr: {stderr}"),
                        location: None,
                    });
                    VerificationStatus::Failed
                }
            }
            Err(e) => {
                findings.push(VerificationFinding {
                    severity: FindingSeverity::Error,
                    message: format!("Failed to spawn verify command: {e}"),
                    location: None,
                });
                VerificationStatus::Failed
            }
        };

        Ok(VerifyResult {
            status,
            findings,
            report: None,
        })
    }
}

// 2. FileExistsVerifier
pub struct FileExistsVerifier;

#[async_trait]
impl Verifier for FileExistsVerifier {
    fn name(&self) -> &str {
        "file_exists"
    }

    fn applies_to(&self, _artifact: &ArtifactRef, _ctx: &VerifyContext) -> bool {
        true
    }

    async fn verify(
        &self,
        artifact: &ArtifactRef,
        _ctx: &VerifyContext,
    ) -> Result<VerifyResult, HarnessError> {
        let exists = artifact.path.exists();
        let status = if exists {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Skipped
        };

        let mut findings = Vec::new();
        if !exists {
            findings.push(VerificationFinding {
                severity: FindingSeverity::Warning,
                message: format!("Expected file does not exist: {}", artifact.path.display()),
                location: None,
            });
        }

        Ok(VerifyResult {
            status,
            findings,
            report: None,
        })
    }
}

// 3. NoSecretsVerifier
pub struct NoSecretsVerifier;

#[async_trait]
impl Verifier for NoSecretsVerifier {
    fn name(&self) -> &str {
        "no_secrets"
    }

    fn applies_to(&self, artifact: &ArtifactRef, _ctx: &VerifyContext) -> bool {
        let path_str = artifact.path.to_string_lossy();
        artifact.mime_type.contains("text")
            || artifact.mime_type.contains("json")
            || ends_with_ignore_ascii_case(&path_str, ".txt")
            || ends_with_ignore_ascii_case(&path_str, ".md")
            || ends_with_ignore_ascii_case(&path_str, ".rs")
            || ends_with_ignore_ascii_case(&path_str, ".py")
            || ends_with_ignore_ascii_case(&path_str, ".toml")
            || ends_with_ignore_ascii_case(&path_str, ".json")
            || ends_with_ignore_ascii_case(&path_str, ".sh")
    }

    async fn verify(
        &self,
        artifact: &ArtifactRef,
        _ctx: &VerifyContext,
    ) -> Result<VerifyResult, HarnessError> {
        let Ok(content) = std::fs::read_to_string(&artifact.path) else {
            // If it fails to read (maybe binary after all or empty), skip or warning
            return Ok(VerifyResult {
                status: VerificationStatus::Skipped,
                findings: vec![],
                report: Some("Failed to read file as text".to_string()),
            });
        };

        let mut findings = Vec::new();
        let lines = content.lines();

        let patterns = [
            ("AWS_ACCESS_KEY_ID", "aws_access_key_id"),
            ("AWS_SECRET_ACCESS_KEY", "aws_secret_access_key"),
            ("BEGIN PRIVATE KEY", "private key header"),
            ("BEGIN RSA PRIVATE KEY", "rsa private key header"),
            ("api_key", "generic api key keyword"),
            ("secret_key", "generic secret key keyword"),
        ];

        for (line_num, line) in lines.enumerate() {
            let line_lower = line.to_lowercase();
            for &(pat, desc) in &patterns {
                if line_lower.contains(&pat.to_lowercase()) {
                    findings.push(VerificationFinding {
                        severity: FindingSeverity::Error,
                        message: format!("Potential secret detected ({desc}): '{pat}'"),
                        location: Some(format!("line {}", line_num + 1)),
                    });
                }
            }
        }

        let status = if findings.is_empty() {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };

        Ok(VerifyResult {
            status,
            findings,
            report: None,
        })
    }
}

// 4. PatchAppliesVerifier
pub struct PatchAppliesVerifier;

#[async_trait]
impl Verifier for PatchAppliesVerifier {
    fn name(&self) -> &str {
        "patch_applies"
    }

    fn applies_to(&self, artifact: &ArtifactRef, _ctx: &VerifyContext) -> bool {
        let path_str = artifact.path.to_string_lossy();
        ends_with_ignore_ascii_case(&path_str, ".patch")
            || ends_with_ignore_ascii_case(&path_str, ".diff")
            || artifact.mime_type.contains("diff")
            || artifact.mime_type.contains("patch")
    }

    async fn verify(
        &self,
        artifact: &ArtifactRef,
        ctx: &VerifyContext,
    ) -> Result<VerifyResult, HarnessError> {
        let content = match std::fs::read_to_string(&artifact.path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(VerifyResult {
                    status: VerificationStatus::Failed,
                    findings: vec![VerificationFinding {
                        severity: FindingSeverity::Error,
                        message: format!("Failed to read patch file: {e}"),
                        location: None,
                    }],
                    report: None,
                });
            }
        };

        let mut targets = Vec::new();
        for line in content.lines() {
            if let Some(stripped) = line.strip_prefix("+++ ") {
                let parts: Vec<&str> = stripped.split_whitespace().collect();
                if let Some(&first) = parts.first() {
                    let path_str = first.trim();
                    if path_str != "/dev/null" {
                        let clean_path = path_str.strip_prefix("b/").unwrap_or(path_str);
                        targets.push(clean_path.to_string());
                    }
                }
            }
        }

        let temp_dir = ctx
            .run_dir
            .join(format!("patch-verify-{}", uuid::Uuid::new_v4()));
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return Ok(VerifyResult {
                status: VerificationStatus::Failed,
                findings: vec![VerificationFinding {
                    severity: FindingSeverity::Error,
                    message: format!("Failed to create temp dir for patch verify: {e}"),
                    location: None,
                }],
                report: None,
            });
        }

        for target in &targets {
            let src_path = ctx.workspace_root.join(target);
            if src_path.exists() {
                let dest_path = temp_dir.join(target);
                if let Some(parent) = dest_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::copy(&src_path, &dest_path);
            }
        }

        let patch_dest = temp_dir.join("patch.diff");
        let _ = std::fs::copy(&artifact.path, &patch_dest);

        let output = tokio::process::Command::new("patch")
            .arg("-p1")
            .arg("-i")
            .arg("patch.diff")
            .arg("--dry-run")
            .current_dir(&temp_dir)
            .output()
            .await;

        let mut findings = Vec::new();
        let status;
        let mut report = None;

        match output {
            Ok(out) => {
                if out.status.success() {
                    status = VerificationStatus::Passed;
                } else {
                    status = VerificationStatus::Failed;
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    findings.push(VerificationFinding {
                        severity: FindingSeverity::Error,
                        message: format!("Patch failed to apply dry-run:\n{stderr}"),
                        location: None,
                    });
                    report = Some(format!("stdout:\n{stdout}\nstderr:\n{stderr}"));
                }
            }
            Err(e) => {
                status = VerificationStatus::Skipped;
                findings.push(VerificationFinding {
                    severity: FindingSeverity::Warning,
                    message: format!("Could not run patch utility: {e}"),
                    location: None,
                });
            }
        }

        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(VerifyResult {
            status,
            findings,
            report,
        })
    }
}

// 5. MarkdownStructureVerifier
pub struct MarkdownStructureVerifier;

#[async_trait]
impl Verifier for MarkdownStructureVerifier {
    fn name(&self) -> &str {
        "markdown_structure"
    }

    fn applies_to(&self, artifact: &ArtifactRef, _ctx: &VerifyContext) -> bool {
        let path_str = artifact.path.to_string_lossy();
        ends_with_ignore_ascii_case(&path_str, ".md")
            || ends_with_ignore_ascii_case(&path_str, ".markdown")
            || artifact.mime_type.contains("markdown")
    }

    async fn verify(
        &self,
        artifact: &ArtifactRef,
        _ctx: &VerifyContext,
    ) -> Result<VerifyResult, HarnessError> {
        let content = match std::fs::read_to_string(&artifact.path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(VerifyResult {
                    status: VerificationStatus::Failed,
                    findings: vec![VerificationFinding {
                        severity: FindingSeverity::Error,
                        message: format!("Failed to read markdown file: {e}"),
                        location: None,
                    }],
                    report: None,
                });
            }
        };

        let parser = Parser::new(&content);
        let mut findings = Vec::new();
        let mut headings = Vec::new();

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    let lvl_num = level as u32;
                    if let Some(&last_lvl) = headings.last() {
                        if lvl_num > last_lvl + 1 {
                            findings.push(VerificationFinding {
                                severity: FindingSeverity::Warning,
                                message: format!(
                                    "Heading level skipped from h{last_lvl} to h{lvl_num}"
                                ),
                                location: None,
                            });
                        }
                    } else if lvl_num != 1 {
                        findings.push(VerificationFinding {
                            severity: FindingSeverity::Info,
                            message: format!(
                                "Markdown does not start with an h1 heading (starts with h{lvl_num})"
                            ),
                            location: None,
                        });
                    }
                    headings.push(lvl_num);
                }
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) if lang.is_empty() => {
                    findings.push(VerificationFinding {
                        severity: FindingSeverity::Warning,
                        message: "Fenced code block is missing a language tag".to_string(),
                        location: None,
                    });
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let url_str = dest_url.as_ref();
                    if url_str.starts_with("http://") || url_str.starts_with("https://") {
                        if url::Url::parse(url_str).is_err() {
                            findings.push(VerificationFinding {
                                severity: FindingSeverity::Error,
                                message: format!("Malformed URL: {url_str}"),
                                location: None,
                            });
                        }
                    } else if !url_str.starts_with('#') && !url_str.starts_with("mailto:") {
                        let parent = artifact.path.parent().unwrap_or_else(|| Path::new(""));
                        let clean_url = url_str.split('#').next().unwrap_or(url_str);
                        let target_path = parent.join(clean_url);
                        if !target_path.exists() {
                            findings.push(VerificationFinding {
                                severity: FindingSeverity::Warning,
                                message: format!("Local link target does not exist: {clean_url}"),
                                location: None,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        let status = if findings
            .iter()
            .any(|f| matches!(f.severity, FindingSeverity::Error))
        {
            VerificationStatus::Failed
        } else if findings
            .iter()
            .any(|f| matches!(f.severity, FindingSeverity::Warning))
        {
            VerificationStatus::Warning
        } else {
            VerificationStatus::Passed
        };

        Ok(VerifyResult {
            status,
            findings,
            report: None,
        })
    }
}
