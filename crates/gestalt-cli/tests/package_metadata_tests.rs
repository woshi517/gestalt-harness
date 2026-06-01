const CARGO_TOML: &str = include_str!("../Cargo.toml");
const README: &str = include_str!("../../../README.md");
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const RELEASE_CHECKLIST: &str = include_str!("../../../docs/release-checklist.md");

#[test]
fn package_metadata_matches_public_install_identity() {
    assert!(
        CARGO_TOML.contains("name = \"gestalt-harness\""),
        "package name should be gestalt-harness"
    );
    assert!(
        CARGO_TOML.contains("name = \"gestalt\""),
        "binary name should remain gestalt"
    );
    assert!(
        CARGO_TOML.contains("readme = \"../../README.md\""),
        "package should point at the repository README"
    );
}

#[test]
fn release_docs_reference_package_and_binary_names() {
    assert!(
        README.contains("cargo install --locked --path crates/gestalt-cli"),
        "README should describe the local install command"
    );
    assert!(
        README.contains("package name will be `gestalt-harness`")
            && README.contains("installed executable will remain `gestalt`"),
        "README should describe the published package and binary names"
    );
    assert!(
        CHANGELOG.contains("gestalt-harness") && CHANGELOG.contains("gestalt` binary"),
        "CHANGELOG should mention the package and binary names"
    );
    assert!(
        RELEASE_CHECKLIST.contains("gestalt-harness")
            && RELEASE_CHECKLIST.contains("cargo install --path crates/gestalt-cli"),
        "release checklist should mention install verification"
    );
}
