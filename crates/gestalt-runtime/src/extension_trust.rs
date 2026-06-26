#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExtensionTrust {
    BuiltIn,
    IntegrityTrusted { manifest_hash: String },
    Untrusted,
}

impl ExtensionTrust {
    pub fn is_trusted(&self) -> bool {
        matches!(self, Self::BuiltIn | Self::IntegrityTrusted { .. })
    }
}

use crate::manifest::{ExtensionManifest, ToolDeclaration};
use gestalt_core::tool::RiskLevel;
use gestalt_core::tool_descriptor::{
    AnnotationSource, CanonicalToolId, ProviderToolFormat, ToolAnnotation, ToolAnnotations,
    ToolDescriptor, ToolNamespace, ToolResponseContract,
};

/// Build a `ToolDescriptor` for a tool declared by an extension process.
///
/// The descriptor carries the canonical `extension:<id>:<tool>` id,
/// an `ExtensionDeclared` annotation source for extension-supplied
/// hints, and the harness-side trust normalization.
pub fn build_extension_tool_descriptor(
    manifest: &ExtensionManifest,
    tool_decl: &ToolDeclaration,
    is_trusted: bool,
) -> ToolDescriptor {
    let canonical_id = CanonicalToolId {
        namespace: ToolNamespace::Extension(manifest.id.clone()),
        name: tool_decl.name.clone(),
    };

    let risk = match tool_decl.risk.as_deref() {
        Some("low") => RiskLevel::Low,
        Some("medium") => RiskLevel::Medium,
        Some("high") => RiskLevel::High,
        Some("critical") => RiskLevel::Critical,
        _ => RiskLevel::High,
    };

    // Trust normalization:
    //
    // * If the manifest is on the harness allow-list, its
    //   annotations are promoted to `BuiltInTrusted` so the
    //   policy engine and retry path can act on them.
    // * Otherwise, every annotation is recorded as
    //   `ExtensionDeclared` so consumers can see the hint but
    //   the system never grants automatic retry on the basis of
    //   an extension claim alone.
    let trusted_extension = is_trusted;

    // Trust gate for retry: extensions only get a non-`None`
    // retry policy when they are on the trusted allow-list. That
    // mirrors U6B: "Allow only trusted built-in descriptors or
    // explicitly user-reviewed policy to enable automatic retry."
    let retry_policy = if trusted_extension
        && tool_decl.read_only.unwrap_or(false)
        && tool_decl.idempotent.unwrap_or(false)
    {
        Some(gestalt_core::tool_descriptor::ToolRetryPolicy {
            max_retries: 1,
            backoff_ms: 200,
        })
    } else {
        None
    };

    let mut annotations = Vec::new();
    let source = if trusted_extension {
        AnnotationSource::BuiltInTrusted
    } else {
        AnnotationSource::ExtensionDeclared
    };

    annotations.push(ToolAnnotation {
        key: "read_only".to_string(),
        value: tool_decl
            .read_only
            .map_or_else(|| "false".to_string(), |v| v.to_string()),
        source,
    });
    annotations.push(ToolAnnotation {
        key: "idempotent".to_string(),
        value: tool_decl
            .idempotent
            .map_or_else(|| "false".to_string(), |v| v.to_string()),
        source,
    });
    // Always record the extension id so trace consumers and policy
    // can recover the provenance even after a descriptor hash
    // round-trip.
    annotations.push(ToolAnnotation {
        key: "extension_id".to_string(),
        value: manifest.id.clone(),
        source: AnnotationSource::ExtensionDeclared,
    });

    let read_only = tool_decl.read_only.unwrap_or(false);
    let idempotent = tool_decl.idempotent.unwrap_or(false);
    let retention = if trusted_extension {
        let clearable = read_only && matches!(risk, RiskLevel::Low);
        gestalt_core::context::ToolRetention::from_clearable(idempotent, clearable)
    } else {
        gestalt_core::context::ToolRetention::conservative_default()
    };

    ToolDescriptor {
        id: canonical_id,
        description: tool_decl.description.clone(),
        schema: serde_json::json!({
            "name": tool_decl.name.clone(),
            "description": tool_decl.description.clone(),
            "input_schema": tool_decl.input_schema.clone(),
        }),
        risk,
        annotations: ToolAnnotations::new(annotations),
        response_contract: ToolResponseContract {
            format: ProviderToolFormat::Text,
            shape_rules: None,
        },
        retry_policy,
        retention: Some(retention),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        Capabilities, Entrypoint, ExtensionManifest, Permissions, ToolDeclaration,
    };
    use gestalt_core::tool_descriptor::{AnnotationSource, ToolNamespace};

    fn manifest(id: &str) -> ExtensionManifest {
        ExtensionManifest {
            id: id.to_string(),
            name: format!("{id} test"),
            version: "0.0.1".to_string(),
            manifest_version: None,
            protocol_version: None,
            runtime: "stdio".to_string(),
            entrypoint: Entrypoint {
                command: "/bin/echo".to_string(),
                args: vec![],
            },
            capabilities: Capabilities {
                tools: true,
                hooks: false,
                context: false,
                ..Default::default()
            },
            permissions: Permissions {
                allow_network: vec![],
                allow_workspace_read: false,
                allow_workspace_write: false,
                allow_shell: false,
                allow_all_paths: false,
                allowed_paths: vec![],
            },
            tools: vec![],
            hooks: vec![],
            context_injectors: vec![],
        }
    }

    fn tool_decl(name: &str, read_only: Option<bool>, idempotent: Option<bool>) -> ToolDeclaration {
        ToolDeclaration {
            name: name.to_string(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            risk: Some("low".to_string()),
            read_only,
            idempotent,
        }
    }

    #[test]
    fn untrusted_extension_uses_extension_declared_source() {
        let descriptor = build_extension_tool_descriptor(
            &manifest("untrusted"),
            &tool_decl("x", Some(true), Some(true)),
            false,
        );
        assert_eq!(
            descriptor.id.namespace,
            ToolNamespace::Extension("untrusted".to_string())
        );
        let read_only = descriptor.annotations.get("read_only").unwrap();
        assert_eq!(read_only.source, AnnotationSource::ExtensionDeclared);
        assert_eq!(read_only.value, "true");
        assert!(descriptor.retry_policy.is_none());
        assert_eq!(
            descriptor.retention,
            Some(gestalt_core::context::ToolRetention::conservative_default())
        );
    }

    #[test]
    fn trusted_extension_promotes_to_builtin_trusted() {
        let descriptor = build_extension_tool_descriptor(
            &manifest("trusted-ext"),
            &tool_decl("x", Some(true), Some(true)),
            true,
        );
        let read_only = descriptor.annotations.get("read_only").unwrap();
        assert_eq!(read_only.source, AnnotationSource::BuiltInTrusted);
        assert!(descriptor.retry_policy.is_some());
        assert_eq!(
            descriptor.retention,
            Some(gestalt_core::context::ToolRetention {
                clearable: true,
                reconstructible: true,
                retain_errors: true,
            })
        );
    }

    #[test]
    fn trusted_extension_without_annotations_does_not_get_retry_policy() {
        let descriptor = build_extension_tool_descriptor(
            &manifest("trusted-ext"),
            &tool_decl("x", None, None),
            true,
        );
        assert!(descriptor.retry_policy.is_none());
    }

    #[test]
    fn trusted_mutating_extension_is_not_clearable() {
        let mut declaration = tool_decl("mutating", Some(false), Some(true));
        declaration.risk = Some("high".to_string());
        let descriptor =
            build_extension_tool_descriptor(&manifest("trusted-ext"), &declaration, true);
        assert_eq!(
            descriptor.retention,
            Some(gestalt_core::context::ToolRetention::conservative_default())
        );
    }
}
