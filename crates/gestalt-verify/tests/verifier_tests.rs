use gestalt_verify::{
    ArtifactRef, CommandVerifier, FileExistsVerifier, MarkdownStructureVerifier, NoSecretsVerifier,
    PatchAppliesVerifier, VerificationStatus, Verifier, VerifyContext,
};
use std::fs;

fn create_temp_dir() -> std::path::PathBuf {
    let test_id = uuid::Uuid::new_v4().to_string();
    let dir = std::env::temp_dir().join(format!("verifier-test-{test_id}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn test_file_exists_verifier() {
    let dir = create_temp_dir();
    let file_path = dir.join("test.txt");
    fs::write(&file_path, "hello").unwrap();

    let ctx = VerifyContext {
        workspace_root: dir.clone(),
        run_dir: dir.clone(),
    };

    let verifier = FileExistsVerifier;
    let artifact_existing = ArtifactRef {
        path: file_path.clone(),
        mime_type: "text/plain".to_string(),
    };
    let res = verifier.verify(&artifact_existing, &ctx).await.unwrap();
    assert_eq!(res.status, VerificationStatus::Passed);
    assert!(res.findings.is_empty());

    let artifact_missing = ArtifactRef {
        path: dir.join("missing.txt"),
        mime_type: "text/plain".to_string(),
    };
    let res_missing = verifier.verify(&artifact_missing, &ctx).await.unwrap();
    assert_eq!(res_missing.status, VerificationStatus::Skipped);
    assert_eq!(res_missing.findings.len(), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_command_verifier() {
    let dir = create_temp_dir();
    let ctx = VerifyContext {
        workspace_root: dir.clone(),
        run_dir: dir.clone(),
    };
    let artifact = ArtifactRef {
        path: dir.join("test.txt"),
        mime_type: "text/plain".to_string(),
    };

    let verifier_ok = CommandVerifier::new("echo 'hello world'");
    let res_ok = verifier_ok.verify(&artifact, &ctx).await.unwrap();
    assert_eq!(res_ok.status, VerificationStatus::Passed);

    let verifier_fail = CommandVerifier::new("exit 1");
    let res_fail = verifier_fail.verify(&artifact, &ctx).await.unwrap();
    assert_eq!(res_fail.status, VerificationStatus::Failed);
    assert!(!res_fail.findings.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_no_secrets_verifier() {
    let dir = create_temp_dir();
    let clean_file = dir.join("clean.txt");
    let dirty_file = dir.join("dirty.txt");

    fs::write(&clean_file, "This is clean text.").unwrap();
    fs::write(
        &dirty_file,
        "Some code\nAWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\nMore code",
    )
    .unwrap();

    let ctx = VerifyContext {
        workspace_root: dir.clone(),
        run_dir: dir.clone(),
    };

    let verifier = NoSecretsVerifier;
    let art_clean = ArtifactRef {
        path: clean_file,
        mime_type: "text/plain".to_string(),
    };
    let res_clean = verifier.verify(&art_clean, &ctx).await.unwrap();
    assert_eq!(res_clean.status, VerificationStatus::Passed);
    assert!(res_clean.findings.is_empty());

    let art_dirty = ArtifactRef {
        path: dirty_file,
        mime_type: "text/plain".to_string(),
    };
    let res_dirty = verifier.verify(&art_dirty, &ctx).await.unwrap();
    assert_eq!(res_dirty.status, VerificationStatus::Failed);
    assert_eq!(res_dirty.findings.len(), 1);
    assert_eq!(res_dirty.findings[0].location.as_deref(), Some("line 2"));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_patch_applies_verifier() {
    let dir = create_temp_dir();
    let target_file = dir.join("target.txt");
    fs::write(&target_file, "Line 1\nLine 2\nLine 3\n").unwrap();

    let patch_file = dir.join("patch.diff");
    fs::write(
        &patch_file,
        "--- a/target.txt\n+++ b/target.txt\n@@ -1,3 +1,3 @@\n Line 1\n-Line 2\n+Modified Line 2\n Line 3\n",
    )
    .unwrap();

    let ctx = VerifyContext {
        workspace_root: dir.clone(),
        run_dir: dir.clone(),
    };

    let verifier = PatchAppliesVerifier;
    let art = ArtifactRef {
        path: patch_file,
        mime_type: "text/x-diff".to_string(),
    };

    let res = verifier.verify(&art, &ctx).await.unwrap();
    assert_eq!(res.status, VerificationStatus::Passed);

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_patch_applies_verifier_fails_for_invalid_patch() {
    let dir = create_temp_dir();
    let target_file = dir.join("target.txt");
    fs::write(&target_file, "Line 1\nLine 2\nLine 3\n").unwrap();

    let patch_file = dir.join("bad.patch");
    fs::write(
        &patch_file,
        "--- a/missing.txt\n+++ b/missing.txt\n@@ -1,3 +1,3 @@\n Line 1\n-Line 2\n+Modified Line 2\n Line 3\n",
    )
    .unwrap();

    let ctx = VerifyContext {
        workspace_root: dir.clone(),
        run_dir: dir.clone(),
    };

    let verifier = PatchAppliesVerifier;
    let art = ArtifactRef {
        path: patch_file,
        mime_type: "text/x-diff".to_string(),
    };

    let res = verifier.verify(&art, &ctx).await.unwrap();
    assert_eq!(res.status, VerificationStatus::Failed);
    assert!(!res.findings.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_markdown_structure_verifier() {
    let dir = create_temp_dir();
    let bad_md = dir.join("bad.md");
    fs::write(
        &bad_md,
        "# H1\n\n### Skipped H3\n\n```\nno lang block\n```\n\n[broken](./broken.txt)",
    )
    .unwrap();

    let ctx = VerifyContext {
        workspace_root: dir.clone(),
        run_dir: dir.clone(),
    };

    let verifier = MarkdownStructureVerifier;
    let art = ArtifactRef {
        path: bad_md,
        mime_type: "text/markdown".to_string(),
    };

    let res = verifier.verify(&art, &ctx).await.unwrap();
    assert_eq!(res.status, VerificationStatus::Warning);
    assert_eq!(res.findings.len(), 3);
    assert!(res
        .findings
        .iter()
        .any(|f| f.message.contains("Heading level skipped")));
    assert!(res
        .findings
        .iter()
        .any(|f| f.message.contains("missing a language tag")));
    assert!(res
        .findings
        .iter()
        .any(|f| f.message.contains("Local link target does not exist")));

    let _ = fs::remove_dir_all(&dir);
}
