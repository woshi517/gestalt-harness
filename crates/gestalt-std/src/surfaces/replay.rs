#![forbid(unsafe_code)]
//! Deterministic offline replay surface (`gestalt replay <session.jsonl>`).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use gestalt_kernel::{ExtensionRegistry, PolicyGate, ReplayMode, ReplayReport, SessionTree};
use gestalt_runtime::{BlobStore, ExternalReplayOptions, replay_session_tree};
use serde_json::json;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::surfaces::SurfaceExitCode;

/// Options for [`run_replay_surface`].
pub struct ReplaySurfaceOptions<'a> {
    /// Path to the recorded `SessionTree` JSONL file.
    pub session_path: &'a Path,
    /// Canonical workspace root owning `.gestalt/blobs/`.
    pub workspace_root: &'a Path,
    /// Replay mode (`Default` or `ReexecuteExternal`).
    pub mode: ReplayMode,
    /// When `true`, write JSON report output; otherwise write human-readable summary.
    pub json_mode: bool,
    /// External worker options required when `mode == ReplayMode::ReexecuteExternal`.
    pub ext_opts: Option<ExternalReplayOptions<'a>>,
}

/// Result returned by [`run_replay_surface`].
#[derive(Debug, Clone, PartialEq)]
pub struct ReplaySurfaceResult {
    /// Canonical exit code (`0` on parity, `1` on divergence/I/O error, `2` on corrupt JSONL).
    pub exit_code: SurfaceExitCode,
    /// Replay report when JSONL parsing succeeded.
    pub report: Option<ReplayReport>,
    /// Optional truncated-tail recovery warning from `SessionTree::from_jsonl_str`.
    pub warning: Option<String>,
    /// Optional error diagnostic.
    pub error: Option<String>,
}

/// Replay a recorded `SessionTree` JSONL file offline and write formatted results to `writer`.
pub async fn run_replay_surface<W: AsyncWrite + Unpin>(
    options: ReplaySurfaceOptions<'_>,
    registry: &ExtensionRegistry,
    policy_gate: &PolicyGate,
    writer: &mut W,
) -> ReplaySurfaceResult {
    let raw = match fs::read_to_string(options.session_path) {
        Ok(s) => s,
        Err(err) => {
            return ReplaySurfaceResult {
                exit_code: SurfaceExitCode::RuntimeError,
                report: None,
                warning: None,
                error: Some(format!(
                    "failed to read session '{}': {err}",
                    options.session_path.display()
                )),
            };
        }
    };

    let (tree, warning) = match SessionTree::from_jsonl_str(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            return ReplaySurfaceResult {
                exit_code: SurfaceExitCode::ConfigOrParseError,
                report: None,
                warning: None,
                error: Some(format!("corrupt session JSONL: {err}")),
            };
        }
    };

    let blob_store = BlobStore::new(options.workspace_root);
    let report = match replay_session_tree(
        &tree,
        registry,
        policy_gate,
        &blob_store,
        options.mode,
        options.ext_opts,
    )
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return ReplaySurfaceResult {
                exit_code: SurfaceExitCode::from_runtime_error(&err),
                report: None,
                warning,
                error: Some(err.to_string()),
            };
        }
    };

    let rendered = if options.json_mode {
        let payload = json!({
            "parity": report.is_parity(),
            "warning": warning,
            "report": report,
        });
        format!("{payload}\n")
    } else {
        format_human_report(&report, warning.as_deref())
    };

    let _ = writer.write_all(rendered.as_bytes()).await;
    let _ = writer.flush().await;

    let exit_code = if report.is_parity() {
        SurfaceExitCode::Success
    } else {
        SurfaceExitCode::RuntimeError
    };

    ReplaySurfaceResult {
        exit_code,
        report: Some(report),
        warning,
        error: None,
    }
}

/// Convenience wrapper running [`run_replay_surface`] against `tokio::io::stdout()`.
pub async fn run_replay_stdout(
    options: ReplaySurfaceOptions<'_>,
    registry: &ExtensionRegistry,
    policy_gate: &PolicyGate,
) -> ReplaySurfaceResult {
    let mut out = tokio::io::stdout();
    run_replay_surface(options, registry, policy_gate, &mut out).await
}

