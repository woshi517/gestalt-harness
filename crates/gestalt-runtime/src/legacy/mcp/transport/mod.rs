use crate::mcp_error::Result;
use async_trait::async_trait;
use serde_json::Value;

pub mod stdio;

#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a request to the server and wait for a response.
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value>;

    /// Send a notification to the server.
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()>;

    /// Retrieve the next notification received from the server.
    async fn recv_notification(&self) -> Option<(String, Option<Value>)>;

    /// Gracefully shutdown or kill the connection.
    async fn shutdown(&self);
}
