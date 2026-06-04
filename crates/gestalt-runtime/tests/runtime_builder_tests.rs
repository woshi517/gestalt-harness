use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};
use gestalt_core::session::ExecutionMode;

#[test]
fn test_runtime_config_defaults() {
    let config = RuntimeConfig::default();
    assert_eq!(config.execution_mode, ExecutionMode::Confirm);
    assert!(config.max_turns > 0);
}

#[test]
fn test_builder_missing_dependencies() {
    let builder = AgentRuntimeBuilder::new();
    let res = builder.build();
    assert!(res.is_err());
    let err_str = format!("{:?}", res.err().unwrap());
    assert!(err_str.contains("Missing provider") || err_str.contains("Builder"));
}

#[test]
fn test_builder_zero_max_turns() {
    let mut config = RuntimeConfig::default();
    config.max_turns = 0;
    let builder = AgentRuntimeBuilder::new().config(config);
    let res = builder.build();
    assert!(res.is_err());
    let err_str = format!("{:?}", res.err().unwrap());
    assert!(err_str.contains("max_turns must be positive"));
}
