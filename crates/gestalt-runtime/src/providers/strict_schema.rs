use serde_json::{json, Value};

pub fn make_strict_schema(schema: &Value) -> Value {
    let mut strict = schema.clone();
    transform_to_strict(&mut strict);
    strict
}

fn transform_to_strict(val: &mut Value) {
    if let Some(obj) = val.as_object_mut() {
        // Recurse into properties
        if let Some(properties) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
            for (_, prop_val) in properties {
                transform_to_strict(prop_val);
            }
        }

        // Recurse into array items
        if let Some(items) = obj.get_mut("items") {
            transform_to_strict(items);
        }

        // If it's an object type
        let is_object = obj.get("type").and_then(|t| t.as_str()) == Some("object")
            || obj.contains_key("properties");
        if is_object {
            obj.insert("additionalProperties".to_string(), Value::Bool(false));

            // Collect all properties
            let mut all_props = Vec::new();
            if let Some(properties) = obj.get("properties").and_then(|p| p.as_object()) {
                for prop_name in properties.keys() {
                    all_props.push(prop_name.clone());
                }
            }
            // Sort to ensure deterministic ordering of required fields
            all_props.sort();

            // Get existing required fields
            let mut required_set = std::collections::HashSet::new();
            if let Some(required) = obj.get("required").and_then(|r| r.as_array()) {
                for req in required {
                    if let Some(s) = req.as_str() {
                        required_set.insert(s.to_string());
                    }
                }
            }

            // Update required list to include all properties
            let mut required_arr = Vec::new();
            for prop in &all_props {
                required_arr.push(Value::String(prop.clone()));

                // If not originally required, make it nullable
                if !required_set.contains(prop) {
                    if let Some(properties) =
                        obj.get_mut("properties").and_then(|p| p.as_object_mut())
                    {
                        if let Some(prop_schema) = properties.get_mut(prop) {
                            make_schema_nullable(prop_schema);
                        }
                    }
                }
            }
            obj.insert("required".to_string(), Value::Array(required_arr));
        }
    }
}

fn make_schema_nullable(schema: &mut Value) {
    if let Some(obj) = schema.as_object_mut() {
        if let Some(t) = obj.get_mut("type") {
            if let Some(type_str) = t.as_str() {
                if type_str != "null" {
                    *t = json!([type_str, "null"]);
                }
            } else if let Some(type_arr) = t.as_array_mut() {
                let has_null = type_arr.iter().any(|v| v.as_str() == Some("null"));
                if !has_null {
                    type_arr.push(Value::String("null".to_string()));
                }
            }
        } else if obj.contains_key("anyOf") {
            if let Some(any_of) = obj.get_mut("anyOf").and_then(|a| a.as_array_mut()) {
                let has_null = any_of
                    .iter()
                    .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("null"));
                if !has_null {
                    any_of.push(json!({ "type": "null" }));
                }
            }
        } else {
            let original = obj.clone();
            obj.clear();
            obj.insert(
                "anyOf".to_string(),
                json!([
                    original,
                    { "type": "null" }
                ]),
            );
        }
    }
}
