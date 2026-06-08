use crate::{
    tool::ToolCatalog,
    tool_failure::{ToolErrorReport, ToolFailureKind},
    tool_name_mapping::ToolNameMapping,
    turn::ProposedToolCall,
};
use serde_json::Value;
use std::collections::HashSet;

pub struct ToolCallValidator;

impl ToolCallValidator {
    pub fn validate(
        call: &ProposedToolCall,
        catalog: &dyn ToolCatalog,
        name_mappings: &[ToolNameMapping],
        seen_ids: &mut HashSet<String>,
    ) -> Result<(), ToolErrorReport> {
        // 1. Check for duplicate tool-call IDs
        if !seen_ids.insert(call.id.clone()) {
            return Err(ToolErrorReport {
                kind: ToolFailureKind::DuplicateCallId,
                message: format!("Duplicate tool call ID: {}", call.id),
                repair_guidance: Some(
                    "Use unique IDs for each tool call in the same turn.".to_string(),
                ),
            });
        }

        // 2. Resolve provider name to canonical ID.
        //    The name_mappings are the authoritative list of tools
        //    exposed to the provider this turn. A tool call whose
        //    provider name is not in the mappings is rejected
        //    immediately — no fallback to canonical‑ID parsing or
        //    the full catalog, which would bypass the planner's
        //    active‑tool‑set boundary (U4).
        let mapping = name_mappings.iter().find(|m| m.provider_name == call.name);

        let canonical_id = match mapping {
            Some(m) => m.internal_id.clone(),
            None => {
                return Err(ToolErrorReport {
                    kind: ToolFailureKind::ToolNotFound,
                    message: format!(
                        "Tool '{}' is not in the active tool set for this turn",
                        call.name
                    ),
                    repair_guidance: Some(
                        "Use only tools from the list provided to you. \
                         If a tool you need is missing, ask the user to enable it."
                            .to_string(),
                    ),
                });
            }
        };

        // 3. Namespace checking
        // (disallowed namespace exposure if namespace is not supported or trusted)
        // Wait, for now let's just make sure the tool exists in catalog
        let tool = match catalog.get_by_id(&canonical_id) {
            Some(t) => t,
            None => {
                return Err(ToolErrorReport {
                    kind: ToolFailureKind::ToolNotFound,
                    message: format!("Tool not found: {}", call.name),
                    repair_guidance: Some(format!(
                        "The tool '{}' is not registered. Please check the spelling or ensure it is loaded.",
                        call.name
                    )),
                });
            }
        };

        let descriptor = tool.descriptor();

        // 4. Validate input JSON structure / types
        let input_schema = descriptor
            .schema
            .get("input_schema")
            .unwrap_or(&descriptor.schema);
        if let Err(err_msg) = validate_json_value(&call.input, input_schema) {
            return Err(ToolErrorReport {
                kind: ToolFailureKind::SchemaMismatch,
                message: format!(
                    "Schema validation failed for tool '{}': {}",
                    call.name, err_msg
                ),
                repair_guidance: Some(format!(
                    "The arguments provided do not match the expected schema. Expected: {}",
                    serde_json::to_string(input_schema).unwrap_or_default()
                )),
            });
        }

        Ok(())
    }

    /// Validate against a provider-rendered strict schema. This is
    /// stricter than the raw descriptor check: the strict schema
    /// adds `additionalProperties: false`, promotes optional fields
    /// to nullable, and may flatten `$ref`s. Returning an `Err` here
    /// means the model has produced JSON that the provider would
    /// reject (or, for non-strict providers, that the strict-rendered
    /// version of the same schema would have rejected).
    pub fn validate_against_strict(input: &Value, strict_schema: &Value) -> Result<(), String> {
        validate_json_value(input, strict_schema)
    }
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches_single(value_type: &str, schema_type: &str) -> bool {
    match schema_type {
        "null" => matches!(value_type, "null"),
        "object" => matches!(value_type, "object"),
        "array" => matches!(value_type, "array"),
        "string" => matches!(value_type, "string"),
        "number" => matches!(value_type, "number"),
        "integer" => matches!(value_type, "number"),
        "boolean" => matches!(value_type, "boolean"),
        _ => true,
    }
}

fn type_matches(value: &Value, schema_type: &Value) -> bool {
    match schema_type.as_str() {
        Some(single) => type_matches_single(value_type_name(value), single),
        None => schema_type.as_array().map_or(true, |types| {
            types.iter().any(|t| {
                t.as_str()
                    .map_or(true, |s| type_matches_single(value_type_name(value), s))
            })
        }),
    }
}

fn validate_json_value(value: &Value, schema: &Value) -> Result<(), String> {
    if let Some(schema_obj) = schema.as_object() {
        // anyOf: value is valid if it satisfies at least one sub-schema.
        // This handles the nullable forms emitted by make_schema_nullable
        // (e.g. `{"anyOf": [{"type":"number"}, {"type":"null"}]}`).
        if let Some(any_of) = schema_obj.get("anyOf").and_then(|a| a.as_array()) {
            let mut errors = Vec::new();
            for sub in any_of {
                match validate_json_value(value, sub) {
                    Ok(()) => return Ok(()),
                    Err(e) => errors.push(e),
                }
            }
            return Err(format!(
                "none of the anyOf alternatives matched: {}",
                errors.join("; ")
            ));
        }

        if let Some(types) = schema_obj.get("type") {
            if !type_matches(value, types) {
                let type_label = match types.as_str() {
                    Some(s) => s.to_string(),
                    None => types.as_array().map_or_else(
                        || "unknown".to_string(),
                        |arr| {
                            let names: Vec<_> = arr.iter().filter_map(|v| v.as_str()).collect();
                            names.join(" or ")
                        },
                    ),
                };
                return Err(format!(
                    "Expected {}, found {}",
                    type_label,
                    value_type_name(value)
                ));
            }

            match types.as_str() {
                Some("object") => {
                    let val_obj = value.as_object().unwrap();
                    if let Some(properties) =
                        schema_obj.get("properties").and_then(|p| p.as_object())
                    {
                        for (prop_name, prop_schema) in properties {
                            if let Some(prop_val) = val_obj.get(prop_name) {
                                if let Err(e) = validate_json_value(prop_val, prop_schema) {
                                    return Err(format!("field '{}': {}", prop_name, e));
                                }
                            }
                        }
                    }
                    if let Some(required) = schema_obj.get("required").and_then(|r| r.as_array()) {
                        for req_field in required {
                            if let Some(field_str) = req_field.as_str() {
                                if !val_obj.contains_key(field_str) {
                                    return Err(format!("Missing required field: '{}'", field_str));
                                }
                            }
                        }
                    }
                    if let Some(additional) = schema_obj.get("additionalProperties") {
                        if let Some(false) = additional.as_bool() {
                            if let Some(properties) =
                                schema_obj.get("properties").and_then(|p| p.as_object())
                            {
                                for key in val_obj.keys() {
                                    if !properties.contains_key(key) {
                                        return Err(format!(
                                            "Additional property '{}' not allowed",
                                            key
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                Some("array") => {
                    let val_arr = value.as_array().unwrap();
                    if let Some(items) = schema_obj.get("items") {
                        for (idx, item_val) in val_arr.iter().enumerate() {
                            if let Err(e) = validate_json_value(item_val, items) {
                                return Err(format!("index {}: {}", idx, e));
                            }
                        }
                    }
                }
                _ => {
                    // Scalar type check already handled by type_matches above;
                    // nothing more to recurse into.
                }
            }
        }
    }
    Ok(())
}
