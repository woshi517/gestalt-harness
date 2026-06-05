use gestalt_runtime::ExtensionDiscovery;
use std::fs;
use std::path::PathBuf;

#[test]
fn test_extension_discovery_priority() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test_workspace");
    let global_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test_global");

    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&global_dir);

    let proj_ext_dir = workspace.join(".gestalt/extensions/ext-a");
    let glob_ext_dir = global_dir.join("extensions/ext-b");
    fs::create_dir_all(&proj_ext_dir).unwrap();
    fs::create_dir_all(&glob_ext_dir).unwrap();

    let ext_a_toml = r#"
id = "ext-a"
name = "Ext A"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "cmd-a"
[capabilities]
[permissions]
"#;

    let ext_b_toml = r#"
id = "ext-b"
name = "Ext B"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "cmd-b"
[capabilities]
[permissions]
"#;

    fs::write(proj_ext_dir.join("gestalt.extension.toml"), ext_a_toml).unwrap();
    fs::write(glob_ext_dir.join("gestalt.extension.toml"), ext_b_toml).unwrap();

    let discovery = ExtensionDiscovery::new(workspace.clone(), Some(global_dir.clone()));

    let all = discovery.discover_all(&[]).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].manifest.id, "ext-a");
    assert_eq!(all[1].manifest.id, "ext-b");

    let explicit_path = glob_ext_dir.join("gestalt.extension.toml");
    let all_with_explicit = discovery.discover_all(&[explicit_path]).unwrap();
    assert_eq!(all_with_explicit.len(), 2);
    assert_eq!(all_with_explicit[0].manifest.id, "ext-b");
    assert_eq!(all_with_explicit[1].manifest.id, "ext-a");

    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&global_dir);
}
