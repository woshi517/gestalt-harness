use gestalt_core::error::ToolError;
use gestalt_core::tool::{RiskLevel, Tool, ToolContext, ToolOutput, ToolSchema};
use gestalt_core::tool_descriptor::{
    CanonicalToolId, ProviderToolFormat, ToolAnnotations, ToolDescriptor, ToolNamespace,
    ToolResponseContract,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct McpDiscoveryState {
    pub selected_tools: Vec<String>, // canonical IDs or provider names of selected tools
}

impl McpDiscoveryState {
    pub fn new() -> Self {
        Self {
            selected_tools: Vec::new(),
        }
    }
}

pub fn rank_tools(
    query: &str,
    tools: &[(CanonicalToolId, String, String)], // (canonical_id, provider_name, description)
) -> Vec<(CanonicalToolId, String, String)> {
    let query_lower = query.to_lowercase();
    let query_tokens: Vec<&str> = query_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    let mut ranked: Vec<(i32, &(CanonicalToolId, String, String))> = tools
        .iter()
        .map(|item| {
            let (ref id, ref provider_name, ref desc) = *item;
            let mut score = 0;

            let name_lower = provider_name.to_lowercase();
            let canon_name_lower = id.name.to_lowercase();

            // 1. Exact name match
            if name_lower == query_lower || canon_name_lower == query_lower {
                score += 1000;
            }

            // Tokenize tool name
            let name_tokens: Vec<&str> = name_lower
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .filter(|s| !s.is_empty())
                .collect();

            // 2. Prefix or name-token match
            for q_tok in &query_tokens {
                for n_tok in &name_tokens {
                    if n_tok == q_tok {
                        score += 100;
                    } else if n_tok.starts_with(q_tok) {
                        score += 10;
                    }
                }
            }

            // 3. Description token overlap
            let desc_lower = desc.to_lowercase();
            let desc_tokens: Vec<&str> = desc_lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect();
            for q_tok in &query_tokens {
                for d_tok in &desc_tokens {
                    if d_tok == q_tok {
                        score += 1;
                    }
                }
            }

            (score, item)
        })
        .filter(|(score, _)| *score > 0)
        .collect();

    ranked.sort_by(|a, b| {
        if b.0 == a.0 {
            a.1 .0.to_string().cmp(&b.1 .0.to_string())
        } else {
            b.0.cmp(&a.0)
        }
    });

    ranked.into_iter().map(|(_, item)| item.clone()).collect()
}

pub struct SearchToolsTool {
    registry: Arc<gestalt_mcp::McpRegistry>,
    schema: ToolSchema,
}

impl SearchToolsTool {
    pub fn new(registry: Arc<gestalt_mcp::McpRegistry>) -> Self {
        let schema = serde_json::json!({
            "name": "search_tools",
            "description": "Search for available tools by keyword or description query.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The query term or keywords to search for in tool names and descriptions."
                    }
                },
                "required": ["query"]
            }
        });
        Self { registry, schema }
    }
}

#[async_trait::async_trait]
impl Tool for SearchToolsTool {
    fn name(&self) -> &str {
        "search_tools"
    }

    fn description(&self) -> &str {
        "Search for available tools by keyword or description query."
    }

    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: CanonicalToolId {
                namespace: ToolNamespace::BuiltIn,
                name: "search_tools".to_string(),
            },
            description: self.description().to_string(),
            schema: self.schema(),
            risk: RiskLevel::Low,
            annotations: ToolAnnotations::default(),
            response_contract: ToolResponseContract {
                format: ProviderToolFormat::Text,
                shape_rules: None,
            },
            retry_policy: None,
            retention: Some(gestalt_core::context::ToolRetention {
                clearable: true,
                reconstructible: true,
                retain_errors: true,
            }),
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let query = input.get("query").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::ExecutionFailed(std::io::Error::other("Missing query parameter"))
        })?;

        let mut mcp_tools = self.registry.list_all_tools().await.map_err(|e| {
            ToolError::ExecutionFailed(std::io::Error::other(format!(
                "Failed to list MCP tools: {}",
                e
            )))
        })?;

        // Sort by canonical ID string representation to ensure deterministic pool order
        mcp_tools.sort_by(|(s_a, t_a), (s_b, t_b)| {
            let canon_a = format!("mcp:{}:{}", s_a.0, t_a.name);
            let canon_b = format!("mcp:{}:{}", s_b.0, t_b.name);
            canon_a.cmp(&canon_b)
        });

        let search_pool: Vec<(CanonicalToolId, String, String)> = mcp_tools
            .into_iter()
            .map(|(server_id, schema)| {
                let canonical_id = CanonicalToolId {
                    namespace: ToolNamespace::Mcp(server_id.0.clone()),
                    name: schema.name.clone(),
                };
                let provider_name =
                    gestalt_core::tool_name_mapping::ToolNameMapping::generate_provider_name(
                        &canonical_id,
                    );
                (canonical_id, provider_name, schema.description)
            })
            .collect();

        let ranked = rank_tools(query, &search_pool);

        let mut results = Vec::new();
        for (canonical_id, provider_name, description) in ranked {
            results.push(serde_json::json!({
                "name": provider_name,
                "canonical_id": canonical_id.to_string(),
                "description": description
            }));
        }

        Ok(ToolOutput::Text {
            content: serde_json::to_string_pretty(&results).unwrap_or_default(),
        })
    }
}

