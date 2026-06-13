use std::{collections::HashMap, sync::Arc};

use gestalt_core::{Tool, ToolCatalog, ToolContext, ToolError, ToolExecutionResult, ToolSchema};
use serde_json::Value;

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(ToolError::InvalidInput {
                tool_name: name,
                reason: "tool already registered".to_string(),
            });
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Result<Self, ToolError> {
        self.register(tool)?;
        Ok(self)
    }

    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        let output = tool.execute(input, ctx).await?;
        let tool_call_id = ctx
            .current_tool_call_id
            .as_deref()
            .unwrap_or_else(|| tool.name());
        output.into_execution_result(false, ctx.max_output_bytes, ctx, tool_call_id)
    }
}

#[async_trait::async_trait]
impl ToolCatalog for ToolRegistry {
    fn schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = self
            .tools
            .values()
            .map(|tool| tool.schema())
            .collect::<Vec<_>>();
        schemas.sort_by(|left, right| {
            let left_name = left.get("name").and_then(Value::as_str).unwrap_or_default();
            let right_name = right
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            left_name.cmp(right_name)
        });
        schemas
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::{RiskLevel, ToolOutput};
    use serde_json::json;
    use std::{path::PathBuf, time::Duration};

    struct MockTool;

    #[async_trait::async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "mock"
        }

        fn description(&self) -> &str {
            "mock"
        }

        fn schema(&self) -> ToolSchema {
            json!({"name": "mock"})
        }

        fn risk(&self, _input: &Value) -> RiskLevel {
            RiskLevel::Low
        }

        async fn execute(
            &self,
            _input: Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::Text {
                content: "ok".to_string(),
            })
        }
    }

    fn ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("."),
            workspace_root: None,
            timeout: Duration::from_secs(1),
            allow_network: false,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 1024,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        }
    }

    #[tokio::test]
    async fn registry_should_register_lookup_and_execute_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool)).expect("register");

        let result = registry
            .execute("mock", Value::Null, &ctx())
            .await
            .expect("execute");

        assert_eq!(result.content, "ok");
    }

    #[test]
    fn registry_should_reject_duplicate_registration() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool)).expect("register");
        let result = registry.register(Arc::new(MockTool));

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }
}
