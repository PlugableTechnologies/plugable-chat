//! Tool call detection and approval Tauri commands.
//!
//! Commands for detecting tool calls in model responses, executing tools,
//! and managing the approval workflow for tool execution.

use crate::app_state::{ActorHandles, ToolApprovalDecision, ToolApprovalState};
use crate::protocol::{parse_tool_calls, McpHostMsg, ParsedToolCall};
use tauri::State;
use tokio::sync::oneshot;

/// Detect tool calls in content (for testing/debugging)
#[tauri::command]
pub fn detect_tool_calls(content: String) -> Vec<ParsedToolCall> {
    parse_tool_calls(&content)
}

/// Execute a tool call directly
#[tauri::command]
pub async fn execute_tool_call(
    server_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    handles: State<'_, ActorHandles>,
) -> Result<String, String> {
    if server_id == "builtin" {
        // Handle built-in tools
        match tool_name.as_str() {
            "python_execution" => {
                Err("Direct python_execution not supported via execute_tool_call".to_string())
            }
            "tool_search" => {
                Err("Direct tool_search not supported via execute_tool_call".to_string())
            }
            _ => Err(format!("Unknown built-in tool: {}", tool_name)),
        }
    } else {
        // Execute MCP tool
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

        let result = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
        result.map(|r| {
            r.content
                .into_iter()
                .filter_map(|c| c.text)
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
}

/// Approve a pending tool call
#[tauri::command]
pub async fn approve_tool_call(
    approval_key: String,
    approval_state: State<'_, ToolApprovalState>,
) -> Result<bool, String> {
    let sender = {
        let mut pending = approval_state.pending.write().await;
        pending.remove(&approval_key)
    };

    if let Some(sender) = sender {
        sender
            .send(ToolApprovalDecision::Approved)
            .map_err(|_| "Failed to send approval")?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Reject a pending tool call
#[tauri::command]
pub async fn reject_tool_call(
    approval_key: String,
    approval_state: State<'_, ToolApprovalState>,
) -> Result<bool, String> {
    let sender = {
        let mut pending = approval_state.pending.write().await;
        pending.remove(&approval_key)
    };

    if let Some(sender) = sender {
        sender
            .send(ToolApprovalDecision::Rejected)
            .map_err(|_| "Failed to send rejection")?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Get list of pending tool approval keys
#[tauri::command]
pub async fn get_pending_tool_approvals(
    approval_state: State<'_, ToolApprovalState>,
) -> Result<Vec<String>, String> {
    let pending = approval_state.pending.read().await;
    Ok(pending.keys().cloned().collect())
}
