//! MCP server management Tauri commands.
//!
//! Commands for managing MCP (Model Context Protocol) server connections,
//! listing tools, and executing remote tool calls.

use crate::actors::mcp_host_actor::{McpTool, McpToolResult};
use crate::app_state::{ActorHandles, SettingsState};
use crate::protocol::McpHostMsg;
use crate::settings::McpServerConfig;
use tauri::State;
use tokio::sync::oneshot;

/// Result of syncing an MCP server - includes error message if failed
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpSyncResult {
    pub server_id: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Sync all enabled MCP servers
#[tauri::command]
pub async fn sync_mcp_servers(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<McpSyncResult>, String> {
    let settings = settings_state.settings.read().await;
    let configs = settings.get_all_mcp_configs();
    drop(settings);

    // Count enabled and deferred servers for informative logging
    let enabled_count = configs.iter().filter(|c| c.enabled).count();
    let deferred_count = configs
        .iter()
        .filter(|c| c.enabled && c.defer_tools)
        .count();
    let active_count = enabled_count - deferred_count;
    println!(
        "[MCP] Syncing {} servers ({} enabled: {} active, {} deferred)",
        configs.len(),
        enabled_count,
        active_count,
        deferred_count
    );

    if enabled_count > 0 {
        let enabled_names: Vec<String> = configs
            .iter()
            .filter(|c| c.enabled)
            .map(|c| format!("{} ({})", c.name, c.id))
            .collect();
        println!("[MCP] Enabled servers: {}", enabled_names.join(", "));
    }

    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::SyncEnabledServers {
            configs,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let results = rx.await.map_err(|_| "MCP Host actor died".to_string())?;

    // Convert to McpSyncResult with error messages
    let sync_results: Vec<McpSyncResult> = results
        .into_iter()
        .map(|(id, r)| match r {
            Ok(()) => McpSyncResult {
                server_id: id,
                success: true,
                error: None,
            },
            Err(e) => {
                println!("[MCP] Server {} sync failed: {}", id, e);
                McpSyncResult {
                    server_id: id,
                    success: false,
                    error: Some(e),
                }
            }
        })
        .collect();

    Ok(sync_results)
}

/// Connect to a specific MCP server
#[tauri::command]
pub async fn connect_mcp_server(
    server_id: String,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    // Get the server config from settings
    let settings = settings_state.settings.read().await;
    let config = settings
        .mcp_servers
        .iter()
        .find(|s| s.id == server_id)
        .cloned()
        .ok_or_else(|| format!("Server {} not found in settings", server_id))?;
    drop(settings);

    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::ConnectServer {
            config,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

/// Disconnect from a specific MCP server
#[tauri::command]
pub async fn disconnect_mcp_server(
    server_id: String,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::DisconnectServer {
            server_id,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

/// List tools available from a specific MCP server
#[tauri::command]
pub async fn list_mcp_tools(
    server_id: String,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<McpTool>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::ListTools {
            server_id,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

/// Execute a tool on an MCP server
#[tauri::command]
pub async fn execute_mcp_tool(
    server_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    handles: State<'_, ActorHandles>,
) -> Result<McpToolResult, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::ExecuteTool {
            server_id,
            tool_name,
            arguments,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

/// Get connection status of a specific MCP server
#[tauri::command]
pub async fn get_mcp_server_status(
    server_id: String,
    handles: State<'_, ActorHandles>,
) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::GetServerStatus {
            server_id,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    Ok(rx.await.map_err(|_| "MCP Host actor died".to_string())?)
}

/// Get tool descriptions from all connected MCP servers
#[tauri::command]
pub async fn get_all_mcp_tool_descriptions(
    handles: State<'_, ActorHandles>,
) -> Result<Vec<(String, Vec<McpTool>)>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;

    Ok(rx.await.map_err(|_| "MCP Host actor died".to_string())?)
}

/// Test an MCP server config and return its tools without storing the connection
#[tauri::command]
pub async fn test_mcp_server_config(
    config: McpServerConfig,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<McpTool>, String> {
    println!(
        "[MCP] Testing server config: {} ({})",
        config.name, config.id
    );

    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::TestServerConfig {
            config,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}
