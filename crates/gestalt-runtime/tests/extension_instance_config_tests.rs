use gestalt_runtime::unstable::extension::ExtensionsConfig;
use gestalt_runtime::unstable::extension::{
    resolve_configured_instances, ExtensionManifestV2, ResolvedExtensionPackage,
};

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
        "workspace_read": true,
        "workspace_write": false,
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
fn extension_grant_aliases_are_rejected() {
    let error = serde_json::from_str::<ExtensionsConfig>(
        r#"
{
  "instances": {
    "review-primary": {
      "package": "com.example.review",
      "grants": {
        "workspaceRead": true
      }
    }
  }
}
"#,
    );

    assert!(error.is_err());
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

#[test]
fn configured_instances_select_components_and_apply_runtime_values() {
    let package = package_with_two_components();
    let config: ExtensionsConfig = serde_json::from_str(
        r#"
{
  "instances": {
    "strict-review": {
      "package": "com.example.review",
      "enabled": true,
      "components": {
        "lifecycle": true,
        "client-metadata": false
      },
      "config": {
        "policySet": "strict"
      },
      "grants": {
        "workspace_read": true,
        "workspace_write": false,
        "network": ["api.example.com"]
      }
    }
  }
}
"#,
    )
    .unwrap();

    let resolved = resolve_configured_instances(&[package], &config.instances).unwrap();
    assert_eq!(resolved.len(), 1);
    let instance = &resolved[0];
    assert_eq!(instance.instance_id, "strict-review");
    assert_eq!(instance.effective_config["policySet"], "strict");
    assert!(instance.effective_grants.workspace_read);
    assert!(!instance.effective_grants.workspace_write);
    assert_eq!(instance.effective_grants.network, ["api.example.com"]);
    assert_eq!(instance.components.len(), 1);
    assert_eq!(instance.components[0].id.instance_id, "strict-review");
    assert_eq!(instance.components[0].config["policySet"], "strict");
    assert!(instance.components[0].grants.workspace_read);
}

#[test]
fn extension_instance_can_disable_component() {
    let package = package_with_two_components();
    let config: ExtensionsConfig = serde_json::from_str(
        r#"{
          "instances": {
            "review": {
              "package": "com.example.review",
              "components": {"client-metadata": false}
            }
          }
        }"#,
    )
    .unwrap();

    let resolved = resolve_configured_instances(&[package], &config.instances).unwrap();
    assert_eq!(resolved[0].components.len(), 1);
    assert_eq!(resolved[0].components[0].id.component_id, "lifecycle");
}

#[test]
fn extension_instance_disabling_all_components_errors() {
    let package = package_with_two_components();
    let config: ExtensionsConfig = serde_json::from_str(
        r#"{
          "instances": {
            "review": {
              "package": "com.example.review",
              "components": {
                "lifecycle": false,
                "client-metadata": false
              }
            }
          }
        }"#,
    )
    .unwrap();

    let error = resolve_configured_instances(&[package], &config.instances)
        .expect_err("an instance with no enabled components must fail");
    assert!(error.to_string().contains("disabled all components"));
}

fn package_with_two_components() -> ResolvedExtensionPackage {
    let manifest = ExtensionManifestV2::parse(
        r#"
manifest_version = 2

[package]
id = "com.example.review"
name = "Review"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "review.lifecycle"]

[[components]]
id = "client-metadata"
kind = "client-product"
optional = true
descriptor = "client/contributions.json"
"#,
    )
    .unwrap();

    ResolvedExtensionPackage::from_v2_manifest(manifest, "review-default").unwrap()
}
