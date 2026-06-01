use std::path::Path;

use gestalt_core::TraceError;
use gestalt_trace::{read_trace, render_display};

pub fn replay_display(path: &Path) -> Result<String, TraceError> {
    let events = read_trace(path)?;
    Ok(render_display(&events))
}
