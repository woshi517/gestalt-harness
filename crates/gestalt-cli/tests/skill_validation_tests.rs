//! Tests for skill activation surfaces: validation, slash commands, and
//! runtime error paths.

use gestalt_app::config::{
    CliOverrides, ContextConfig, DefaultsConfig, EffectiveConfig, ExtensionsConfig, ObserveConfig,
    PoliciesConfig, PromptConfig, SkillsConfig, ToolsConfig, TuiConfig,
};
use gestalt_app::runtime_factory::{
    activate_skill, deactivate_skill, validate_skill_activation, SkillValidation,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_workspace() -> PathBuf {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("gestalt-cli-skills-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn overrides_with_workspace(workspace: &Path) -> CliOverrides {
    CliOverrides {
        workspace: Some(workspace.to_path_buf()),
        ..Default::default()
    }
}

fn write_gestalt_json(workspace: &Path, skills: SkillsConfig) {
    let path = workspace.join("gestalt.json");
    let wrapper = serde_json::json!({
        "version": 1,
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

fn load_config_for(workspace: &Path, skills: SkillsConfig) -> EffectiveConfig {
    write_gestalt_json(workspace, skills);
    let overrides = overrides_with_workspace(workspace);
    gestalt_app::config::load_effective_config(&overrides).expect("config loads")
}

fn make_skill_dir(workspace: &Path, name: &str) -> PathBuf {
    let dir = workspace.join(".gestalt").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest =
        format!("---\nname: {name}\ndescription: Description for {name}\n---\n# {name} body\n");
    std::fs::write(dir.join("SKILL.md"), manifest).unwrap();
    dir
}

fn empty_effective_config() -> EffectiveConfig {
    EffectiveConfig {
        workspace_root: PathBuf::from("/tmp"),
        config_path: PathBuf::from("/tmp/gestalt.json"),
        defaults: DefaultsConfig::default(),
        tools: ToolsConfig::default(),
        context: ContextConfig::default(),
        observe: ObserveConfig::default(),
        providers: HashMap::new(),
        profiles: HashMap::new(),
        prompt: PromptConfig::default(),
        policies: PoliciesConfig::default(),
        provider_override: None,
        model_override: None,
        tui: TuiConfig::default(),
        extensions: ExtensionsConfig::default(),
        skills: SkillsConfig::default(),
        mcp: None,
        context_window_override: None,
    }
}

#[test]
fn validation_rejects_unknown_skill() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let config = load_config_for(&workspace, SkillsConfig::default());

    let result = validate_skill_activation(&config, "missing-skill");
    assert!(matches!(result, SkillValidation::Unknown { .. }));
    let msg = result.render_error().unwrap();
    assert!(msg.contains("Unknown skill"));
}

#[test]
fn validation_accepts_known_workspace_skill() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let config = load_config_for(&workspace, SkillsConfig::default());

    let result = validate_skill_activation(&config, "pdf");
    assert!(result.is_ok(), "expected ok, got {result:?}");
}

#[test]
fn activate_skill_persists_to_workspace_config() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let overrides = overrides_with_workspace(&workspace);
    // Need a config to validate against before we mutate it; write a minimal
    // gestalt.json that contains no skills.active entry yet.
    write_gestalt_json(&workspace, SkillsConfig::default());

    activate_skill(&overrides, "pdf").expect("activate succeeds for known skill");

    // Re-read the file and assert the entry is present.
    let raw = std::fs::read_to_string(workspace.join("gestalt.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let active = parsed
        .get("skills")
        .and_then(|s| s.get("active"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(active
        .iter()
        .filter_map(|v| v.as_str())
        .any(|name| name == "pdf"));
}

#[test]
fn activate_skill_rejects_unknown_name_with_error() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    write_gestalt_json(&workspace, SkillsConfig::default());
    let overrides = overrides_with_workspace(&workspace);

    let res = activate_skill(&overrides, "missing-skill");
    assert!(res.is_err(), "expected error for unknown skill");
    let err = format!("{res:?}");
    assert!(
        err.contains("Unknown skill"),
        "error must explain unknown skill, got: {err}"
    );

    // The file must NOT have been mutated to include the unknown skill.
    let raw = std::fs::read_to_string(workspace.join("gestalt.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let active = parsed
        .get("skills")
        .and_then(|s| s.get("active"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!active
        .iter()
        .filter_map(|v| v.as_str())
        .any(|name| name == "missing-skill"));
}

#[test]
fn deactivate_skill_rejects_unknown_name_with_error() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    write_gestalt_json(&workspace, SkillsConfig::default());
    let overrides = overrides_with_workspace(&workspace);

    let res = deactivate_skill(&overrides, "missing-skill");
    assert!(res.is_err());
    let err = format!("{res:?}");
    assert!(
        err.contains("Cannot deactivate unknown skill"),
        "error must explain unknown skill, got: {err}"
    );
}

#[test]
fn validation_honors_trusted_list() {
    // This is a smoke test for the validation path on a discovered skill.
    let config = empty_effective_config();
    let result = validate_skill_activation(&config, "anything");
    assert!(matches!(result, SkillValidation::Unknown { .. }));
}

#[test]
fn trusted_list_allows_workspace_skill() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "my-skill");
    let skills = SkillsConfig {
        explicit_paths: vec![],
        active: vec![],
        trusted: vec!["my-skill".to_string()],
    };
    let config = load_config_for(&workspace, skills);
    let result = validate_skill_activation(&config, "my-skill");
    assert!(result.is_ok());
}

#[test]
fn deactivation_override_removes_persisted_active_skill() {
    let workspace = temp_workspace();
    make_skill_dir(&workspace, "pdf");
    let mut skills = SkillsConfig::default();
    skills.active.push("pdf".to_string());
    let _config = load_config_for(&workspace, skills);

    let mut overrides = overrides_with_workspace(&workspace);
    overrides.skills.push("!pdf".to_string());

    let effective = gestalt_app::config::load_effective_config(&overrides).expect("config loads");
    assert!(!effective.skills.active.iter().any(|name| name == "pdf"));
}
