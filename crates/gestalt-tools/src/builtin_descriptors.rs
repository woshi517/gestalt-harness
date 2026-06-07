use gestalt_core::tool::Tool;
use gestalt_core::tool_descriptor::{
    ToolDescriptor, CanonicalToolId, ToolNamespace, ToolAnnotations, ToolAnnotation,
    AnnotationSource, ToolResponseContract, ProviderToolFormat, ToolRetryPolicy
};

pub fn make_builtin_descriptor(
    tool: &dyn Tool,
    read_only: bool,
    idempotent: bool,
    retry_policy: Option<ToolRetryPolicy>,
) -> ToolDescriptor {
    let name = tool.name().to_string();
    let canonical_id = CanonicalToolId {
        namespace: ToolNamespace::BuiltIn,
        name,
    };
    
    let annotations = vec![
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

    ToolDescriptor {
        id: canonical_id,
        description: tool.description().to_string(),
        schema: tool.schema(),
        risk: tool.risk(&serde_json::Value::Null),
        annotations: ToolAnnotations::new(annotations),
        response_contract: ToolResponseContract {
            format: ProviderToolFormat::Text,
            shape_rules: None,
        },
        retry_policy,
    }
}
