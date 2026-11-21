#![forbid(unsafe_code)]
//! Milestone 1.4 System One Triage 50-task benchmark suite.

use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use gestalt_kernel::{
    BudgetCap, Extension, ExtensionRegistry, ModelCapabilities, ModelKind, PolicyGate,
    ReplayEngine, ReplayMode, Usage,
};
use gestalt_runtime::{
    BlobStore, CancellationToken, CredentialStore, HeadlessPauseHandler, ModelProvider,
    ModelRequest, ModelResponse, ProviderContext, ProviderRegistry, RuntimeError, RuntimeHost,
    RuntimeRegistry, StreamDeltaFn, TurnOutcome, default_sandbox_driver,
};
use gestalt_std::{
    DecisionOutput, LlmOutput, StdExtension, StdToolsExtension, ToolCallRequest,
    default_std_sandbox_policy, register_std_tools,
};
use serde_json::{Value, json};

struct BenchmarkDecisionProvider;

impl ModelProvider for BenchmarkDecisionProvider {
    fn id(&self) -> &str {
        "jev"
    }

    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities::default()
    }

    fn invoke<'a>(
        &'a self,
        req: &'a ModelRequest,
        _ctx: &'a ProviderContext<'a>,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let prompt_text = req
                .input
                .get("state")
                .and_then(|s| s.get("prompt"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();

            let decision = if prompt_text.contains("fast_read:") {
                DecisionOutput {
                    choice: "fast_read".to_string(),
                    confidence: 0.96,
                    latency_ms: 8,
                }
            } else if prompt_text.contains("fast_glob:") {
                DecisionOutput {
                    choice: "fast_glob".to_string(),
                    confidence: 0.95,
                    latency_ms: 7,
                }
            } else if prompt_text.contains("fast_grep:") {
                DecisionOutput {
                    choice: "fast_grep".to_string(),
                    confidence: 0.94,
                    latency_ms: 9,
                }
            } else {
                DecisionOutput {
                    choice: "escalate".to_string(),
                    confidence: 0.35,
                    latency_ms: 10,
                }
            };

            Ok(ModelResponse {
                kind: ModelKind::Decision,
                output: serde_json::to_value(decision).expect("serialize decision"),
                usage: Usage {
                    input_units: 45,
                    output_units: 18,
                    cost_micro_usd: 12,
                },
            })
        })
    }
}

struct BenchmarkLlmProvider;

impl ModelProvider for BenchmarkLlmProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities {
            max_context_tokens: 200_000,
            supports_prompt_caching: true,
            ..ModelCapabilities::default()
        }
    }

    fn invoke<'a>(
        &'a self,
        req: &'a ModelRequest,
        _ctx: &'a ProviderContext<'a>,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let messages = req
                .input
                .get("messages")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let output = if messages <= 1 {
                LlmOutput {
                    text: String::new(),
                    reasoning: Vec::new(),
                    tool_calls: vec![ToolCallRequest {
                        id: "tc_1".to_string(),
                        name: "read".to_string(),
                        args: json!({ "path": "src/lib.rs" }),
                    }],
                    stop_reason: Some("tool_use".to_string()),
                }
            } else {
                LlmOutput {
                    text: "Completed multi-step analysis.".to_string(),
                    reasoning: Vec::new(),
                    tool_calls: Vec::new(),
                    stop_reason: Some("end_turn".to_string()),
                }
            };
            Ok(ModelResponse {
                kind: ModelKind::Llm,
                output: serde_json::to_value(output).expect("serialize llm output"),
                usage: Usage {
                    input_units: 950,
                    output_units: 180,
                    cost_micro_usd: 4_200,
                },
            })
        })
    }
}

