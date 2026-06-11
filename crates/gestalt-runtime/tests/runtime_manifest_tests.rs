use gestalt_runtime::ExtensionManifest;

#[test]
fn test_manifest_parsing_and_validation() {
    let valid_toml = r#"
id = "mock-extension"
name = "Mock Extension"
version = "1.0.0"
runtime = "stdio"

[entrypoint]
command = "mock-bin"
args = ["--foo", "bar"]

[capabilities]
tools = true
hooks = true
context = true

[permissions]
allow_network = ["github.com"]
allow_workspace_read = true
allow_workspace_write = false
allow_shell = false
allow_all_paths = false
allowed_paths = ["/tmp/allowed"]

[[tools]]
name = "mock_tool"
description = "mock description"
input_schema = { type = "object" }

[[hooks]]
name = "mock_hook"
lifecycle_point = "before_context_build"

[[context_injectors]]
name = "mock_injector"
stability = "turn_dynamic"
"#;

    let manifest = ExtensionManifest::parse(valid_toml).unwrap();
    assert_eq!(manifest.id, "mock-extension");
    assert_eq!(manifest.entrypoint.command, "mock-bin");
    assert_eq!(manifest.permissions.allow_network[0], "github.com");
    assert_eq!(manifest.tools.len(), 1);
    assert_eq!(manifest.hooks.len(), 1);
    assert_eq!(manifest.context_injectors.len(), 1);

    // Validate should succeed
    assert!(manifest.validate(true).is_ok());
}

#[test]
fn test_manifest_validation_failures() {
    // Missing id/name
    let invalid_toml = r#"
id = ""
name = ""
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "mock-bin"
[capabilities]
[permissions]
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Unsupported runtime
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "node"
[entrypoint]
command = "index.js"
[capabilities]
[permissions]
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Tools declared but capabilities.tools is false
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "bin"
[capabilities]
tools = false
[permissions]
[[tools]]
name = "tool1"
description = "desc"
input_schema = {}
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Shell command without allow_shell
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "bin arg1"
[capabilities]
[permissions]
allow_shell = false
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Context injector without stability
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "bin"
[capabilities]
context = true
[permissions]
[[context_injectors]]
name = "missing_stability"
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Shell command with allow_shell
    let shell_ok_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "bin arg1"
[capabilities]
[permissions]
allow_shell = true
"#;
    let mut manifest = ExtensionManifest::parse(shell_ok_toml).unwrap();
    manifest.permissions.allow_shell = true;
    assert!(manifest.validate(true).is_ok());

    // Shell bypass command without allow_shell (known shell executable)
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "bash"
[capabilities]
[permissions]
allow_shell = false
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Shell bypass through entrypoint args without allow_shell
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "env"
args = ["bash", "-c", "echo hi"]
[capabilities]
[permissions]
allow_shell = false
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Shell bypass through wrapper command without shell-only flags
    let invalid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "env"
args = ["bash", "script.sh"]
[capabilities]
[permissions]
allow_shell = false
"#;
    let manifest = ExtensionManifest::parse(invalid_toml).unwrap();
    assert!(manifest.validate(true).is_err());

    // Non-shell commands may legitimately use -c style flags
    let valid_toml = r#"
id = "ext"
name = "ext"
version = "1.0.0"
runtime = "stdio"
[entrypoint]
command = "python"
args = ["-c", "print('hi')"]
[capabilities]
[permissions]
allow_shell = false
"#;
    let manifest = ExtensionManifest::parse(valid_toml).unwrap();
    assert!(manifest.validate(true).is_ok());
}
