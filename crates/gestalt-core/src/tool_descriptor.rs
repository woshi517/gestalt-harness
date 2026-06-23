use crate::tool::{RiskLevel, ToolSchema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum ToolNamespace {
    BuiltIn,
    Extension(String),
    Mcp(String),
}

impl std::fmt::Display for ToolNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuiltIn => write!(f, "builtin"),
            Self::Extension(id) => write!(f, "extension:{}", id),
            Self::Mcp(id) => write!(f, "mcp:{}", id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanonicalToolId {
    pub namespace: ToolNamespace,
    pub name: String,
}

impl std::fmt::Display for CanonicalToolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.namespace {
            ToolNamespace::BuiltIn => write!(f, "builtin:{}", self.name),
            ToolNamespace::Extension(id) => write!(f, "extension:{}:{}", id, self.name),
            ToolNamespace::Mcp(id) => write!(f, "mcp:{}:{}", id, self.name),
        }
    }
}

impl std::str::FromStr for CanonicalToolId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.is_empty() {
            return Err("Empty canonical ID".to_string());
        }
        match parts[0] {
            "builtin" => {
                if parts.len() != 2 {
                    return Err(format!("Invalid builtin ID format: {}", s));
                }
                Ok(Self {
                    namespace: ToolNamespace::BuiltIn,
                    name: parts[1].to_string(),
                })
            }
            "extension" => {
                if parts.len() != 3 {
                    return Err(format!("Invalid extension ID format: {}", s));
                }
                Ok(Self {
                    namespace: ToolNamespace::Extension(parts[1].to_string()),
                    name: parts[2].to_string(),
                })
            }
            "mcp" => {
                if parts.len() != 3 {
                    return Err(format!("Invalid mcp ID format: {}", s));
                }
                Ok(Self {
                    namespace: ToolNamespace::Mcp(parts[1].to_string()),
                    name: parts[2].to_string(),
                })
            }
            _ => Err(format!("Unknown namespace prefix: {}", parts[0])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationSource {
    BuiltInTrusted,
    ExtensionDeclared,
    McpDeclared,
    UserOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotation {
    pub key: String,
    pub value: String,
    pub source: AnnotationSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    pub annotations: Vec<ToolAnnotation>,
}

impl ToolAnnotations {
    pub fn new(annotations: Vec<ToolAnnotation>) -> Self {
        Self { annotations }
    }

    pub fn get(&self, key: &str) -> Option<&ToolAnnotation> {
        self.annotations.iter().find(|a| a.key == key)
    }

    pub fn get_trusted_bool(&self, key: &str) -> bool {
        self.get(key)
            .is_some_and(|a| a.source == AnnotationSource::BuiltInTrusted && a.value == "true")
    }

    pub fn get_bool_advisory(&self, key: &str) -> bool {
        self.get(key).is_some_and(|a| a.value == "true")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderToolFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseShapeRules {
    pub summarize_output: bool,
    pub output_format_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResponseContract {
    pub format: ProviderToolFormat,
    pub shape_rules: Option<ResponseShapeRules>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRetryPolicy {
    pub max_retries: usize,
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub id: CanonicalToolId,
    pub description: String,
    pub schema: ToolSchema,
    pub risk: RiskLevel,
    pub annotations: ToolAnnotations,
    pub response_contract: ToolResponseContract,
    pub retry_policy: Option<ToolRetryPolicy>,
    #[serde(default)]
    pub retention: Option<crate::context::ToolRetention>,
}
