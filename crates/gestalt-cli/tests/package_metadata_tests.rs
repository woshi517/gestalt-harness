const CARGO_TOML: &str = include_str!("../Cargo.toml");
const README: &str = include_str!("../../../README.md");
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const RELEASE_CHECKLIST: &str = include_str!("../../../docs/release-checklist.md");

#[test]
fn package_metadata_matches_public_install_identity() {
    assert!(
        CARGO_TOML.contains("name = \"gestalt-cli\""),
        "package name should be gestalt-cli"
    );
    assert!(
        CARGO_TOML.contains("name = \"gestalt\""),
        "binary name should remain gestalt"
    );
}

#[test]
fn release_docs_reference_package_and_binary_names() {
    assert!(
        README.contains("cargo install --locked --path crates/gestalt-cli"),
        "README should describe the local install command"
    );
    assert!(
        README.contains("package name will be `gestalt-cli`")
            && README.contains("installed executable will remain `gestalt`"),
        "README should describe the published package and binary names"
    );
    assert!(
        CHANGELOG.contains("gestalt-cli") && CHANGELOG.contains("gestalt` binary"),
        "CHANGELOG should mention the package and binary names"
    );
    assert!(
        RELEASE_CHECKLIST.contains("gestalt-cli")
            && RELEASE_CHECKLIST.contains("cargo install --path crates/gestalt-cli"),
        "release checklist should mention install verification"
    );
}
