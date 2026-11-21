#![forbid(unsafe_code)]
//! Non-interactive print (`-p`) and NDJSON (`--json`) surface.

use std::sync::Arc;

use gestalt_kernel::RunEvent;
use gestalt_runtime::{CancellationToken, HeadlessPauseHandler, RuntimeHost, TurnOutcome};
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::surfaces::{ChannelEventSink, SurfaceExitCode, extract_stream_text};

/// Configuration for [`run_print_surface`].
#[derive(Debug, Clone)]
pub struct PrintOptions {
    /// Initial user prompt payload.
    pub prompt: Value,
    /// Active preset or base loop ID.
    pub preset: String,
    /// Optional extra decorators (e.g., `["goal_check"]`).
    pub extra_decorators: Vec<String>,
    /// When `true`, emit one redacted NDJSON `RunEvent` per line; otherwise stream plain text.
    pub json_mode: bool,
    /// Pause reasons or actions explicitly auto-approved in headless mode.
    pub headless_auto_approve: Vec<String>,
}

/// Result returned by [`run_print_surface`].
#[derive(Debug, Clone, PartialEq)]
pub struct PrintResult {
    /// Canonical exit code (`0`, `1`, `2`, `3`, or `130`).
    pub exit_code: SurfaceExitCode,
    /// Final Turn outcome when the Turn ran to completion or abort.
    pub outcome: Option<TurnOutcome>,
    /// Optional error message if the Turn failed before completion.
    pub error: Option<String>,
}

