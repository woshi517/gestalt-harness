use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    Transport(String),
    Protocol(String),
    Timeout(String),
    Initialization(String),
    Execution(String),
    Config(String),
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "MCP Transport Error: {}", msg),
            Self::Protocol(msg) => write!(f, "MCP Protocol Error: {}", msg),
            Self::Timeout(msg) => write!(f, "MCP Timeout: {}", msg),
            Self::Initialization(msg) => write!(f, "MCP Initialization Failed: {}", msg),
            Self::Execution(msg) => write!(f, "MCP Execution Failed: {}", msg),
            Self::Config(msg) => write!(f, "MCP Config Error: {}", msg),
        }
    }
}

impl std::error::Error for McpError {}

pub type Result<T> = std::result::Result<T, McpError>;
