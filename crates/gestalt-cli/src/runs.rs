use gestalt_app::config::EffectiveConfig;
pub use gestalt_app::runs::{delete_run, inspect_run, list_runs, prune_runs, resolve_run_path};
use gestalt_core::HarnessError;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

/// Custom line reader that performs byte-by-byte reads.
/// If EOF is reached before a trailing newline `\n`, it seeks back to the start of the line.
/// This prevents reading partial lines when tailing a trace file that is actively being written to.
pub fn read_next_line(file: &mut fs::File, buf: &mut String) -> std::io::Result<usize> {
    buf.clear();
    let mut bytes = Vec::new();
    let mut temp = [0u8; 1];
    let start_pos = file.stream_position()?;

    loop {
        match file.read(&mut temp) {
            Ok(0) => {
                if !bytes.is_empty() {
                    if bytes.last() == Some(&b'\n') {
                        *buf = String::from_utf8(bytes)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                        return Ok(buf.len());
                    }
                    file.seek(SeekFrom::Start(start_pos))?;
                    return Ok(0);
                }
                return Ok(0);
            }
            Ok(1) => {
                let b = temp[0];
                bytes.push(b);
                if b == b'\n' {
                    *buf = String::from_utf8(bytes)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                    return Ok(buf.len());
                }
            }
            Ok(_) => unreachable!(),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// Streams new lines appended to the run's trace log in real-time.
pub fn tail_run(
    config: &EffectiveConfig,
    run_id_or_path: &str,
    format: crate::output::OutputFormat,
) -> Result<(), HarnessError> {
    let resolved_path = resolve_run_path(config, run_id_or_path)?;
    let trace_path = resolved_path.join("trace.jsonl");

    if !trace_path.exists() {
        return Err(HarnessError::Trace(gestalt_core::TraceError::WriteFailed(
            std::io::Error::new(std::io::ErrorKind::NotFound, "trace.jsonl file not found"),
        )));
    }

    let mut file = fs::File::open(&trace_path)
        .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
    let mut line = String::new();

    // Read all existing complete lines
    loop {
        match read_next_line(&mut file, &mut line) {
            Ok(0) => break,
            Ok(_) => {
                print_tailed_line(&line, format)?;
            }
            Err(e) => {
                return Err(HarnessError::Trace(gestalt_core::TraceError::WriteFailed(
                    e,
                )));
            }
        }
    }

    // Keep tailing for new complete lines
    loop {
        match read_next_line(&mut file, &mut line) {
            Ok(0) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Ok(_) => {
                print_tailed_line(&line, format)?;
            }
            Err(e) => {
                return Err(HarnessError::Trace(gestalt_core::TraceError::WriteFailed(
                    e,
                )));
            }
        }
    }
}

fn print_tailed_line(line: &str, format: crate::output::OutputFormat) -> Result<(), HarnessError> {
    let envelope = serde_json::from_str::<gestalt_trace::EventEnvelope>(line).map_err(|err| {
        HarnessError::Trace(gestalt_core::TraceError::InvalidFormat {
            line: 0,
            reason: err.to_string(),
        })
    })?;

    match format {
        crate::output::OutputFormat::Json => {
            let wrapped = crate::output::JsonEnvelope {
                schema_version: 1,
                kind: "runs.tail.event".to_string(),
                data: envelope,
            };
            println!("{}", serde_json::to_string(&wrapped).unwrap_or_default());
        }
        crate::output::OutputFormat::Text => {
            if let Some(rendered) = crate::output::render_event(&envelope.event) {
                println!("{rendered}");
            }
        }
    }
    Ok(())
}
