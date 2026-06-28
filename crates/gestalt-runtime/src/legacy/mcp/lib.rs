//! `gestalt-mcp` — MCP client (stdio + HTTP SSE)
//!
//! This crate is part of the gestalt-harness workspace.
//! See the [architecture document](../../docs/gestalt-harness-architecture.md) for crate boundaries.

pub mod client;
pub mod error;
pub mod model;
pub mod registry;
pub mod transport;

pub use client::McpClient;
pub use error::McpError;
pub use model::{
    parse_mcp_call_result, McpCallResult, McpConnectionState, McpEventCallback, McpLifecycleMode,
    McpRegistryEvent, McpServerConfig, McpServerId, McpServerState, McpToolIdentity, McpToolSchema,
    McpToolSummary, McpTransportConfig,
};
pub use registry::McpRegistry;
pub use transport::McpTransport;
