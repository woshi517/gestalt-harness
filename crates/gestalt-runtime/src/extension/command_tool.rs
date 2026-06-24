use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use gestalt_core::error::ToolError;
use gestalt_core::tool::{RiskLevel, Tool, ToolContext, ToolOutput, ToolSchema};
use gestalt_core::tool_descriptor::{
    AnnotationSource, CanonicalToolId, ProviderToolFormat, ToolAnnotation, ToolAnnotations,
    ToolDescriptor, ToolNamespace, ToolResponseContract,
};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::ResolvedExtensionComponent;

pub struct CommandTool {
    package_id: String,
    name: String,
    description: String,
    schema: ToolSchema,
    risk: RiskLevel,
    read_only: bool,
    idempotent: bool,
    command: String,
    args: Vec<String>,
}

impl CommandTool {
    pub fn from_component(component: &ResolvedExtensionComponent) -> crate::Result<Self> {
        let description = component.description.clone().ok_or_else(|| {
            crate::RuntimeError::Extension(format!(
                "Command tool '{}' is missing description",
                component.id.canonical_id()
            ))
        })?;
        let input_schema = component.input_schema.clone().ok_or_else(|| {
            crate::RuntimeError::Extension(format!(
                "Command tool '{}' is missing input schema",
                component.id.canonical_id()
            ))
        })?;
        let risk = component.risk.ok_or_else(|| {
            crate::RuntimeError::Extension(format!(
                "Command tool '{}' is missing risk",
                component.id.canonical_id()
            ))
        })?;
        let schema = serde_json::json!({
            "name": component.id.component_id,
            "description": description,
            "input_schema": input_schema,
        });

        Ok(Self {
            package_id: component.id.package_id.clone(),
            name: component.id.component_id.clone(),
            description,
            schema,
            risk,
            read_only: component.read_only,
            idempotent: component.idempotent,
            command: component.entrypoint.command.clone(),
            args: component.entrypoint.args.clone(),
        })
    }
}

#[async_trait]
impl Tool for CommandTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    fn risk(&self, _input: &serde_json::Value) -> RiskLevel {
        self.risk
    }

    fn descriptor(&self) -> ToolDescriptor {
        let annotations = ToolAnnotations::new(vec![
            ToolAnnotation {
                key: "read_only".to_string(),
                value: self.read_only.to_string(),
                source: AnnotationSource::ExtensionDeclared,
            },
            ToolAnnotation {
                key: "idempotent".to_string(),
                value: self.idempotent.to_string(),
                source: AnnotationSource::ExtensionDeclared,
            },
        ]);
        ToolDescriptor {
            id: CanonicalToolId {
                namespace: ToolNamespace::Extension(self.package_id.clone()),
                name: self.name.clone(),
            },
            description: self.description.clone(),
            schema: self.schema.clone(),
            risk: self.risk,
            annotations,
            response_contract: ToolResponseContract {
                format: ProviderToolFormat::Json,
                shape_rules: None,
            },
            retry_policy: None,
            retention: Some(gestalt_core::context::ToolRetention::from_clearable(
                self.idempotent,
                self.read_only && matches!(self.risk, RiskLevel::Low),
            )),
        }
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .current_dir(&ctx.working_dir)
            .env_clear()
            .envs(&ctx.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(ToolError::ExecutionFailed)?;

        if let Some(mut stdin) = child.stdin.take() {
            let mut input_bytes =
                serde_json::to_vec(&input).map_err(|err| io_error(err.to_string()))?;
            input_bytes.push(b'\n');
            stdin
                .write_all(&input_bytes)
                .await
                .map_err(ToolError::ExecutionFailed)?;
        }

        let timeout = if ctx.timeout.is_zero() {
            Duration::from_secs(60)
        } else {
            ctx.timeout
        };
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| ToolError::Timeout {
                tool_name: self.name.clone(),
                timeout_secs: timeout.as_secs(),
            })?
            .map_err(ToolError::ExecutionFailed)?;

        if output.stdout.len() > ctx.max_output_bytes {
            return Err(ToolError::OutputTooLarge {
                tool_name: self.name.clone(),
                limit: ctx.max_output_bytes,
            });
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ToolError::ExecutionFailed(io_error(format!(
                "command exited with status {}: {}",
                output.status, stderr
            ))));
        }

        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .map(|value| ToolOutput::Json { value })
            .map_err(|err| ToolError::ExecutionFailed(io_error(err.to_string())))
    }
}

fn io_error(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}
