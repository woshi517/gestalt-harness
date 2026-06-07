use gestalt_core::tool_descriptor::ToolDescriptor;
use gestalt_core::provider::{ProviderCapabilities, ProviderToolSchema};
use gestalt_core::tool_name_mapping::ToolNameMapping;
use sha2::{Digest, Sha256};
use crate::strict_schema::make_strict_schema;

pub struct ToolSchemaAdapter;

impl ToolSchemaAdapter {
    /// Adapt a single descriptor without catalog-wide collision
    /// resolution. Prefer `adapt_batch` whenever the full catalog is
    /// available so two canonical IDs that sanitize to the same
    /// provider-facing name still receive distinct aliases.
    pub fn adapt(
        descriptor: &ToolDescriptor,
        capabilities: &ProviderCapabilities,
    ) -> (ProviderToolSchema, ToolNameMapping) {
        let provider_name = ToolNameMapping::generate_provider_name(&descriptor.id);
        let (schema, input_schema, strict) =
            Self::build_schema_and_input(descriptor, capabilities, &provider_name);
        let mapping = Self::build_mapping(descriptor, &provider_name, input_schema, strict);
        (schema, mapping)
    }

    /// Adapt a full batch of descriptors with deterministic
    /// collision-safe provider names. Descriptors are first sorted by
    /// canonical internal ID so the resulting aliases do not depend
    /// on the order of the input slice.
    pub fn adapt_batch(
        descriptors: &[ToolDescriptor],
        capabilities: &ProviderCapabilities,
    ) -> (Vec<ProviderToolSchema>, Vec<ToolNameMapping>) {
        let mut sorted: Vec<&ToolDescriptor> = descriptors.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));

        let ids: Vec<_> = sorted.iter().map(|d| d.id.clone()).collect();
        let resolved = ToolNameMapping::resolve_provider_names(&ids);

        let mut schemas = Vec::with_capacity(sorted.len());
        let mut mappings = Vec::with_capacity(sorted.len());

        for (descriptor, (canonical_id, provider_name)) in sorted.iter().zip(resolved.iter()) {
            debug_assert_eq!(&descriptor.id, canonical_id);
            let (schema, input_schema, strict) = Self::build_schema_and_input(descriptor, capabilities, provider_name);
            schemas.push(schema.clone());
            mappings.push(Self::build_mapping(descriptor, provider_name, input_schema, strict));
        }
        (schemas, mappings)
    }

    fn build_mapping(
        descriptor: &ToolDescriptor,
        provider_name: &str,
        input_schema: Option<serde_json::Value>,
        strict: Option<bool>,
    ) -> ToolNameMapping {
        let desc_json = serde_json::to_string(descriptor).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(desc_json.as_bytes());
        let descriptor_hash = format!("{:x}", hasher.finalize());

        ToolNameMapping {
            internal_id: descriptor.id.clone(),
            provider_name: provider_name.to_string(),
            display_name: descriptor.id.name.clone(),
            descriptor_hash,
            input_schema,
            strict,
        }
    }

    fn build_schema_and_input(
        descriptor: &ToolDescriptor,
        capabilities: &ProviderCapabilities,
        provider_name: &str,
    ) -> (ProviderToolSchema, Option<serde_json::Value>, Option<bool>) {
        let mut input_schema = descriptor
            .schema
            .get("input_schema")
            .cloned()
            .unwrap_or_else(|| {
                if descriptor.schema.get("properties").is_some()
                    || descriptor.schema.get("type").is_some()
                {
                    descriptor.schema.clone()
                } else {
                    serde_json::json!({
                        "type": "object",
                        "properties": {}
                    })
                }
            });

        if let Some(obj) = input_schema.as_object_mut() {
            obj.remove("title");
        }

        let (strict, schema_to_emit) = if capabilities.supports_strict_schema {
            let strict_form = make_strict_schema(&input_schema);
            (Some(true), strict_form)
        } else {
            (None, input_schema.clone())
        };

        let provider_schema = ProviderToolSchema {
            name: provider_name.to_string(),
            description: descriptor.description.clone(),
            input_schema: schema_to_emit.clone(),
            strict,
        };

        (provider_schema, Some(schema_to_emit), strict)
    }
}
