use std::path::Path;

use gestalt_core::TraceError;
use gestalt_runtime::unstable::{read_trace, render_display};

/// Replays and formats a trace log for display.
/// Accepts either the path to a run directory containing a `trace.jsonl` file or the path to a trace file directly.
pub fn replay_display(path: &Path) -> Result<String, TraceError> {
    let trace_path = if path.is_dir() {
        path.join("trace.jsonl")
    } else {
        path.to_path_buf()
    };
    let events = read_trace(&trace_path)?;
    Ok(render_display(&events))
}
