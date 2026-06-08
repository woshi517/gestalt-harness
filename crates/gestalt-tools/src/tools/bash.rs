use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};
use gestalt_exec::{ExecRequest, ExecutionSandbox, NetworkPolicy, NoSandbox};

use crate::path::validate_child_dir;

use super::common::{parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BashInput {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Clone)]
pub struct BashTool {
    sandbox: Arc<dyn ExecutionSandbox>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            sandbox: Arc::new(NoSandbox),
        }
    }
}

impl BashTool {
    #[must_use]
    pub fn new(sandbox: Arc<dyn ExecutionSandbox>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command as a fresh subprocess."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<BashInput>(self.name(), self.description())
    }

    fn risk(&self, input: &Value) -> RiskLevel {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        classify_bash(command)
    }

    fn can_run_in_parallel(&self, input: &Value) -> bool {
        self.risk(input) == RiskLevel::Low
    }

    fn descriptor(&self) -> gestalt_core::tool_descriptor::ToolDescriptor {
        crate::builtin_descriptors::make_builtin_descriptor(
            self, false, // read_only
            false, // idempotent
            None,  // no retries
        )
    }

    fn shape_output(&self, result: &mut gestalt_core::tool::ToolExecutionResult) {
        crate::response_shaping::shape_tool_response(self.name(), result);
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<BashInput>(self.name(), input)?;
        let working_dir = validate_child_dir(input.cwd.as_deref(), ctx)?;
        let result = self
            .sandbox
            .run(ExecRequest {
                command: vec!["bash".to_string(), "-lc".to_string(), input.command],
                working_dir,
                workspace_root: ctx.workspace_root.clone(),
                env: ctx.environment.clone(),
                timeout: input.timeout_secs.map_or(ctx.timeout, Duration::from_secs),
                max_output_bytes: ctx.max_output_bytes,
                network_policy: if ctx.allow_network {
                    NetworkPolicy::Full
                } else {
                    NetworkPolicy::None
                },
                mounts: Vec::new(),
                artifact_dir: ctx.artifact_dir.clone(),
                tool_call_id: ctx.current_tool_call_id.clone(),
            })
            .await
            .map_err(|err| match err {
                gestalt_core::HarnessError::Tool(err) => err,
                other => ToolError::InvalidInput {
                    tool_name: self.name().to_string(),
                    reason: other.to_string(),
                },
            })?;

        Ok(ToolOutput::Text {
            content: result.combined_text(),
        })
    }
}

fn classify_bash(command: &str) -> RiskLevel {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.contains("rm -rf /")
        || normalized.contains("mkfs")
        || normalized.contains("dd if=")
        || normalized.contains(":(){")
        || normalized.contains("chmod 777")
    {
        return RiskLevel::Critical;
    }

    if is_secret_command(command) {
        return RiskLevel::High;
    }

    if has_shell_metacharacters(&normalized)
        || normalized.contains("/dev/tcp")
        || normalized.contains("/dev/udp")
        || normalized.contains("python -c")
        || normalized.contains("python3 -c")
        || normalized.contains("sh -c")
        || normalized.contains("bash -c")
        || starts_with_any(&normalized, &["env", "xargs", "sudo -u"])
        || normalized.contains(" env ")
        || normalized.contains(" xargs ")
        || normalized.contains(" sudo -u ")
    {
        return RiskLevel::High;
    }

    if starts_with_any(
        &normalized,
        &["sudo", "docker", "git push", "ssh", "curl", "wget"],
    ) {
        return RiskLevel::High;
    }

    if starts_with_any(
        &normalized,
        &[
            "rm",
            "mv",
            "cp",
            "mkdir",
            "cargo install",
            "npm install",
            "pnpm install",
            "yarn add",
            "pip install",
        ],
    ) {
        return RiskLevel::Medium;
    }

    if starts_with_any(
        &normalized,
        &[
            "ls",
            "cat",
            "grep",
            "rg",
            "find",
            "cargo check",
            "git status",
            "git diff",
        ],
    ) {
        return RiskLevel::Low;
    }

    RiskLevel::Medium
}

fn is_secret_command(command: &str) -> bool {
    command.split_whitespace().any(|token| {
        let token = token
            .trim_matches(|c| c == '\'' || c == '"')
            .to_ascii_lowercase();
        token.contains(".env")
            || ends_with_ignore_ascii_case(&token, ".key")
            || ends_with_ignore_ascii_case(&token, ".pem")
            || token.starts_with("secrets/")
            || token.contains("/secrets/")
            || token.contains("/secret/")
            || token.starts_with("secret.")
            || ends_with_ignore_ascii_case(&token, ".secret")
    })
}

fn ends_with_ignore_ascii_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len() && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn has_shell_metacharacters(command: &str) -> bool {
    command.chars().any(|ch| {
        matches!(
            ch,
            '>' | '<' | '|' | '&' | ';' | '`' | '$' | '\\' | '\n' | '\r'
        )
    })
}

fn starts_with_any(command: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| command == *prefix || command.starts_with(&format!("{prefix} ")))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::BashTool;
    use gestalt_core::{RiskLevel, Tool, ToolError, ToolOutput};
    use serde_json::json;

    #[tokio::test]
    async fn bash_should_use_subprocess_timeout_and_output_cap() {
        let root = temp_workspace("bash");
        let mut ctx = ctx(&root);
        ctx.max_output_bytes = 4;
        let output = BashTool::default()
            .execute(json!({"command": "printf 123456789"}), &ctx)
            .await
            .expect("bash succeeds");

        assert!(
            matches!(output, ToolOutput::Text { content } if content.contains("truncated: true"))
        );
    }

    #[test]
    fn bash_should_treat_shell_metacharacters_as_high_risk() {
        assert_eq!(
            BashTool::default().risk(&json!({"command": "cat foo.txt ; ls"})),
            RiskLevel::High
        );
        assert_eq!(
            BashTool::default().risk(&json!({"command": "cat /dev/tcp/127.0.0.1/80"})),
            RiskLevel::High
        );
        assert_eq!(
            BashTool::default().risk(&json!({"command": "grep secret docs.md"})),
            RiskLevel::Low
        );
    }

    #[tokio::test]
    async fn bash_should_restrict_cwd() {
        let root = temp_workspace("bash-cwd");
        let result = BashTool::default()
            .execute(json!({"command": "pwd", "cwd": "/tmp"}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::PathNotAllowed(_))));
    }
}
