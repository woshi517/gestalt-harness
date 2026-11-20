#![forbid(unsafe_code)]
//! Advisory `.lock` file management (with stale-PID recovery), crash-safe
//! JSONL session persistence, and `RunEvent` redaction helpers.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gestalt_kernel::{RunEvent, SessionTree};

use crate::RuntimeError;
use crate::credentials::StreamRedactor;

/// RAII advisory lock guard holding `<session_jsonl_path>.lock` exclusively.
///
/// Automatically cleans up stale locks left behind by crashed processes (where
/// the recorded PID is no longer alive) and removes the lock file on drop.
#[derive(Debug)]
pub struct SessionLockGuard {
    lock_path: PathBuf,
}

impl SessionLockGuard {
    /// Acquire `<session_jsonl_path>.lock` atomically (`create_new`), reclaiming
    /// stale locks from dead PIDs and failing with [`RuntimeError::SessionLocked`]
    /// if another live process holds it.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::SessionLocked`] on live lock contention or [`RuntimeError::Io`].
    pub fn acquire(session_jsonl_path: &Path) -> Result<Self, RuntimeError> {
        if let Some(parent) = session_jsonl_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = PathBuf::from(format!("{}.lock", session_jsonl_path.display()));

        for attempt in 0..2 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut f) => {
                    let _ = writeln!(
                        f,
                        "pid={} acquired_ms={}",
                        std::process::id(),
                        current_epoch_ms()
                    );
                    let _ = f.flush();
                    return Ok(Self { lock_path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = fs::read_to_string(&lock_path)
                        .unwrap_or_else(|_| "unknown".to_string())
                        .trim()
                        .to_string();
                    if attempt == 0 && is_stale_lock(&holder) {
                        tracing::warn!(
                            lock = %lock_path.display(),
                            holder = %holder,
                            "reclaiming stale session lock from dead process"
                        );
                        let _ = fs::remove_file(&lock_path);
                        continue;
                    }
                    return Err(RuntimeError::SessionLocked {
                        lock_path: lock_path.display().to_string(),
                        holder,
                    });
                }
                Err(err) => return Err(err.into()),
            }
        }

        Err(RuntimeError::SessionLocked {
            lock_path: lock_path.display().to_string(),
            holder: "concurrent contention".to_string(),
        })
    }

    /// Return the underlying lock file path.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// Load and validate an existing JSONL `SessionTree` file. If a truncated
/// trailing line was discarded during crash recovery, rewrite the file with
/// only the intact verified lines so subsequent appends never corrupt an
/// interior line.
///
/// # Errors
///
/// Returns [`RuntimeError`] on I/O failure or interior `SessionTree` corruption.
pub(crate) fn load_and_repair_session_file(
    session_path: &Path,
) -> Result<(SessionTree, File), RuntimeError> {
    let raw = fs::read_to_string(session_path)?;
    let (tree, warning) = SessionTree::from_jsonl_str(&raw)?;
    if let Some(warn_msg) = warning {
        tracing::warn!(session = %session_path.display(), "{warn_msg}; truncating partial tail");
        let mut repaired = String::new();
        for node in tree.all_nodes() {
            repaired.push_str(&node.to_jsonl_line()?);
            repaired.push('\n');
        }
        let tmp_path = session_path.with_extension(format!("jsonl.repair.{}", std::process::id()));
        fs::write(&tmp_path, repaired.as_bytes())?;
        fs::rename(&tmp_path, session_path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(session_path)?;
    Ok((tree, file))
}

/// Flush redacted compact JSONL lines to `session_file` and sync to disk.
pub(crate) fn flush_jsonl_lines(
    session_file: &mut File,
    lines: Vec<String>,
    redactor: &StreamRedactor,
) -> Result<(), RuntimeError> {
    if lines.is_empty() {
        return Ok(());
    }
    for line in lines {
        let safe = redactor.redact_str(&line);
        writeln!(session_file, "{safe}")?;
    }
    session_file.flush()?;
    session_file.sync_data()?;
    Ok(())
}

/// Scrub any known host secrets from a [`RunEvent`] before forwarding.
pub(crate) fn redact_run_event(event: &mut RunEvent, redactor: &StreamRedactor) {
    match event {
        RunEvent::PolicyDenied { reason, .. } | RunEvent::TurnAborted { reason, .. } => {
            *reason = redactor.redact_str(reason);
        }
        RunEvent::Paused { payload, .. }
        | RunEvent::TurnCompleted {
            output: payload, ..
        }
        | RunEvent::StreamDelta { payload, .. }
        | RunEvent::ToolUi { data: payload, .. } => {
            redactor.redact_json(payload);
        }
        RunEvent::NodeAppended { .. } | RunEvent::PhaseSwitched { .. } => {}
    }
}

/// Return current wall-clock time in milliseconds since Unix epoch.
#[must_use]
pub fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn is_stale_lock(holder: &str) -> bool {
    let Some(pid_str) = holder
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("pid="))
    else {
        return false;
    };
    let Ok(pid) = pid_str.parse::<u32>() else {
        return false;
    };
    !is_process_alive(pid)
}

#[cfg(target_os = "linux")]
fn is_process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn stale_lock_from_dead_pid_is_reclaimed() {
        let dir = std::env::temp_dir().join(format!("gestalt_lock_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let session = dir.join("s.jsonl");
        let lock_file = PathBuf::from(format!("{}.lock", session.display()));
        // PID 999_999_999 is guaranteed non-existent on Unix
        fs::write(&lock_file, "pid=999999999 acquired_ms=1\n").expect("write stale lock");

        let guard = SessionLockGuard::acquire(&session).expect("reclaim stale lock");
        assert!(guard.lock_path().exists());
        drop(guard);
        assert!(!lock_file.exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
