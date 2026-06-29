pub mod client;
pub mod error;
pub mod model;
pub mod registry;
pub mod transport;
pub mod discovery;

pub use client::McpClient;
pub use error::McpError;
pub use model::{
    parse_mcp_call_result, McpCallResult, McpConnectionState, McpEventCallback, McpLifecycleMode,
    McpRegistryEvent, McpServerConfig, McpServerId, McpServerState, McpToolIdentity, McpToolSchema,
    McpToolSummary, McpTransportConfig,
};
pub use registry::McpRegistry;
pub use transport::McpTransport;
pub use discovery::{GetToolDetailsTool, McpDiscoveryState, SearchToolsTool};

use async_trait::async_trait;
use gestalt_core::error::ToolError;
use gestalt_core::tool::{RiskLevel, Tool, ToolContext, ToolOutput, ToolSchema};
use gestalt_core::tool_descriptor::{
    CanonicalToolId, ProviderToolFormat, ToolAnnotations, ToolDescriptor, ToolNamespace,
    ToolResponseContract,
};
use serde_json::Value;
use std::sync::Arc;

pub struct McpBackedTool {
    registry: Arc<McpRegistry>,
    server_name: String,
    tool_name: String,
    description: String,
    schema: ToolSchema,
    risk: RiskLevel,
    annotations: ToolAnnotations,
    event_bus: Option<crate::event_bus::RuntimeEventBus>,
}

impl McpBackedTool {
    pub fn new(
        registry: Arc<McpRegistry>,
        server_name: String,
        tool_name: String,
        description: String,
        schema: ToolSchema,
        trust_level: Option<&str>,
        event_bus: Option<crate::event_bus::RuntimeEventBus>,
    ) -> Self {
        let mut tool_ann_list = Vec::new();
        if let Some(ann) = registry.get_tool_annotations(&server_name, &tool_name) {
            for (k, v) in ann {
                tool_ann_list.push(gestalt_core::tool_descriptor::ToolAnnotation {
                    key: k,
                    value: v,
                    source: gestalt_core::tool_descriptor::AnnotationSource::BuiltInTrusted,
                });
            }
        }
        let annotations = ToolAnnotations::new(tool_ann_list);
        let risk = calculate_risk(&tool_name, &description, trust_level, &annotations);
        Self {
            registry,
            server_name,
            tool_name,
            description,
            schema,
            risk,
            annotations,
            event_bus,
        }
    }
}

fn calculate_risk(
    name: &str,
    description: &str,
    trust_level: Option<&str>,
    annotations: &ToolAnnotations,
) -> RiskLevel {
    let name_lower = name.to_lowercase();
    let desc_lower = description.to_lowercase();

    let is_high = name_lower.contains("write")
        || name_lower.contains("delete")
        || name_lower.contains("remove")
        || name_lower.contains("execute")
        || name_lower.contains("run")
        || name_lower.contains("bash")
        || name_lower.contains("shell")
        || name_lower.contains("cmd")
        || name_lower.contains("fetch")
        || name_lower.contains("post")
        || name_lower.contains("send")
        || name_lower.contains("request")
        || name_lower.contains("http")
        || desc_lower.contains("write")
        || desc_lower.contains("delete")
        || desc_lower.contains("filesystem")
        || desc_lower.contains("network")
        || desc_lower.contains("shell")
        || desc_lower.contains("execute");

    if is_high {
        RiskLevel::High
    } else {
        if let Some(tl) = trust_level {
            if tl.eq_ignore_ascii_case("high") {
                let is_read_only = annotations.get("read_only").map_or(false, |a| {
                    a.value == "true"
                        && a.source
                            == gestalt_core::tool_descriptor::AnnotationSource::BuiltInTrusted
                });
                let is_idempotent = annotations.get("idempotent").map_or(false, |a| {
                    a.value == "true"
                        && a.source
                            == gestalt_core::tool_descriptor::AnnotationSource::BuiltInTrusted
                });
                if is_read_only || is_idempotent {
                    return RiskLevel::Low;
                }
            }
        }
        RiskLevel::Medium
    }
}

#[async_trait]
impl Tool for McpBackedTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        self.risk
    }

    fn descriptor(&self) -> ToolDescriptor {
        let canonical_id = CanonicalToolId {
            namespace: ToolNamespace::Mcp(self.server_name.clone()),
            name: self.tool_name.clone(),
        };

        let read_only = self.annotations.get_trusted_bool("read_only");
        let idempotent = self.annotations.get_trusted_bool("idempotent");
        let clearable = read_only && matches!(self.risk, RiskLevel::Low);
        let retention = gestalt_core::context::ToolRetention::from_clearable(idempotent, clearable);

        ToolDescriptor {
            id: canonical_id,
            description: self.description.clone(),
            schema: self.schema.clone(),
            risk: self.risk,
            annotations: self.annotations.clone(),
            response_contract: ToolResponseContract {
                format: ProviderToolFormat::Text,
                shape_rules: None,
            },
            retry_policy: None,
            retention: Some(retention),
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let call_id = format!("call-{}", uuid::Uuid::new_v4());
        let start_time = std::time::Instant::now();

        if let Some(ref event_bus) = self.event_bus {
            event_bus.publish(crate::event_bus::RuntimeEvent::McpToolCallStarted {
                server_name: self.server_name.clone(),
                tool_name: self.tool_name.clone(),
                call_id: call_id.clone(),
            });
        }

        let res = self
            .registry
            .call_tool(&self.server_name, &self.tool_name, input)
            .await;

        let success = match &res {
            Ok(r) => !r.is_error,
            Err(_) => false,
        };

        if let Some(ref event_bus) = self.event_bus {
            event_bus.publish(crate::event_bus::RuntimeEvent::McpToolCallCompleted {
                server_name: self.server_name.clone(),
                tool_name: self.tool_name.clone(),
                call_id: call_id.clone(),
                success,
                duration_ms: u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX),
            });
        }

        let res =
            res.map_err(|e| ToolError::ExecutionFailed(std::io::Error::other(e.to_string())))?;

        if res.is_error {
            // Re-render execution failure so policy/approvals see the error
            return Err(ToolError::ExecutionFailed(std::io::Error::other(
                res.content,
            )));
        }

        Ok(ToolOutput::Text {
            content: res.content,
        })
    }
}
