use gestalt_app::reports::{ConnectReport, RunIndexEntry, RunsListReport};
use serde::Serialize;

#[test]
fn app_reports_serialize_without_cli_types() {
    fn assert_serializable<T: Serialize>() {}

    assert_serializable::<ConnectReport>();
    assert_serializable::<RunIndexEntry>();
    assert_serializable::<RunsListReport>();
}
