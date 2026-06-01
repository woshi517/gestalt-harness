use std::fs;

use gestalt_models::ModelCatalog;
use gestalt_trace::{aggregate_costs, read_trace, render_display};

fn read_fixture(path: &str) -> String {
    fs::read_to_string(format!("../../tests/fixtures/{path}")).expect("fixture exists")
}

#[test]
fn replay_display_matches_golden_fixture() {
    let events = read_trace("../../tests/fixtures/traces/minimal-run.jsonl").expect("trace reads");
    let rendered = render_display(&events);
    assert_eq!(
        rendered,
        read_fixture("cli-golden/replay-display.txt").trim_end()
    );
}

#[test]
fn cost_aggregation_matches_golden_fixture() {
    let catalog = ModelCatalog::new();
    let report = aggregate_costs("../../tests/fixtures/traces/minimal-run.jsonl", |model| {
        catalog.get(model)
    })
    .expect("cost aggregates");
    let rendered = format!(
        "runs={} input_tokens={} output_tokens={}\nestimated_cost=${:.6}",
        report.runs,
        report.input_tokens,
        report.output_tokens,
        report.estimated_cost_usd.expect("cost")
    );
    assert_eq!(
        rendered,
        read_fixture("cli-golden/cost-single-run.txt").trim_end()
    );
}