pub struct GetToolDetailsTool {
    registry: Arc<gestalt_mcp::McpRegistry>,
    discovery_state: Arc<Mutex<McpDiscoveryState>>,
    schema: ToolSchema,
}

impl GetToolDetailsTool {
    pub fn new(
        registry: Arc<gestalt_mcp::McpRegistry>,
        discovery_state: Arc<Mutex<McpDiscoveryState>>,
    ) -> Self {
        let schema = serde_json::json!({
            "name": "get_tool_details",
            "description": "Inspect the detailed schema and arguments for a specific tool.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The name or canonical ID of the tool to inspect."
                    }
                },
                "required": ["name"]
            }
        });
        Self {
            registry,
            discovery_state,
            schema,
        }
    }
}

#[async_trait::async_trait]
impl Tool for GetToolDetailsTool {
    fn name(&self) -> &str {
        "get_tool_details"
    }

    fn description(&self) -> &str {
        "Inspect the detailed schema and arguments for a specific tool."
    }

    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: CanonicalToolId {
                namespace: ToolNamespace::BuiltIn,
                name: "get_tool_details".to_string(),
            },
            description: self.description().to_string(),
            schema: self.schema(),
            risk: RiskLevel::Low,
            annotations: ToolAnnotations::default(),
            response_contract: ToolResponseContract {
                format: ProviderToolFormat::Text,
                shape_rules: None,
            },
            retry_policy: None,
            retention: Some(gestalt_core::context::ToolRetention {
                clearable: true,
                reconstructible: true,
                retain_errors: true,
            }),
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = input.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::ExecutionFailed(std::io::Error::other("Missing name parameter"))
        })?;

        let mut mcp_tools = self.registry.list_all_tools().await.map_err(|e| {
            ToolError::ExecutionFailed(std::io::Error::other(format!(
                "Failed to list MCP tools: {}",
                e
            )))
        })?;

        // Sort by canonical ID string representation to ensure deterministic order
        mcp_tools.sort_by(|(s_a, t_a), (s_b, t_b)| {
            let canon_a = format!("mcp:{}:{}", s_a.0, t_a.name);
            let canon_b = format!("mcp:{}:{}", s_b.0, t_b.name);
            canon_a.cmp(&canon_b)
        });

        // Find the matching tool
        for (server_id, schema) in mcp_tools {
            let canonical_id = CanonicalToolId {
                namespace: ToolNamespace::Mcp(server_id.0.clone()),
                name: schema.name.clone(),
            };
            let provider_name =
                gestalt_core::tool_name_mapping::ToolNameMapping::generate_provider_name(
                    &canonical_id,
                );

            if provider_name == name || canonical_id.to_string() == name || schema.name == name {
                // Add to selected working set
                let mut state = self.discovery_state.lock().unwrap();
                let canonical_id_str = canonical_id.to_string();
                if !state.selected_tools.contains(&canonical_id_str) {
                    state.selected_tools.push(canonical_id_str);
                }
                if !state.selected_tools.contains(&provider_name) {
                    state.selected_tools.push(provider_name.clone());
                }

                // Return details
                let details = serde_json::json!({
                    "name": provider_name,
                    "canonical_id": canonical_id.to_string(),
                    "description": schema.description,
                    "input_schema": schema.input_schema
                });

                return Ok(ToolOutput::Text {
                    content: serde_json::to_string_pretty(&details).unwrap_or_default(),
                });
            }
        }

        Err(ToolError::ExecutionFailed(std::io::Error::other(format!(
            "Tool '{}' not found in available MCP tool catalog",
            name
        ))))
    }
}
