use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::client::McpClient;
use crate::error::{McpError, Result};
use crate::model::{
    McpCallResult, McpConnectionState, McpServerConfig, McpServerId, McpServerState, McpToolSchema,
};

#[allow(clippy::type_complexity)]
pub struct McpRegistry {
    workspace_root: PathBuf,
    configs: HashMap<String, McpServerConfig>,
    clients: Arc<
        Mutex<
            HashMap<
                String,
                Arc<tokio::sync::OnceCell<std::result::Result<Arc<McpClient>, McpError>>>,
            >,
        >,
    >,
    failures: Arc<Mutex<HashMap<String, String>>>, // server_name -> error_msg
    event_callback: Arc<std::sync::Mutex<Option<crate::model::McpEventCallback>>>,
    permission_validator: Arc<
        std::sync::RwLock<
            Option<
                Arc<
                    dyn Fn(&str, &McpServerConfig) -> std::result::Result<(), String> + Send + Sync,
                >,
            >,
        >,
    >,
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRegistry")
            .field("workspace_root", &self.workspace_root)
            .field("configs", &self.configs)
            .finish_non_exhaustive()
    }
}

impl McpRegistry {
    pub fn new(workspace_root: PathBuf, configs: HashMap<String, McpServerConfig>) -> Self {
        Self {
            workspace_root,
            configs,
            clients: Arc::new(Mutex::new(HashMap::new())),
            failures: Arc::new(Mutex::new(HashMap::new())),
            event_callback: Arc::new(std::sync::Mutex::new(None)),
            permission_validator: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn set_event_callback(&self, callback: crate::model::McpEventCallback) {
        if let Ok(mut lock) = self.event_callback.lock() {
            *lock = Some(callback);
        }
    }

    pub fn set_permission_validator<F>(&self, validator: F)
    where
        F: Fn(&str, &McpServerConfig) -> std::result::Result<(), String> + Send + Sync + 'static,
    {
        if let Ok(mut lock) = self.permission_validator.write() {
            *lock = Some(Arc::new(validator));
        }
    }

    pub async fn get_client(&self, name: &str) -> Result<Arc<McpClient>> {
        {
            let failures = self.failures.lock().await;
            if let Some(err) = failures.get(name) {
                return Err(McpError::Initialization(format!(
                    "Server '{}' is marked unavailable: {}",
                    name, err
                )));
            }
        }

        if !self.configs.contains_key(name) {
            return Err(McpError::Config(format!(
                "MCP Server '{}' not found in config",
                name
            )));
        }

        {
            let validator = self.permission_validator.read().unwrap();
            if let Some(ref validator_fn) = *validator {
                let config = self.configs.get(name).unwrap();
                if let Err(e) = validator_fn(name, config) {
                    return Err(McpError::Initialization(format!(
                        "Permission denied for MCP server '{}': {}",
                        name, e
                    )));
                }
            }
        }

        let cell = {
            let mut clients = self.clients.lock().await;
            if let Some(cell) = clients.get(name) {
                cell.clone()
            } else {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                clients.insert(name.to_string(), cell.clone());
                cell
            }
        };

        let cb = if let Ok(lock) = self.event_callback.lock() {
            lock.clone()
        } else {
            None
        };

        let name_clone = name.to_string();
        let result = cell
            .get_or_init(|| {
                let config = self.configs.get(name).unwrap().clone();
                let workspace_root = self.workspace_root.clone();
                let name_for_init = name_clone.clone();
                let cb_for_init = cb.clone();
                async move {
                    if let Some(ref handler) = cb_for_init {
                        handler(crate::model::McpRegistryEvent::Connecting {
                            server_name: name_for_init.clone(),
                        });
                    }
                    match McpClient::connect(config, &workspace_root, cb_for_init.clone()).await {
                        Ok(client) => {
                            let tool_count = client.get_cached_tools().map_or(0, |t| t.len());
                            if let Some(ref handler) = cb_for_init {
                                handler(crate::model::McpRegistryEvent::Connected {
                                    server_name: name_for_init.clone(),
                                    protocol_version: "2024-11-05".to_string(),
                                    tool_count,
                                });
                            }
                            Ok(Arc::new(client))
                        }
                        Err(e) => {
                            if let Some(ref handler) = cb_for_init {
                                handler(crate::model::McpRegistryEvent::ConnectionFailed {
                                    server_name: name_for_init.clone(),
                                    reason: e.to_string(),
                                });
                            }
                            Err(e)
                        }
                    }
                }
            })
            .await;

        match result {
            Ok(client) => Ok(client.clone()),
            Err(e) => {
                let err_msg = e.to_string();
                let mut failures = self.failures.lock().await;
                failures.insert(name.to_string(), err_msg.clone());
                Err(e.clone())
            }
        }
    }

    pub fn get_all_states(&self, discovery_threshold: usize) -> Vec<McpServerState> {
        let mut states = Vec::new();

        let clients = self.clients.try_lock();
        let failures = self.failures.try_lock();

        let mut total_tools = 0;
        let mut server_info = Vec::new();

        for (name, config) in &self.configs {
            let server_id = McpServerId(name.clone());
            let (conn_state, tool_count, cache_fresh, last_error) =
                if let Ok(ref failures) = failures {
                    if let Some(err) = failures.get(name) {
                        (McpConnectionState::Failed, 0, false, Some(err.clone()))
                    } else if let Ok(ref clients) = clients {
                        if let Some(cell) = clients.get(name) {
                            match cell.get() {
                                Some(Ok(client)) => {
                                    let tools = client.get_cached_tools().unwrap_or_default();
                                    let is_fresh = client.get_cached_tools().is_some();
                                    (McpConnectionState::Connected, tools.len(), is_fresh, None)
                                }
                                Some(Err(e)) => {
                                    (McpConnectionState::Failed, 0, false, Some(e.to_string()))
                                }
                                None => (McpConnectionState::Connecting, 0, false, None),
                            }
                        } else {
                            (McpConnectionState::Disconnected, 0, false, None)
                        }
                    } else {
                        (McpConnectionState::Disconnected, 0, false, None)
                    }
                } else {
                    (McpConnectionState::Disconnected, 0, false, None)
                };

            total_tools += tool_count;
            server_info.push((
                server_id,
                conn_state,
                tool_count,
                cache_fresh,
                last_error,
                config.trust_level.clone(),
            ));
        }

        let discovery_mode = total_tools > discovery_threshold;

        for (server_id, conn_state, tool_count, cache_fresh, last_error, trust_level) in server_info
        {
            states.push(McpServerState {
                server_id,
                connection_state: conn_state,
                tool_count,
                trust_level,
                discovery_mode,
                cache_fresh,
                last_error,
            });
        }

        states
    }

    pub async fn list_all_tools(&self) -> Result<Vec<(McpServerId, McpToolSchema)>> {
        let mut all_tools = Vec::new();
        for name in self.configs.keys() {
            let client = self.get_client(name).await?;
            let tools = client.list_tools().await?;
            for tool in tools {
                all_tools.push((McpServerId(name.clone()), tool));
            }
        }
        Ok(all_tools)
    }

    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpCallResult> {
        let client = self.get_client(server_name).await?;
        client.call_tool(tool_name, arguments).await
    }

    pub fn get_cached_tools(&self) -> Vec<(McpServerId, crate::model::McpToolSchema)> {
        let mut tools = Vec::new();
        if let Ok(clients) = self.clients.try_lock() {
            for (name, cell) in clients.iter() {
                if let Some(Ok(client)) = cell.get() {
                    if let Some(cached) = client.get_cached_tools() {
                        for tool in cached {
                            tools.push((McpServerId(name.clone()), tool));
                        }
                    }
                }
            }
        }
        tools
    }

    pub fn get_server_trust_level(&self, name: &str) -> Option<String> {
        self.configs.get(name).and_then(|c| c.trust_level.clone())
    }

    pub fn get_tool_annotations(
        &self,
        server_name: &str,
        tool_name: &str,
    ) -> Option<HashMap<String, String>> {
        self.configs
            .get(server_name)
            .and_then(|c| c.tool_annotations.get(tool_name).cloned())
    }

    pub fn get_cached_tool(
        &self,
        server_name: &str,
        tool_name: &str,
    ) -> Option<crate::model::McpToolSchema> {
        if let Ok(clients) = self.clients.try_lock() {
            if let Some(cell) = clients.get(server_name) {
                if let Some(Ok(client)) = cell.get() {
                    if let Some(cached) = client.get_cached_tools() {
                        return cached.into_iter().find(|t| t.name == tool_name);
                    }
                }
            }
        }
        None
    }

    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.lock().await;
        for (_, cell) in clients.drain() {
            if let Some(Ok(client)) = cell.get() {
                client.shutdown().await;
            }
        }
    }
}
