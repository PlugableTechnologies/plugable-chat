//! MCP server management Tauri commands.
//!
//! Commands for managing MCP (Model Context Protocol) server connections,
//! listing tools, and executing remote tool calls.

use crate::actors::mcp_host_actor::{McpTool, McpToolResult};
use crate::app_state::{ActorHandles, CancellationState, EmbeddingModelState, SettingsState, TurnProgress, TurnTrackerState};
use crate::direct_tool_turn::{DirectToolEnd, DirectToolRequest};
use crate::mcp_direct_tools::DirectTool;
use crate::protocol::{ChatMessage, McpHostMsg};
use crate::settings::{McpServerConfig, SelectedDirectTool};
use tauri::{Emitter, State};
use tokio::sync::oneshot;

/// Sent whenever a server connects or disconnects; the screen then asks for the direct tools again.
pub const MCP_TOOLS_CHANGED_EVENT: &str = "mcp-tools-changed";

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

/// The tools of connected servers that take one text field: the "tool" entries of the model drop-down.
#[tauri::command]
pub async fn list_direct_mcp_tools(handles: State<'_, ActorHandles>) -> Result<Vec<DirectTool>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::ListDirectTools { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "MCP Host actor died".to_string())
}

/// The tool chosen in the drop-down when the app last closed, if one was.
#[tauri::command]
pub async fn get_selected_direct_tool(settings_state: State<'_, SettingsState>) -> Result<Option<SelectedDirectTool>, String> {
    Ok(settings_state.settings.read().await.selected_direct_tool.clone())
}

/// Remember (or, with `None`, forget) the tool chosen in the drop-down.
#[tauri::command]
pub async fn set_selected_direct_tool(
    tool: Option<SelectedDirectTool>,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.selected_direct_tool = tool;
    crate::settings::save_settings(&guard).await
}

/// Send the message straight to an MCP tool and show its text as the assistant reply: no local model is
/// involved, so this works while the model is missing, failed or still downloading. The turn is saved like
/// any other and reported on the same events as a model turn (`chat-token`, `chat-error`, `chat-saved`,
/// `chat-finished`). Choosing the tool and sending is the approval, so nothing is asked first.
#[tauri::command]
pub async fn call_mcp_tool_directly(
    chat_id: Option<String>,
    title: Option<String>,
    message: String,
    history: Vec<ChatMessage>,
    server_id: String,
    tool_name: String,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
    cancellation_state: State<'_, CancellationState>,
    turn_tracker: State<'_, TurnTrackerState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let chat_id = chat_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let chat_id_return = chat_id.clone();
    let title = title.unwrap_or_else(|| message.chars().take(50).collect::<String>());

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let generation_id = {
        let mut gen_id = cancellation_state.current_generation_id.write().await;
        *gen_id = gen_id.wrapping_add(1);
        *cancellation_state.cancel_signal.write().await = Some(cancel_tx);
        *gen_id
    };
    let now_ms = || {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
    };
    *turn_tracker.progress.write().await = TurnProgress {
        active: true,
        chat_id: Some(chat_id.clone()),
        generation_id,
        timestamp_ms: now_ms(),
        ..TurnProgress::default()
    };

    let mcp_host_tx = handles.mcp_host_tx.clone();
    let vector_tx = handles.vector_tx.clone();
    let embedding_model = embedding_state.cpu_model.clone();
    let turn_progress = turn_tracker.progress.clone();
    let request = DirectToolRequest { chat_id, title, message, history, server_id, tool_name };
    tauri::async_runtime::spawn(async move {
        let turn = crate::direct_tool_turn::run_direct_tool_turn(&mcp_host_tx, &vector_tx, &embedding_model, &request, Some(cancel_rx)).await;
        crate::app_log::log("turn", &format!("direct tool call {} ended: {:?}", turn.label, turn.end));

        let (reply, outcome, detail) = match &turn.end {
            DirectToolEnd::Replied(text) => (text.clone(), crate::turn_record::TurnOutcome::Ok, String::new()),
            DirectToolEnd::Failed(why) => (String::new(), crate::turn_record::TurnOutcome::Error, format!("tool_error: {}: {}", turn.label, why)),
            DirectToolEnd::Cancelled => (String::new(), crate::turn_record::TurnOutcome::Cancelled, "cancelled".to_string()),
        };
        match &turn.end {
            DirectToolEnd::Replied(text) => {
                let _ = app_handle.emit("chat-token", text);
            }
            DirectToolEnd::Failed(why) => {
                let payload = crate::turn_failure::TurnFailureKind::ToolError.event_payload(&format!("{}: {}", turn.label, why));
                let _ = app_handle.emit("chat-error", payload);
            }
            DirectToolEnd::Cancelled => {}
        }
        let _ = app_handle.emit("chat-saved", &request.chat_id);
        {
            let mut progress = turn_progress.write().await;
            progress.active = false;
            progress.finished = true;
            progress.had_tool_calls = true;
            progress.assistant_response = reply.clone();
            progress.timestamp_ms = now_ms();
        }
        // Written before the screen is told the turn finished, as for a model turn (see turn_record.rs).
        if crate::turn_record::is_enabled() {
            let is_error = outcome == crate::turn_record::TurnOutcome::Error;
            let trace = crate::turn_record::ToolTraceEntry::new(&request.server_id, &request.tool_name, turn.arguments.clone(), if is_error { &detail } else { &reply }, is_error, turn.duration);
            let exit_code = crate::turn_record::record_turn(crate::turn_record::TurnRecord {
                prompt_index: 0,
                question: Some(request.message.clone()),
                reply,
                outcome,
                detail,
                model: Some(turn.label.clone()),
                chat_id: Some(request.chat_id.clone()),
                duration_ms: turn.duration.as_millis() as u64,
                tool_calls: vec![trace],
            });
            if let Some(code) = exit_code {
                crate::turn_record::exit_app(&app_handle, code);
            }
        }
        let _ = app_handle.emit("chat-finished", ());
    });

    Ok(chat_id_return)
}
