use gestalt_skills::{SkillDiscovery, SkillTrustLevel};
use std::path::PathBuf;

#[test]
fn test_fixture_skill_discovery() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/skills/pdf-processing");

    let discovery = SkillDiscovery::new(PathBuf::from("/tmp"), None, None);
    let found = discovery.discover_all(&[fixture_dir]).unwrap();

    assert!(!found.is_empty(), "Expected at least one fixture skill");
    let pdf = found.iter().find(|s| s.name == "pdf-processing");
    assert!(pdf.is_some(), "Expected pdf-processing skill");

    let pdf = pdf.unwrap();
    assert_eq!(
        pdf.description,
        "Extract PDF text, fill forms, merge files. Use when handling PDFs."
    );
    assert_eq!(pdf.trust_level, SkillTrustLevel::Explicit);
    assert!(pdf.allowed_tools.as_ref().unwrap().contains("Read"));
}