fn build_benchmark_host(ws: &Path, session_path: &Path) -> RuntimeHost {
    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("register std");
    StdToolsExtension
        .register(&mut reg)
        .expect("register tools");

    if let Some(triage) = reg.presets.get_mut("triage") {
        triage.config.params = json!({
            "decision_model": "jev/jev-1",
            "confidence_threshold": 0.85,
            "fallback_preset": "general",
            "routes": {
                "fast_read": { "action": "tool", "name": "read", "args": { "path": "src/lib.rs" } },
                "fast_glob": { "action": "tool", "name": "glob", "args": { "pattern": "src/*.rs" } },
                "fast_grep": { "action": "tool", "name": "grep", "args": { "pattern": "benchmark_target" } },
            }
        });
    }
    if let Some(general) = reg.presets.get_mut("general") {
        general.config.params["model"] = json!("anthropic/claude-sonnet-4-6");
    }

    let gate = PolicyGate::new(BudgetCap::default(), None, default_std_sandbox_policy());
    let mut providers = ProviderRegistry::new();
    providers.register_provider(Arc::new(BenchmarkDecisionProvider));
    providers.register_provider(Arc::new(BenchmarkLlmProvider));

    let mut runtime = RuntimeRegistry::new(default_sandbox_driver(true));
    runtime.providers = providers;
    register_std_tools(&mut runtime);

    let creds =
        CredentialStore::load_from_path(ws.join(".gestalt/credentials.toml")).expect("load creds");
    RuntimeHost::new(
        "bench_run".to_string(),
        ws,
        session_path.to_path_buf(),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host")
}

#[tokio::test]
async fn triage_benchmark_50_tasks_meets_accuracy_latency_cost_and_replay_gates() {
    let ws = std::env::temp_dir().join(format!("gestalt_triage_bench_{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(ws.join("src")).expect("mkdir src");
    fs::create_dir_all(ws.join(".gestalt/sessions")).expect("mkdir sessions");
    fs::write(
        ws.join("src/lib.rs"),
        "pub fn benchmark_target() -> i32 { 42 }\n",
    )
    .expect("write lib.rs");

    let mut tasks: Vec<(String, bool)> = Vec::with_capacity(50);
    for i in 0..25 {
        let prompt = match i % 3 {
            0 => format!("fast_read: inspect lib.rs #{i}"),
            1 => format!("fast_glob: list rust files #{i}"),
            _ => format!("fast_grep: locate benchmark_target #{i}"),
        };
        tasks.push((prompt, false));
    }
    for i in 0..25 {
        tasks.push((
            format!("complex_refactor: redesign module architecture across workspace #{i}"),
            true,
        ));
    }

    let mut correct_routes = 0usize;
    let mut triage_latencies_ms = Vec::with_capacity(50);
    let mut triage_total_tokens = 0u64;
    let mut triage_total_cost = 0u64;
    let mut baseline_total_tokens = 0u64;
    let mut baseline_total_cost = 0u64;

    for (idx, (prompt, expected_escalate)) in tasks.iter().enumerate() {
        let triage_session = ws.join(format!(".gestalt/sessions/triage_{idx}.jsonl"));
        let mut triage_host = build_benchmark_host(&ws, &triage_session);
        let t0 = Instant::now();
        let outcome = triage_host
            .run_turn(json!(prompt), "triage", &[], CancellationToken::new())
            .await
            .expect("triage turn");
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        triage_latencies_ms.push(elapsed_ms);

        let TurnOutcome::Completed { usage, .. } = outcome else {
            panic!("task {idx} did not complete");
        };
        triage_total_tokens += usage.input_units + usage.output_units;
        triage_total_cost += usage.cost_micro_usd;

        let phase_count = triage_host
            .kernel()
            .session_tree()
            .all_nodes()
            .iter()
            .filter(|n| {
                matches!(
                    n.decode_payload(),
                    Ok(gestalt_kernel::NodePayload::PhaseOpen { .. })
                )
            })
            .count();
        let escalated = phase_count >= 2;
        if escalated == *expected_escalate {
            correct_routes += 1;
        }

        let blobs = BlobStore::new(&ws);
        let report = ReplayEngine::new(
            triage_host.kernel().registry(),
            triage_host.kernel().policy_gate(),
        )
        .replay(
            triage_host.kernel().session_tree(),
            ReplayMode::Default,
            |d| blobs.get_utf8(d).ok(),
        )
        .expect("replay triage trace");
        assert!(
            report.divergences.is_empty(),
            "task {idx} replay diverged: {:?}",
            report.divergences
        );

        let base_session = ws.join(format!(".gestalt/sessions/baseline_{idx}.jsonl"));
        let mut base_host = build_benchmark_host(&ws, &base_session);
        let base_outcome = base_host
            .run_turn(json!(prompt), "general", &[], CancellationToken::new())
            .await
            .expect("baseline turn");
        let TurnOutcome::Completed {
            usage: base_usage, ..
        } = base_outcome
        else {
            panic!("baseline task {idx} did not complete");
        };
        baseline_total_tokens += base_usage.input_units + base_usage.output_units;
        baseline_total_cost += base_usage.cost_micro_usd;
    }

    triage_latencies_ms.sort_by(f64::total_cmp);
    let median_latency_ms = triage_latencies_ms[triage_latencies_ms.len() / 2];
    let accuracy = (correct_routes as f64) / (tasks.len() as f64);
    let token_savings = 1.0 - (triage_total_tokens as f64) / (baseline_total_tokens as f64);
    let cost_savings = 1.0 - (triage_total_cost as f64) / (baseline_total_cost as f64);

    assert!(accuracy >= 0.85, "routing accuracy {accuracy:.3} < 0.85");
    assert!(
        median_latency_ms <= 150.0,
        "median triage latency {median_latency_ms:.2}ms > 150ms"
    );
    assert!(
        token_savings >= 0.30,
        "token savings {token_savings:.3} < 0.30"
    );
    assert!(
        cost_savings >= 0.30,
        "cost savings {cost_savings:.3} < 0.30"
    );

    let _ = fs::remove_dir_all(&ws);
}
