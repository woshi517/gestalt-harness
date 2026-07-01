use std::fs;
use std::path::Path;

use gestalt_core::event::{FindingSeverity, VerificationStatus};
use gestalt_runtime::unstable::{ArtifactRef, VerifyContext};

use crate::config::EffectiveConfig;
use crate::reports::{ArtifactVerificationResult, VerifierResultEntry, VerifyRunReport};
use crate::runs;

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

/// Verification command to execute registered verifiers post-run.
pub async fn verify_run(
    config: &EffectiveConfig,
    run_id_or_path: &str,
) -> Result<VerifyRunReport, Box<dyn std::error::Error>> {
    let run_dir = runs::resolve_run_path(config, run_id_or_path)?;
    let run_id = run_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let artifacts_dir = run_dir.join("artifacts");

    let mut verifier_registry = gestalt_runtime::unstable::VerifierRegistry::new();
    verifier_registry.register(Box::new(gestalt_runtime::unstable::FileExistsVerifier));
    verifier_registry.register(Box::new(gestalt_runtime::unstable::NoSecretsVerifier));
    verifier_registry.register(Box::new(gestalt_runtime::unstable::PatchAppliesVerifier));
    verifier_registry.register(Box::new(
        gestalt_runtime::unstable::MarkdownStructureVerifier,
    ));

    let mut artifact_results = Vec::new();
    let mut total_checks = 0;
    let mut total_failed = 0;
    let mut overall_status = VerificationStatus::Passed;

    if artifacts_dir.exists() && artifacts_dir.is_dir() {
        let entries = fs::read_dir(&artifacts_dir)?;
        let mut artifact_paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                artifact_paths.push(path);
            }
        }
        artifact_paths.sort();

        let ctx = VerifyContext {
            workspace_root: config.workspace_root.clone(),
            run_dir: run_dir.clone(),
        };

        for path in artifact_paths {
            let target_path = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let artifact_ref = ArtifactRef {
                path: path.clone(),
                mime_type: mime_type_for_path(&target_path),
            };

            let verifier_results = verifier_registry.run_all(&artifact_ref, &ctx).await;
            let mut verifiers = Vec::new();

            for (name, res) in verifier_results {
                total_checks += 1;
                let failed_count = res
                    .findings
                    .iter()
                    .filter(|f| matches!(f.severity, FindingSeverity::Error))
                    .count();
                if failed_count > 0 || matches!(res.status, VerificationStatus::Failed) {
                    total_failed += 1;
                    overall_status = VerificationStatus::Failed;
                }

                verifiers.push(VerifierResultEntry {
                    name,
                    status: res.status,
                    findings: res.findings,
                    report: res.report,
                });
            }

            artifact_results.push(ArtifactVerificationResult {
                artifact_path: target_path,
                verifiers,
            });
        }
    }

    if total_checks == 0 {
        overall_status = VerificationStatus::Skipped;
    }

    Ok(VerifyRunReport {
        run_id,
        status: overall_status,
        total_checks,
        total_failed,
        artifacts: artifact_results,
    })
}
