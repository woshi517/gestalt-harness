use gestalt_runtime::extension::ExtensionsConfig;

#[test]
fn extension_instances_deserialize_with_grants_and_component_overrides() {
    let config: ExtensionsConfig = serde_json::from_str(
        r#"
{
  "instances": {
    "review-primary": {
      "package": "com.example.review",
      "enabled": true,
      "components": {
        "lifecycle": true,
        "client-metadata": false
      },
      "config": {
        "policySet": "default"
      },
      "grants": {
        "workspaceRead": true,
        "workspaceWrite": false,
        "network": ["api.example.com"]
      }
    }
  }
}
"#,
    )
    .unwrap();

    let instance = config.instances.get("review-primary").unwrap();
    assert_eq!(instance.package, "com.example.review");
    assert!(instance.enabled);
    assert_eq!(instance.components.get("lifecycle"), Some(&true));
    assert_eq!(instance.components.get("client-metadata"), Some(&false));
    assert_eq!(instance.config["policySet"], "default");
    assert!(instance.grants.workspace_read);
    assert!(!instance.grants.workspace_write);
    assert_eq!(instance.grants.network, ["api.example.com"]);
}

#[test]
fn extension_instances_default_to_enabled_with_empty_grants() {
    let config: ExtensionsConfig = serde_json::from_str(
        r#"
{
  "instances": {
    "review-primary": {
      "package": "com.example.review"
    }
  }
}
"#,
    )
    .unwrap();

    let instance = config.instances.get("review-primary").unwrap();
    assert!(instance.enabled);
    assert!(instance.components.is_empty());
    assert!(instance.grants.network.is_empty());
    assert!(!instance.grants.workspace_read);
    assert!(!instance.grants.workspace_write);
}
