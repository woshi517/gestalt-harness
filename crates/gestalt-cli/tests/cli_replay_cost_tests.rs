use std::fs;

use gestalt_cli::{
    cost::{calculate_cost, render_cost},
    replay::replay_display,
};

fn read_fixture(path: &str) -> String {
    fs::read_to_string(format!("../../tests/fixtures/{path}")).expect("fixture exists")
}

#[test]
fn replay_matches_golden() {
    let rendered = replay_display(std::path::Path::new(
        "../../tests/fixtures/traces/minimal-run.jsonl",
    ))
    .expect("replay works");
    assert_eq!(
        rendered,
        read_fixture("cli-golden/replay-display.txt").trim_end()
    );
}

#[test]
fn cost_matches_golden() {
    let rendered = render_cost(
        &calculate_cost(std::path::Path::new(
            "../../tests/fixtures/traces/minimal-run.jsonl",
        ))
        .expect("cost works"),
    );
    assert_eq!(
        rendered,
        read_fixture("cli-golden/cost-single-run.txt").trim_end()
    );
}
