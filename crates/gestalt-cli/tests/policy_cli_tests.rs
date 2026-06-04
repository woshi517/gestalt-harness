use gestalt_cli::config::CliOverrides;
use gestalt_cli::policy::{explain_policy, test_policy};

#[tokio::test]
async fn test_policy_input_validation() {
    let overrides = CliOverrides::default();

    // 1. Malformed JSON should fail with JSON parsing error
    let malformed_json = "{invalid json}";
    let res = explain_policy(&overrides, "bash", malformed_json).await;
    assert!(res.is_err());

    // 2. Schema-valid but shape-invalid bash input (missing command)
    let missing_cmd_json = "{}";
    let res = explain_policy(&overrides, "bash", missing_cmd_json).await;
    if let Err(e) = res {
        assert!(e.to_string().contains("Bash tool input must contain a non-empty 'command' string"));
    } else {
        panic!("Expected error for missing command");
    }

    // 3. Schema-valid but empty command string
    let empty_cmd_json = r#"{"command": "   "}"#;
    let res = explain_policy(&overrides, "bash", empty_cmd_json).await;
    if let Err(e) = res {
        assert!(e.to_string().contains("Bash tool input must contain a non-empty 'command' string"));
    } else {
        panic!("Expected error for empty command");
    }

    // 4. Non-object bash input
    let non_object_json = r#""echo hello""#;
    let res = explain_policy(&overrides, "bash", non_object_json).await;
    if let Err(e) = res {
        assert!(e.to_string().contains("Bash tool input must be a JSON object"));
    } else {
        panic!("Expected error for non-object input");
    }

    // 5. Valid input works
    let valid_json = r#"{"command": "echo hello"}"#;
    let res = explain_policy(&overrides, "bash", valid_json).await;
    assert!(res.is_ok());
    let rep = res.unwrap();
    assert_eq!(rep.tool, "bash");

    // Same behavior for test_policy
    let res_test = test_policy(&overrides, "bash", empty_cmd_json, None).await;
    if let Err(e) = res_test {
        assert!(e.to_string().contains("Bash tool input must contain a non-empty 'command' string"));
    } else {
        panic!("Expected error for test_policy empty command");
    }

    let res_test_valid = test_policy(&overrides, "bash", valid_json, None).await;
    assert!(res_test_valid.is_ok());
}