fn format_human_report(report: &ReplayReport, warning: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(warn) = warning {
        let _ = writeln!(out, "[WARN] {warn}");
    }
    let status = if report.is_parity() {
        "PASS"
    } else {
        "DIVERGENCE"
    };
    let _ = writeln!(
        out,
        "[{status}] nodes={} turns={} phases={} effects={} divergences={}",
        report.total_nodes_verified,
        report.turns_replayed,
        report.phases_replayed,
        report.effects_verified,
        report.divergences.len()
    );
    for (idx, div) in report.divergences.iter().enumerate() {
        let _ = writeln!(
            out,
            "  {}. node={} kind={:?} turn={} phase={} msg={}",
            idx + 1,
            div.node_id,
            div.kind,
            div.turn_id.as_deref().unwrap_or("-"),
            div.phase_id.as_deref().unwrap_or("-"),
            div.message
        );
        let _ = writeln!(out, "     expected={} actual={}", div.expected, div.actual);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, BudgetCap, ContextBuilder, LoopAction, LoopConfig,
        LoopContext, ModelCapabilities, SandboxPolicy, SessionSlice, StepResult,
    };
    use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
    use gestalt_runtime::{CredentialStore, HeadlessPauseHandler, RuntimeHost, RuntimeRegistry};
    use serde_json::Value;
    use std::sync::Arc;

    struct DoneLoop;
    impl AgentLoop for DoneLoop {
        fn id(&self) -> &str {
            "done"
        }
        fn digest(&self) -> &str {
            "d1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            _results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            LoopAction::Done(json!("ok"))
        }
    }
    struct DoneFactory;
    impl AgentLoopFactory for DoneFactory {
        fn id(&self) -> &str {
            "done"
        }
        fn digest(&self) -> &str {
            "d1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(DoneLoop)
        }
    }
    struct NullBuilder;
    impl ContextBuilder for NullBuilder {
        fn id(&self) -> &str {
            "default"
        }
        fn digest(&self) -> &str {
            "b1"
        }
        fn build(
            &self,
            _s: &SessionSlice,
            _a: &gestalt_kernel::ArtifactStoreView,
            _c: &ModelCapabilities,
        ) -> Value {
            Value::Null
        }
    }

    #[tokio::test]
    async fn replay_surface_verifies_parity_tampering_and_corrupt_jsonl() {
        let dir = std::env::temp_dir().join(format!("gestalt_replay_surf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        let mut reg = ExtensionRegistry::new();
        reg.loops
            .insert("done".into(), std::sync::Arc::new(DoneFactory));
        reg.builders
            .insert("default".into(), std::sync::Arc::new(NullBuilder));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
        let session_path = dir.join(".gestalt/sessions/r.jsonl");

        {
            let mut host = RuntimeHost::new(
                "r_rep".into(),
                &dir,
                session_path.clone(),
                ExtensionRegistry::new(),
                PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default()),
                rt,
                CredentialStore::default(),
                None,
                Arc::new(HeadlessPauseHandler::default()),
            )
            .expect("host");
            host.kernel_mut()
                .registry_mut()
                .loops
                .insert("done".into(), Arc::new(DoneFactory));
            host.kernel_mut()
                .registry_mut()
                .builders
                .insert("default".into(), Arc::new(NullBuilder));
            let _ = host
                .run_turn(json!("hi"), "done", &[], Default::default())
                .await
                .expect("turn");
        }

        // 1. Parity in JSON mode
        let mut out_json = Vec::new();
        let res_ok = run_replay_surface(
            ReplaySurfaceOptions {
                session_path: &session_path,
                workspace_root: &dir,
                mode: ReplayMode::Default,
                json_mode: true,
                ext_opts: None,
            },
            &reg,
            &gate,
            &mut out_json,
        )
        .await;
        assert_eq!(res_ok.exit_code, SurfaceExitCode::Success);
        let parsed: Value = serde_json::from_slice(&out_json).expect("json");
        assert_eq!(parsed["parity"], true);

        // 2. Divergent loop digest in registry -> exit code 1
        struct DriftFactory;
        impl AgentLoopFactory for DriftFactory {
            fn id(&self) -> &str {
                "done"
            }
            fn digest(&self) -> &str {
                "d_drift"
            }
            fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
                Box::new(DoneLoop)
            }
        }
        let mut reg_drift = ExtensionRegistry::new();
        reg_drift
            .loops
            .insert("done".into(), Arc::new(DriftFactory));
        reg_drift
            .builders
            .insert("default".into(), Arc::new(NullBuilder));
        let mut out_div = Vec::new();
        let res_div = run_replay_surface(
            ReplaySurfaceOptions {
                session_path: &session_path,
                workspace_root: &dir,
                mode: ReplayMode::Default,
                json_mode: false,
                ext_opts: None,
            },
            &reg_drift,
            &gate,
            &mut out_div,
        )
        .await;
        assert_eq!(res_div.exit_code, SurfaceExitCode::RuntimeError);
        assert!(
            String::from_utf8(out_div)
                .expect("utf8")
                .contains("[DIVERGENCE]")
        );

        // 3. Corrupt interior JSONL -> exit code 2
        let corrupt_path = dir.join("corrupt.jsonl");
        fs::write(&corrupt_path, "{bad json}\n{\"id\":\"x\"}\n").expect("write corrupt");
        let mut out_corrupt = Vec::new();
        let res_corrupt = run_replay_surface(
            ReplaySurfaceOptions {
                session_path: &corrupt_path,
                workspace_root: &dir,
                mode: ReplayMode::Default,
                json_mode: false,
                ext_opts: None,
            },
            &reg,
            &gate,
            &mut out_corrupt,
        )
        .await;
        assert_eq!(res_corrupt.exit_code, SurfaceExitCode::ConfigOrParseError);

        let _ = fs::remove_dir_all(&dir);
    }
}
