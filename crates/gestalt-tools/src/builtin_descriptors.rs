use gestalt_core::tool::Tool;
use gestalt_core::tool_descriptor::{
    AnnotationSource, CanonicalToolId, ProviderToolFormat, ToolAnnotation, ToolAnnotations,
    ToolDescriptor, ToolNamespace, ToolResponseContract, ToolRetryPolicy,
};

pub fn make_builtin_descriptor(
    tool: &dyn Tool,
    read_only: bool,
    idempotent: bool,
    retry_policy: Option<ToolRetryPolicy>,
    extra_annotations: &[(&str, &str)],
) -> ToolDescriptor {
    let name = tool.name().to_string();
    let canonical_id = CanonicalToolId {
        namespace: ToolNamespace::BuiltIn,
        name,
    };

    let mut annotations = vec![
        ToolAnnotation {
            key: "read_only".to_string(),
            value: read_only.to_string(),
            source: AnnotationSource::BuiltInTrusted,
        },
        ToolAnnotation {
            key: "idempotent".to_string(),
            value: idempotent.to_string(),
            source: AnnotationSource::BuiltInTrusted,
        },
    ];

    for &(key, value) in extra_annotations {
        annotations.push(ToolAnnotation {
            key: key.to_string(),
            value: value.to_string(),
            source: AnnotationSource::BuiltInTrusted,
        });
    }

    use gestalt_core::tool::RiskLevel;
    let risk = tool.risk(&serde_json::Value::Null);
    let clearable = read_only && matches!(risk, RiskLevel::Low);
    let retention = gestalt_core::context::ToolRetention::from_clearable(idempotent, clearable);

    ToolDescriptor {
        id: canonical_id,
        description: tool.description().to_string(),
        schema: tool.schema(),
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
