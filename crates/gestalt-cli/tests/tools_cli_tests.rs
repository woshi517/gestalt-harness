use gestalt_cli::config::CliOverrides;
use gestalt_cli::tools::{list_tools, inspect_tool};

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
}