/// Run a non-interactive Turn (plus any auto-promoted `FollowUp` Turns) and
/// stream text or NDJSON [`RunEvent`]s to `writer`.
pub async fn run_print_surface<W: AsyncWrite + Unpin>(
    host: &mut RuntimeHost,
    options: PrintOptions,
    writer: &mut W,
    cancel: CancellationToken,
) -> PrintResult {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<RunEvent>();
    host.set_event_sink(Some(Arc::new(ChannelEventSink::new(event_tx))));
    host.set_pause_handler(Arc::new(HeadlessPauseHandler {
        auto_approve: options.headless_auto_approve,
    }));

    let mut next_prompt = Some(options.prompt);
    let mut last_outcome = None;

    while let Some(prompt) = next_prompt.take() {
        let mut streamed_text = false;
        let mut ends_with_newline = false;

        let turn_res = {
            let mut turn_fut = std::pin::pin!(host.run_turn(
                prompt,
                &options.preset,
                &options.extra_decorators,
                cancel.clone(),
            ));

            loop {
                tokio::select! {
                    biased;
                    Some(ev) = event_rx.recv() => {
                        let _ = write_print_event(
                            writer,
                            &ev,
                            options.json_mode,
                            &mut streamed_text,
                            &mut ends_with_newline,
                        ).await;
                    }
                    res = &mut turn_fut => {
                        break res;
                    }
                }
            }
        };

        while let Ok(ev) = event_rx.try_recv() {
            let _ = write_print_event(
                writer,
                &ev,
                options.json_mode,
                &mut streamed_text,
                &mut ends_with_newline,
            )
            .await;
        }

        match turn_res {
            Ok(TurnOutcome::Completed {
                turn_id,
                output,
                usage,
            }) => {
                if !options.json_mode && !streamed_text && !output.is_null() {
                    let text = match &output {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let _ = writer.write_all(text.as_bytes()).await;
                    if !text.ends_with('\n') {
                        let _ = writer.write_all(b"\n").await;
                    }
                    let _ = writer.flush().await;
                } else if !options.json_mode && streamed_text && !ends_with_newline {
                    let _ = writer.write_all(b"\n").await;
                    let _ = writer.flush().await;
                }

                last_outcome = Some(TurnOutcome::Completed {
                    turn_id,
                    output,
                    usage,
                });

                if let Some(follow_up) = host.kernel_mut().pop_next_follow_up() {
                    next_prompt = Some(follow_up.content);
                }
            }
            Ok(TurnOutcome::Aborted { turn_id, reason }) => {
                return PrintResult {
                    exit_code: SurfaceExitCode::Aborted,
                    outcome: Some(TurnOutcome::Aborted { turn_id, reason }),
                    error: None,
                };
            }
            Err(err) => {
                let code = SurfaceExitCode::from_runtime_error(&err);
                return PrintResult {
                    exit_code: code,
                    outcome: last_outcome,
                    error: Some(err.to_string()),
                };
            }
        }
    }

    PrintResult {
        exit_code: SurfaceExitCode::Success,
        outcome: last_outcome,
        error: None,
    }
}

/// Convenience wrapper running [`run_print_surface`] against `tokio::io::stdout()`.
pub async fn run_print_stdout(
    host: &mut RuntimeHost,
    options: PrintOptions,
    cancel: CancellationToken,
) -> PrintResult {
    let mut out = tokio::io::stdout();
    run_print_surface(host, options, &mut out, cancel).await
}

async fn write_print_event<W: AsyncWrite + Unpin>(
    writer: &mut W,
    event: &RunEvent,
    json_mode: bool,
    streamed_text: &mut bool,
    ends_with_newline: &mut bool,
) -> std::io::Result<()> {
    if json_mode {
        if let Ok(line) = serde_json::to_string(event) {
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
        return Ok(());
    }

    if let RunEvent::StreamDelta {
        channel, payload, ..
    } = event
        && channel == "text"
        && let Some(txt) = extract_stream_text(payload)
    {
        *streamed_text = true;
        *ends_with_newline = txt.ends_with('\n');
        writer.write_all(txt.as_bytes()).await?;
        writer.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, BudgetCap, ContextBuilder, Effect, ExtensionRegistry,
        LoopAction, LoopConfig, LoopContext, ModelCapabilities, ModelKind, PauseReason, PolicyGate,
        SandboxPolicy, SessionSlice, StepResult,
    };
    use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
    use gestalt_runtime::{
        CredentialStore, ModelProvider, ModelRequest, ModelResponse, ProviderContext, RuntimeError,
        RuntimeRegistry, StreamDeltaFn,
    };
    use serde_json::json;
    use std::fs;
    use std::future::Future;
    use std::pin::Pin;

    struct StreamMockProvider;
    impl ModelProvider for StreamMockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        fn capabilities(&self, _model: &str) -> ModelCapabilities {
            ModelCapabilities::default()
        }
        fn invoke<'a>(
            &'a self,
            _req: &'a ModelRequest,
            _ctx: &'a ProviderContext<'a>,
            on_delta: StreamDeltaFn<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>>
        {
            Box::pin(async move {
                on_delta("text", json!({ "text": "Hello " }));
                on_delta("text", json!({ "text": "world" }));
                Ok(ModelResponse {
                    kind: ModelKind::Llm,
                    output: json!({ "text": "Hello world" }),
                    usage: Default::default(),
                })
            })
        }
    }

    struct PrintTestLoop {
        step_idx: usize,
        yield_pause: bool,
    }
    impl AgentLoop for PrintTestLoop {
        fn id(&self) -> &str {
            "print_loop"
        }
        fn digest(&self) -> &str {
            "pl1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if self.yield_pause && self.step_idx == 0 {
                self.step_idx += 1;
                return LoopAction::Yield(vec![Effect::Pause {
                    reason: PauseReason::Approval,
                    payload: json!({ "action": "promote" }),
                }]);
            }
            if results.is_none() || (self.yield_pause && self.step_idx == 1) {
                self.step_idx += 1;
                return LoopAction::Yield(vec![Effect::Model {
                    target: "mock/m1".into(),
                    kind: ModelKind::Llm,
                    input: json!({}),
                    schema: None,
                }]);
            }
            LoopAction::Done(json!("done"))
        }
    }
    struct PrintLoopFactory {
        yield_pause: bool,
    }
    impl AgentLoopFactory for PrintLoopFactory {
        fn id(&self) -> &str {
            "print_loop"
        }
        fn digest(&self) -> &str {
            "pl1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(PrintTestLoop {
                step_idx: 0,
                yield_pause: self.yield_pause,
            })
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

    fn make_host(dir: &std::path::Path, yield_pause: bool) -> RuntimeHost {
        let mut reg = ExtensionRegistry::new();
        reg.loops.insert(
            "print_loop".into(),
            Box::new(PrintLoopFactory { yield_pause }),
        );
        reg.builders.insert("default".into(), Box::new(NullBuilder));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
        rt.providers.register_provider(Arc::new(StreamMockProvider));
        RuntimeHost::new(
            "r_print".into(),
            dir,
            dir.join(".gestalt/sessions/p.jsonl"),
            reg,
            gate,
            rt,
            CredentialStore::default(),
            None,
            Arc::new(HeadlessPauseHandler::default()),
        )
        .expect("host")
    }

    #[tokio::test]
    async fn print_surface_streams_text_ndjson_and_enforces_headless_pause() {
        let dir = std::env::temp_dir().join(format!("gestalt_print_surf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        // 1. Plain text streaming
        fs::create_dir_all(dir.join("t1")).expect("mkdir1");
        let mut host1 = make_host(&dir.join("t1"), false);
        let mut buf1 = Vec::new();
        let res1 = run_print_surface(
            &mut host1,
            PrintOptions {
                prompt: json!("hi"),
                preset: "print_loop".into(),
                extra_decorators: Vec::new(),
                json_mode: false,
                headless_auto_approve: Vec::new(),
            },
            &mut buf1,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(res1.exit_code, SurfaceExitCode::Success);
        assert_eq!(String::from_utf8(buf1).expect("utf8"), "Hello world\n");

        // 2. NDJSON mode
        fs::create_dir_all(dir.join("t2")).expect("mkdir2");
        let mut host2 = make_host(&dir.join("t2"), false);
        let mut buf2 = Vec::new();
        let res2 = run_print_surface(
            &mut host2,
            PrintOptions {
                prompt: json!("hi"),
                preset: "print_loop".into(),
                extra_decorators: Vec::new(),
                json_mode: true,
                headless_auto_approve: Vec::new(),
            },
            &mut buf2,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(res2.exit_code, SurfaceExitCode::Success);
        let ndjson = String::from_utf8(buf2).expect("utf8");
        let parsed: Vec<RunEvent> = ndjson
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid RunEvent json"))
            .collect();
        assert!(
            parsed
                .iter()
                .any(|e| matches!(e, RunEvent::TurnCompleted { .. }))
        );

        // 3. Fail-closed pause -> exit code 3
        fs::create_dir_all(dir.join("t3")).expect("mkdir3");
        let mut host3 = make_host(&dir.join("t3"), true);
        let mut buf3 = Vec::new();
        let res3 = run_print_surface(
            &mut host3,
            PrintOptions {
                prompt: json!("hi"),
                preset: "print_loop".into(),
                extra_decorators: Vec::new(),
                json_mode: false,
                headless_auto_approve: Vec::new(),
            },
            &mut buf3,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(res3.exit_code, SurfaceExitCode::PolicyOrPauseDenied);

        // 4. Explicit headless_auto_approve -> exit code 0
        fs::create_dir_all(dir.join("t4")).expect("mkdir4");
        let mut host4 = make_host(&dir.join("t4"), true);
        let mut buf4 = Vec::new();
        let res4 = run_print_surface(
            &mut host4,
            PrintOptions {
                prompt: json!("hi"),
                preset: "print_loop".into(),
                extra_decorators: Vec::new(),
                json_mode: false,
                headless_auto_approve: vec!["approval".into()],
            },
            &mut buf4,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(res4.exit_code, SurfaceExitCode::Success);

        let _ = fs::remove_dir_all(&dir);
    }
}
