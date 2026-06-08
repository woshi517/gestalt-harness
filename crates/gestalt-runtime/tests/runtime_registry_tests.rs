use gestalt_core::ContextStability;
use gestalt_runtime::{compute_schema_hash, compute_tool_schema_hash, RuntimeRegistry};
use serde_json::json;
use std::sync::Arc;

struct DummyContributor;

#[async_trait::async_trait]
impl gestalt_runtime::ContextContributor for DummyContributor {
    fn name(&self) -> &str {
        "dummy"
    }

    fn stability(&self) -> ContextStability {
        ContextStability::ActivationStatic
    }

    async fn contribute(
        &self,
        _workspace_root: &std::path::Path,
    ) -> Result<gestalt_core::message::Message, gestalt_runtime::RuntimeError> {
        Ok(gestalt_core::message::Message::System {
            content: "dummy".to_string(),
        })
    }
}

#[test]
fn test_registry_duplicate_checks() {
    let mut reg = RuntimeRegistry::new();

    reg.register_tool("tool1".to_string(), json!({})).unwrap();
    let res = reg.register_tool("tool1".to_string(), json!({}));
    assert!(res.is_err());
    assert!(format!("{:?}", res.err().unwrap()).contains("Duplicate tool"));

    reg.register_verifier("verifier1".to_string()).unwrap();
    let res = reg.register_verifier("verifier1".to_string());
    assert!(res.is_err());

    reg.register_hook("hook1".to_string()).unwrap();
    let res = reg.register_hook("hook1".to_string());
    assert!(res.is_err());
}

#[test]
fn test_schema_hashes() {
    let schema1 = json!({
        "name": "test_tool",
        "description": "a test tool",
    });
    let hash1 = compute_schema_hash(&schema1);
    assert!(!hash1.is_empty());

    let schemas = vec![schema1];
    let hash_all = compute_tool_schema_hash(&schemas);
    assert!(!hash_all.is_empty());
}

#[test]
fn test_context_contributor_stability_is_recorded() {
    let mut reg = RuntimeRegistry::new();
    reg.register_context_contributor("dummy".to_string(), Arc::new(DummyContributor))
        .unwrap();

    let metadata = reg.context_contributors.get("dummy").unwrap();
    assert_eq!(metadata.stability, ContextStability::ActivationStatic);

    let clone = metadata.clone();
    assert_eq!(clone.stability, ContextStability::ActivationStatic);
}

#[test]
fn test_composed_tool_catalog_sorting_and_conflicts() {
    use gestalt_core::tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema};
    use gestalt_runtime::ComposedToolCatalog;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct DummyTool {
        name: String,
    }

    #[async_trait::async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "dummy description"
        }
        fn schema(&self) -> ToolSchema {
            serde_json::from_value(serde_json::json!({
                "name": self.name.clone(),
                "description": "dummy description"
            }))
            .unwrap()
        }
        fn risk(&self, _input: &serde_json::Value) -> RiskLevel {
            RiskLevel::Low
        }
        async fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, gestalt_core::error::ToolError> {
            Ok(ToolOutput::Text {
                content: "dummy".to_string(),
            })
        }
    }

    struct BaseCatalog {
        tools: Vec<Arc<dyn Tool>>,
    }
    impl ToolCatalog for BaseCatalog {
        fn schemas(&self) -> Vec<ToolSchema> {
            self.tools.iter().map(|t| t.schema()).collect()
        }
        fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
            self.tools.iter().find(|t| t.name() == name).cloned()
        }
    }

    let base = Arc::new(BaseCatalog {
        tools: vec![
            Arc::new(DummyTool {
                name: "b_tool".to_string(),
            }),
            Arc::new(DummyTool {
                name: "a_tool".to_string(),
            }),
        ],
    });

    let mut extension_tools: BTreeMap<String, Arc<dyn Tool>> = BTreeMap::new();
    extension_tools.insert(
        "c_tool".to_string(),
        Arc::new(DummyTool {
            name: "c_tool".to_string(),
        }),
    );
    extension_tools.insert(
        "d_tool".to_string(),
        Arc::new(DummyTool {
            name: "d_tool".to_string(),
        }),
    );

    let composed = ComposedToolCatalog::new(base.clone(), extension_tools).unwrap();
    let schemas = composed.schemas();

    // Check sorting order: a_tool, b_tool, c_tool, d_tool
    assert_eq!(schemas.len(), 4);
    assert_eq!(schemas[0].get("name").unwrap().as_str().unwrap(), "a_tool");
    assert_eq!(schemas[1].get("name").unwrap().as_str().unwrap(), "b_tool");
    assert_eq!(schemas[2].get("name").unwrap().as_str().unwrap(), "c_tool");
    assert_eq!(schemas[3].get("name").unwrap().as_str().unwrap(), "d_tool");
}
