use std::path::Path;

use gestalt_core::TraceError;
use gestalt_runtime::ModelCatalog;
use gestalt_runtime::{aggregate_costs, CostReport};

pub fn calculate_cost(path: &Path) -> Result<CostReport, TraceError> {
    let catalog = ModelCatalog::new();
    aggregate_costs(path, |model| catalog.get(model))
}

pub fn render_cost(report: &CostReport) -> String {
    let cost = report
        .estimated_cost_usd
        .map_or_else(|| "unknown".to_string(), |value| format!("${value:.6}"));
    let mut output = vec![
        format!(
            "runs={} input_tokens={} output_tokens={}",
            report.runs, report.input_tokens, report.output_tokens
        ),
        format!("estimated_cost={cost}"),
    ];
    output.extend(
        report
            .warnings
            .iter()
            .map(|warning| format!("warning={warning}")),
    );
    output.join("\n")
}
