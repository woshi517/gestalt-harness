//! Tests for the `/skill` slash command. Validates that unknown or
//! untrusted skill names are rejected, and that known names produce the
//! expected `SlashOutcome` for the chat loop to consume.

use gestalt_cli::config::{
    CliOverrides, EffectiveConfig, SkillsConfig,
};
use gestalt_cli::slash::{handle_slash_command, SlashOutcome};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_workspace() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "gestalt-slash-skills-{}-{}",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn overrides_with_workspace(workspace: PathBuf) -> CliOverrides {
    CliOverrides {
        workspace: Some(workspace),
        ..Default::default()
    }
}

fn write_gestalt_json(workspace: &PathBuf, skills: SkillsConfig) {
    let path = workspace.join("gestalt.json");
    let wrapper = serde_json::json!({
        "providers": {},
        "profiles": {},
        "tools": {},
        "policies": {},
        "prompt": {},
        "skills": skills,
        "extensions": {},
    });
    std::fs::write(&path, serde_json::to_string_pretty(&wrapper).unwrap()).unwrap();
}

fn make_skill_dir(workspace: &PathBuf, name: &str) -> PathBuf {
    let dir = workspace.join(".gestalt").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = format!(
        "---\nname: {name}\ndescription: Description for {name}\n---\n# {name} body\n"
    );
    std::fs::write(dir.join("SKILL.md"), manifest).unwrap();
    dir
}

fn load_config(workspace: &PathBuf) -> EffectiveConfig {
    write_gestalt_json(workspace, SkillsConfig::default());
    let overrides = overrides_with_workspace(workspace.clone());
    gestalt_cli::config::load_effective_config(&overrides).expect("config loads")
}

#[tokio::test]
async fn slash_skill_unknown_name_does_not_activate() {
    std::env::set_var("GESTALT_NO_GLOBAL_SKILLS", "1");
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let config = load_config(&workspace);
    let mut overrides = overrides_with_workspace(workspace.clone());

    let outcome = handle_slash_command(
        "/skill missing-skill",
        "test-session",
        None,
        &mut overrides,
        &config,
    )
    .await
    .expect("slash handler returns Ok");
    assert!(
        matches!(outcome, SlashOutcome::None),
        "unknown name must not produce SkillActivated; got {outcome:?}"
    );
    std::env::remove_var("GESTALT_NO_GLOBAL_SKILLS");
}

#[tokio::test]
async fn slash_skill_known_name_returns_activation_outcome() {
    std::env::set_var("GESTALT_NO_GLOBAL_SKILLS", "1");
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let config = load_config(&workspace);
    let mut overrides = overrides_with_workspace(workspace.clone());

    let outcome = handle_slash_command("/skill pdf", "test-session", None, &mut overrides, &config)
        .await
        .expect("slash handler returns Ok");
    match outcome {
        SlashOutcome::SkillActivated(name) => assert_eq!(name, "pdf"),
        other => panic!("expected SkillActivated, got {other:?}"),
    }
    std::env::remove_var("GESTALT_NO_GLOBAL_SKILLS");
}

#[tokio::test]
async fn slash_skill_off_known_name_returns_deactivation_outcome() {
    std::env::set_var("GESTALT_NO_GLOBAL_SKILLS", "1");
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let config = load_config(&workspace);
    let mut overrides = overrides_with_workspace(workspace.clone());

    let outcome =
        handle_slash_command("/skill off pdf", "test-session", None, &mut overrides, &config)
            .await
            .expect("slash handler returns Ok");
    match outcome {
        SlashOutcome::SkillDeactivated(name) => assert_eq!(name, "pdf"),
        other => panic!("expected SkillDeactivated, got {other:?}"),
    }
    std::env::remove_var("GESTALT_NO_GLOBAL_SKILLS");
}

#[tokio::test]
async fn slash_skill_off_unknown_name_does_not_return_outcome() {
    std::env::set_var("GESTALT_NO_GLOBAL_SKILLS", "1");
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let config = load_config(&workspace);
    let mut overrides = overrides_with_workspace(workspace.clone());

    let outcome =
        handle_slash_command("/skill off missing", "test-session", None, &mut overrides, &config)
            .await
            .expect("slash handler returns Ok");
    assert!(
        matches!(outcome, SlashOutcome::None),
        "unknown deactivation target must not produce SkillDeactivated; got {outcome:?}"
    );
    std::env::remove_var("GESTALT_NO_GLOBAL_SKILLS");
}

#[tokio::test]
async fn slash_skill_missing_args_is_noop() {
    std::env::set_var("GESTALT_NO_GLOBAL_SKILLS", "1");
    let workspace = temp_workspace();
    let config = load_config(&workspace);
    let mut overrides = overrides_with_workspace(workspace.clone());

    let outcome = handle_slash_command("/skill", "test-session", None, &mut overrides, &config)
        .await
        .expect("slash handler returns Ok");
    assert!(matches!(outcome, SlashOutcome::None));
    std::env::remove_var("GESTALT_NO_GLOBAL_SKILLS");
}

#[tokio::test]
async fn slash_skill_off_missing_args_is_noop() {
    std::env::set_var("GESTALT_NO_GLOBAL_SKILLS", "1");
    let workspace = temp_workspace();
    let config = load_config(&workspace);
    let mut overrides = overrides_with_workspace(workspace.clone());

    let outcome = handle_slash_command("/skill off", "test-session", None, &mut overrides, &config)
        .await
        .expect("slash handler returns Ok");
    assert!(matches!(outcome, SlashOutcome::None));
    std::env::remove_var("GESTALT_NO_GLOBAL_SKILLS");
}
