use super::{BashTool, FindFilesTool, PatchTool, ReadTool, SearchTool, WebFetchTool, WriteTool};

use std::sync::Arc;

use gestalt_core::ToolError;

pub fn default_registry() -> Result<super::ToolRegistry, ToolError> {
    let mut registry = super::ToolRegistry::new();
    registry.register(Arc::new(ReadTool))?;
    registry.register(Arc::new(SearchTool))?;
    registry.register(Arc::new(FindFilesTool))?;
    registry.register(Arc::new(WriteTool))?;
    registry.register(Arc::new(PatchTool))?;
    registry.register(Arc::new(BashTool::default()))?;
    registry.register(Arc::new(WebFetchTool::default()))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::ToolCatalog;

    #[test]
    fn schemas_should_include_public_contracts() {
        let registry = default_registry().expect("registry builds");
        let schemas = registry.schemas();

        assert_eq!(schemas.len(), 7);

        // 1. Assert read tool schema details
        let read_schema = schemas
            .iter()
            .find(|s| s["name"] == "read")
            .expect("has read schema");
        let read_props = &read_schema["input_schema"]["properties"];
        assert_eq!(read_props["start_line"]["default"], 1);
        assert_eq!(read_props["max_tokens"]["default"], 4000);
        assert!(read_props["start_line"]["description"]
            .as_str()
            .unwrap()
            .contains("1-indexed"));
        assert!(read_props["end_line"]["default"].is_null()); // Optional field without default value

        // 2. Assert search tool schema details
        let search_schema = schemas
            .iter()
            .find(|s| s["name"] == "search")
            .expect("has search schema");
        let search_props = &search_schema["input_schema"]["properties"];
        assert_eq!(search_props["max_results"]["default"], 100);
        assert_eq!(search_props["respect_gitignore"]["default"], true);
        assert_eq!(search_props["case_insensitive"]["type"], "boolean");
        assert!(search_props["case_insensitive"]["description"]
            .as_str()
            .unwrap()
            .contains("case-insensitive"));

        // 3. Assert find_files tool schema details
        let find_schema = schemas
            .iter()
            .find(|s| s["name"] == "find_files")
            .expect("has find_files schema");
        let find_props = &find_schema["input_schema"]["properties"];
        assert_eq!(find_props["max_results"]["default"], 50);
        assert_eq!(find_props["respect_gitignore"]["default"], true);

        // 4. Assert write tool schema details
        let write_schema = schemas
            .iter()
            .find(|s| s["name"] == "write")
            .expect("has write schema");
        let write_props = &write_schema["input_schema"]["properties"];
        assert!(write_props.get("expected_hash").is_some());
        assert!(write_props.get("dry_run").is_some());
        assert_eq!(write_props["dry_run"]["type"], "boolean");

        // 5. Assert patch tool schema details
        let patch_schema = schemas
            .iter()
            .find(|s| s["name"] == "patch")
            .expect("has patch schema");
        let patch_props = &patch_schema["input_schema"]["properties"];
        assert!(patch_props.get("expected_hash").is_some());
        assert!(patch_props.get("dry_run").is_some());
        assert_eq!(patch_props["dry_run"]["type"], "boolean");
    }
}
