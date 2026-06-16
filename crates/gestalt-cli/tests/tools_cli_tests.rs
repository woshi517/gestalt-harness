use gestalt_cli::config::CliOverrides;
use gestalt_cli::tools::{inspect_tool, list_tools};

#[test]
fn test_tools_list_and_inspect() {
    let overrides = CliOverrides::default();

    // 1. List tools
    let list_rep = list_tools(&overrides).unwrap();
    assert!(!list_rep.tools.is_empty());
    assert!(list_rep.tools.iter().any(|t| t.name == "read"));

    // 2. Inspect a tool
    let inspect_rep = inspect_tool(&overrides, "read").unwrap();
    assert_eq!(inspect_rep.name, "read");
    assert!(inspect_rep.schema.is_object());

    // Assert read schema properties from CLI inspection
    let read_props = &inspect_rep.schema["input_schema"]["properties"];
    assert_eq!(read_props["start_line"]["default"], 1);
    assert_eq!(read_props["max_tokens"]["default"], 4000);

    // 3. Inspect write tool and check new fields
    let write_inspect = inspect_tool(&overrides, "write").unwrap();
    assert_eq!(write_inspect.name, "write");
    assert!(write_inspect.schema["input_schema"]["properties"].get("expected_hash").is_some());
    assert!(write_inspect.schema["input_schema"]["properties"].get("dry_run").is_some());
}
