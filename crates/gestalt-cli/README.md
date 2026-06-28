# Gestalt CLI Crate (`gestalt-cli`)

Binary entry point and command surface for the `gestalt` CLI tool. Local-first AI agent harness for safe, inspectable single-agent work.

---

## Commands

### Execution

| Command | Description |
|---------|-------------|
| `gestalt run <prompt>` | Execute a single prompt in a fresh session |
| `gestalt chat` | Start interactive chat mode |
| `gestalt tui` | Launch the separately installed `gestalt-tui` binary |

### Inspection & Analysis

| Command | Description |
|---------|-------------|
| `gestalt trace replay <path>` | Replay a trace file as a readable transcript |
| `gestalt trace inspect <path>` | Show trace metadata (events, timestamps, provider info) |
| `gestalt trace validate <path>` | Verify trace integrity against golden fixtures |
| `gestalt trace analyze <path-or-id> [--kind tools]` | Analyze tool metrics from a trace |
| `gestalt tools list` | List all registered tools |
| `gestalt tools inspect <name>` | Show a tool's JSON schema |
| `gestalt tools classify bash <command>` | Classify a bash command's risk level |
| `gestalt runtime inspect` | Show runtime configuration and registered capabilities |
| `gestalt runtime events` | Show runtime event history |
| `gestalt runtime doctor` | Diagnose runtime configuration issues |
| `gestalt policy validate` | Inspect and test policy configuration |
| `gestalt context explain` | Show what context the agent sees |

### Sessions

| Command | Description |
|---------|-------------|
| `gestalt sessions list` | List all sessions |
| `gestalt sessions inspect <id>` | Inspect a session's metadata and lineage |
| `gestalt sessions continue <id>` | Continue a session |
| `gestalt sessions resume <id>` | Resume a session from its last checkpoint |
| `gestalt sessions branch <id>` | Fork a new session from an existing one |

### Runs & Export

| Command | Description |
|---------|-------------|
| `gestalt runs list` | List all runs |
| `gestalt runs inspect <id>` | Inspect a run's details |
| `gestalt runs tail <id>` | Stream a running session's events |
| `gestalt runs prune` | Clean up old runs |
| `gestalt runs delete <id>` | Delete a run |
| `gestalt export <id> [--format markdown\|jsonl\|sharegpt]` | Export a run |
| `gestalt verify run` | Run artifact verification |
| `gestalt replay <path>` | Replay a trace |
| `gestalt cost <path>` | Calculate run token cost |

### Configuration

| Command | Description |
|---------|-------------|
| `gestalt config validate` | Validate configuration |
| `gestalt config show` | Show current configuration |
| `gestalt config explain` | Explain configuration resolution |
| `gestalt auth resolve <provider>` | Show resolved credentials for a provider |
| `gestalt auth doctor` | Diagnose auth configuration |
| `gestalt providers list` | List configured providers |
| `gestalt providers inspect <name>` | Inspect a provider's configuration |
| `gestalt providers test <name>` | Test a provider connection |
| `gestalt providers doctor` | Diagnose provider configuration |
| `gestalt models list` | List available models |
| `gestalt models inspect <name>` | Inspect model details |
| `gestalt models refresh` | Refresh model catalog |
| `gestalt models search <query>` | Search for models |
| `gestalt profiles list` | List configuration profiles |
| `gestalt profiles inspect <name>` | Inspect a profile |
| `gestalt profiles use <name>` | Set active profile |
| `gestalt init` | Initialize workspace |
| `gestalt status` | Show workspace status |
| `gestalt workspace info | snapshot | doctor` | Workspace management |

### Extensions

| Command | Description |
|---------|-------------|
| `gestalt extension list` | List discovered extensions |
| `gestalt extension inspect <id>` | Inspect an extension's manifest |
| `gestalt extension enable <id>` | Enable an extension |
| `gestalt extension disable <id>` | Disable an extension |
| `gestalt extension validate <path>` | Validate an extension manifest |
| `gestalt extension reload` | Reload extensions |

---

## Trace Analysis (`gestalt trace analyze`)

`gestalt trace analyze` computes tool metrics from a trace JSONL file:

```
$ gestalt trace analyze path/to/trace --tools

Total Proposed Calls        12
  Complete Turns             8
  Validation Failures         2
Invalid Tool Call Rate      16.7%

Policy / Approval
  Policy Decisions Evaluated 10
  Policy Denied               1
  Approval Requests           1
  Approval Denied             0

Truncation
  Truncated Results           1
  Truncation Rate            10.0%

Success Rate
  First-call Successes        7
  First-call Retries          1
  First-call Success Rate    87.5%
  Total Executed Calls        8

Tool Catalog Exposure
  Unique Tools Exposed       6
  Turn Count                  8
  Avg Tools per Turn         6.0
  Max Tools per Turn         6

Token Cost
  Total Tokens             15,234
  Total Cost (est.)         $0.03
```

The `--tools` flag is an alias for `--kind tools`. Future analyzer kinds (cost, retries) can be added via `--kind`.

---

## Output Formats

All commands support `--format text` (default) and `--format json`. JSON output includes the same structured data as the text renderings, suitable for piping into `jq` or other tools.

---

## Runtime Construction

The CLI builds the runtime pipeline in `gestalt_app::runtime_factory::build_cli_runtime()`:

```rust
pub async fn build_cli_runtime(
    workspace: &Path,
    effective: &EffectiveConfig,
    overrides: &CliOverrides,
    trace_sink: Option<Arc<dyn TraceSink>>,
) -> Result<AgentRuntime>
```

This assembles:
- Tool registry (`gestalt_runtime::default_registry()`)
- Provider adapter (resolved from `gestalt.json` providers and model catalog)
- Context pipeline (`gestalt-runtime::context`)
- Policy engine (`gestalt-runtime::policy`, driven from the `policies` key in `gestalt.json`)
- Approval provider (CLI interactive or yolo auto-approve)
- Trace sink (filesystem JSONL writer)
- Extension discovery and loading
- Verification hooks

Extension trust is configured via `extensions.trusted` in `gestalt.json`. Trusted extension IDs are wired to `extension_trust::set_trusted_extension_ids()` before extension descriptors are built, ensuring the trust gate is active before any tool is registered.
