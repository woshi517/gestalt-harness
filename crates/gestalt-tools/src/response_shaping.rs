use gestalt_core::tool::ToolExecutionResult;

pub fn shape_tool_response(tool_name: &str, result: &mut ToolExecutionResult) {
    if result.is_error {
        return;
    }

    match tool_name {
        "read" => {
            let mut prefix = String::new();
            if result.truncated {
                prefix.push_str(&format!(
                    "[Output truncated. Original: {} bytes. Full output saved to artifact: {}]\n",
                    result.original_bytes.unwrap_or(0),
                    result.artifact.as_ref().map_or("unavailable", |a| a.path.to_str().unwrap_or("")),
                ));
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        "search" => {
            let mut prefix = String::new();
            if result.truncated {
                prefix.push_str("[Search results truncated due to size limits]\n");
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        "bash" => {
            let mut prefix = String::new();
            prefix.push_str("[Execution successful]\n");
            if result.truncated {
                prefix.push_str("[Output truncated]\n");
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        "web_fetch" => {
            let mut prefix = String::new();
            prefix.push_str("[Web content fetched successfully]\n");
            if result.truncated {
                prefix.push_str("[Output truncated]\n");
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        _ => {}
    }
}
