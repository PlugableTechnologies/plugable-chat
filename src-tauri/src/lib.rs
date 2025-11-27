pub mod protocol;
pub mod actors;
pub mod settings;

use protocol::{VectorMsg, FoundryMsg, RagMsg, McpHostMsg, ChatMessage, CachedModel, ModelInfo, RagChunk, RagIndexResult, ParsedToolCall, parse_tool_calls, ToolCallsPendingEvent, ToolExecutingEvent, ToolResultEvent, ToolLoopFinishedEvent};
use actors::vector_actor::VectorActor;
use actors::foundry_actor::FoundryActor;
use actors::rag_actor::RagActor;
use actors::mcp_host_actor::{McpHostActor, McpTool, McpToolResult};
use settings::{AppSettings, McpServerConfig};
use tauri::{State, Manager, Emitter};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use fastembed::TextEmbedding;

/// Approval decision for tool calls
#[derive(Debug, Clone)]
pub enum ToolApprovalDecision {
    Approved,
    Rejected,
}

/// Pending tool approval state
type PendingApprovals = Arc<RwLock<HashMap<String, oneshot::Sender<ToolApprovalDecision>>>>;

// State managed by Tauri
struct ActorHandles {
    vector_tx: mpsc::Sender<VectorMsg>,
    foundry_tx: mpsc::Sender<FoundryMsg>,
    rag_tx: mpsc::Sender<RagMsg>,
    mcp_host_tx: mpsc::Sender<McpHostMsg>,
}

// Shared embedding model for RAG operations
struct EmbeddingModelState {
    model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
}

// Shared settings state
struct SettingsState {
    settings: Arc<RwLock<AppSettings>>,
}

// Pending tool approvals state
struct ToolApprovalState {
    pending: PendingApprovals,
}

/// Maximum number of tool call iterations before stopping (safety limit)
const MAX_TOOL_ITERATIONS: usize = 20;

/// Helper to execute a single tool call via McpHostActor
async fn execute_tool_internal(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    call: &ParsedToolCall,
) -> Result<String, String> {
    let (tx, rx) = oneshot::channel();
    mcp_host_tx.send(McpHostMsg::ExecuteTool {
        server_id: call.server.clone(),
        tool_name: call.tool.clone(),
        arguments: call.arguments.clone(),
        respond_to: tx,
    }).await.map_err(|e| format!("Failed to send to MCP Host: {}", e))?;
    
    let result = rx.await.map_err(|_| "MCP Host actor died".to_string())??;
    
    // Convert the result to a string
    let result_text = result.content.iter()
        .filter_map(|c| c.text.as_ref())
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    
    if result.is_error {
        Err(result_text)
    } else {
        Ok(result_text)
    }
}

/// Format a tool result for injection into the chat history
fn format_tool_result_message(call: &ParsedToolCall, result: &str, is_error: bool) -> String {
    format!(
        "<tool_result server=\"{}\" tool=\"{}\"{}>\n{}\n</tool_result>",
        call.server,
        call.tool,
        if is_error { " error=\"true\"" } else { "" },
        result
    )
}

/// Run the agentic loop: call model, detect tool calls, execute, repeat
async fn run_agentic_loop(
    foundry_tx: mpsc::Sender<FoundryMsg>,
    mcp_host_tx: mpsc::Sender<McpHostMsg>,
    vector_tx: mpsc::Sender<VectorMsg>,
    pending_approvals: PendingApprovals,
    app_handle: tauri::AppHandle,
    mut full_history: Vec<ChatMessage>,
    reasoning_effort: String,
    server_configs: Vec<McpServerConfig>,
    chat_id: String,
    title: String,
    original_message: String,
) {
    let mut iteration = 0;
    let mut had_tool_calls = false;
    let mut final_response = String::new();
    
    loop {
        println!("\n[AgenticLoop] Iteration {} starting...", iteration);
        
        // Create channel for this iteration
        let (tx, mut rx) = mpsc::unbounded_channel();
        
        // Send chat request to Foundry
        if let Err(e) = foundry_tx.send(FoundryMsg::Chat {
            history: full_history.clone(),
            reasoning_effort: reasoning_effort.clone(),
            respond_to: tx,
        }).await {
            println!("[AgenticLoop] ERROR: Failed to send to Foundry: {}", e);
            let _ = app_handle.emit("chat-finished", ());
            return;
        }
        
        // Collect response while streaming tokens to frontend
        let mut assistant_response = String::new();
        while let Some(token) = rx.recv().await {
            assistant_response.push_str(&token);
            let _ = app_handle.emit("chat-token", token);
        }
        
        println!("[AgenticLoop] Response complete ({} chars)", assistant_response.len());
        
        // Check for tool calls in the response
        let tool_calls = parse_tool_calls(&assistant_response);
        
        if tool_calls.is_empty() {
            println!("[AgenticLoop] No tool calls detected, loop complete");
            final_response = assistant_response.clone();
            
            // Add final assistant response to history
            full_history.push(ChatMessage {
                role: "assistant".to_string(),
                content: assistant_response,
            });
            break;
        }
        
        if iteration >= MAX_TOOL_ITERATIONS {
            println!("[AgenticLoop] Max iterations ({}) reached, stopping", MAX_TOOL_ITERATIONS);
            final_response = assistant_response.clone();
            
            // Add response with unexecuted tool calls
            full_history.push(ChatMessage {
                role: "assistant".to_string(),
                content: assistant_response,
            });
            break;
        }
        
        had_tool_calls = true;
        println!("[AgenticLoop] Found {} tool call(s)", tool_calls.len());
        
        // Add assistant response (with tool calls) to history
        full_history.push(ChatMessage {
            role: "assistant".to_string(),
            content: assistant_response.clone(),
        });
        
        // Process each tool call
        let mut tool_results: Vec<String> = Vec::new();
        let mut any_executed = false;
        
        for (idx, call) in tool_calls.iter().enumerate() {
            println!("[AgenticLoop] Processing tool call {}/{}: {}::{}", 
                idx + 1, tool_calls.len(), call.server, call.tool);
            
            // Check if this server allows auto-approve
            let auto_approve = server_configs.iter()
                .find(|s| s.id == call.server)
                .map(|s| s.auto_approve_tools)
                .unwrap_or(false);
            
            if !auto_approve {
                println!("[AgenticLoop] Server {} requires manual approval, emitting pending event", call.server);
                
                // Create a unique approval key for this tool call
                let approval_key = format!("{}-{}-{}", chat_id, iteration, idx);
                
                // Emit pending event for manual approval
                let _ = app_handle.emit("tool-calls-pending", ToolCallsPendingEvent {
                    approval_key: approval_key.clone(),
                    calls: vec![call.clone()],
                    iteration,
                });
                
                // Create approval channel and register it
                let (approval_tx, approval_rx) = oneshot::channel();
                {
                    let mut pending = pending_approvals.write().await;
                    pending.insert(approval_key.clone(), approval_tx);
                }
                
                println!("[AgenticLoop] Waiting for approval on key: {}", approval_key);
                
                // Wait for approval (with timeout)
                let approval_result = tokio::time::timeout(
                    std::time::Duration::from_secs(300), // 5 minute timeout
                    approval_rx
                ).await;
                
                // Clean up the pending entry
                {
                    let mut pending = pending_approvals.write().await;
                    pending.remove(&approval_key);
                }
                
                match approval_result {
                    Ok(Ok(ToolApprovalDecision::Approved)) => {
                        println!("[AgenticLoop] Tool call approved by user");
                        // Fall through to execute the tool
                    }
                    Ok(Ok(ToolApprovalDecision::Rejected)) => {
                        println!("[AgenticLoop] Tool call rejected by user");
                        tool_results.push(format_tool_result_message(
                            call,
                            "Tool execution was rejected by the user.",
                            true,
                        ));
                        continue;
                    }
                    Ok(Err(_)) => {
                        println!("[AgenticLoop] Approval channel closed unexpectedly");
                        tool_results.push(format_tool_result_message(
                            call,
                            "Tool approval was cancelled.",
                            true,
                        ));
                        continue;
                    }
                    Err(_) => {
                        println!("[AgenticLoop] Approval timed out after 5 minutes");
                        tool_results.push(format_tool_result_message(
                            call,
                            "Tool approval timed out. Tool call was skipped.",
                            true,
                        ));
                        continue;
                    }
                }
            }
            
            // Emit executing event
            let _ = app_handle.emit("tool-executing", ToolExecutingEvent {
                server: call.server.clone(),
                tool: call.tool.clone(),
                arguments: call.arguments.clone(),
            });
            
            // Execute the tool
            let (result_text, is_error) = match execute_tool_internal(&mcp_host_tx, call).await {
                Ok(result) => {
                    println!("[AgenticLoop] Tool {} succeeded: {} chars", call.tool, result.len());
                    (result, false)
                }
                Err(e) => {
                    println!("[AgenticLoop] Tool {} failed: {}", call.tool, e);
                    (e, true)
                }
            };
            
            // Emit result event
            let _ = app_handle.emit("tool-result", ToolResultEvent {
                server: call.server.clone(),
                tool: call.tool.clone(),
                result: result_text.clone(),
                is_error,
            });
            
            // Format and collect tool result
            tool_results.push(format_tool_result_message(call, &result_text, is_error));
            any_executed = true;
        }
        
        // If no tools were actually executed (all required manual approval), stop the loop
        if !any_executed {
            println!("[AgenticLoop] No tools executed (all require approval), stopping loop");
            break;
        }
        
        // Add all tool results as a single user message
        let combined_results = tool_results.join("\n\n");
        full_history.push(ChatMessage {
            role: "user".to_string(),
            content: combined_results,
        });
        
        iteration += 1;
        println!("[AgenticLoop] Continuing to iteration {}...", iteration);
    }
    
    // Emit loop finished event
    let _ = app_handle.emit("tool-loop-finished", ToolLoopFinishedEvent {
        iterations: iteration,
        had_tool_calls,
    });
    let _ = app_handle.emit("chat-finished", ());
    
    println!("[AgenticLoop] Loop complete after {} iterations, had_tool_calls={}", iteration, had_tool_calls);
    
    // Save the chat
    let messages_json = serde_json::to_string(&full_history).unwrap_or_default();
    let embedding_text = format!("{}\nUser: {}\nAssistant: {}", title, original_message, final_response);
    
    println!("[ChatSave] Requesting embedding...");
    let (emb_tx, emb_rx) = oneshot::channel();
    
    match foundry_tx.send(FoundryMsg::GetEmbedding {
        text: embedding_text.clone(),
        respond_to: emb_tx,
    }).await {
        Ok(_) => {
            println!("[ChatSave] Waiting for embedding response...");
            match emb_rx.await {
                Ok(vector) => {
                    println!("[ChatSave] Got embedding (len={}), sending to VectorActor...", vector.len());
                    match vector_tx.send(VectorMsg::UpsertChat {
                        id: chat_id.clone(),
                        title: title.clone(),
                        content: embedding_text,
                        messages: messages_json,
                        vector: Some(vector),
                        pinned: false,
                    }).await {
                        Ok(_) => {
                            println!("[ChatSave] UpsertChat sent, emitting chat-saved event");
                            let _ = app_handle.emit("chat-saved", chat_id.clone());
                        }
                        Err(e) => println!("[ChatSave] ERROR: Failed to send UpsertChat: {}", e),
                    }
                }
                Err(e) => println!("[ChatSave] ERROR: Failed to receive embedding: {}", e),
            }
        }
        Err(e) => println!("[ChatSave] ERROR: Failed to send GetEmbedding: {}", e),
    }
}

/// Build the full system prompt with MCP tool descriptions
fn build_system_prompt(base_prompt: &str, tool_descriptions: &[(String, Vec<McpTool>)]) -> String {
    let mut prompt = base_prompt.to_string();
    
    // Check if there are any tools to describe
    let has_tools = tool_descriptions.iter().any(|(_, tools)| !tools.is_empty());
    
    if has_tools {
        prompt.push_str("\n\n## IMPORTANT: Tool Calling Instructions\n\n");
        prompt.push_str("You have access to external tools. When the user asks you to do something that requires a tool, you MUST actually call the tool by outputting the exact XML format below. Do NOT just explain how to call it - actually call it!\n\n");
        prompt.push_str("**REQUIRED FORMAT** (you must output this exact structure):\n");
        prompt.push_str("```\n<tool_call>{\"server\": \"SERVER_ID\", \"tool\": \"TOOL_NAME\", \"arguments\": {\"arg1\": \"value1\"}}</tool_call>\n```\n\n");
        prompt.push_str("**RULES:**\n");
        prompt.push_str("1. Output the <tool_call>...</tool_call> tags exactly as shown - the system parses them automatically\n");
        prompt.push_str("2. The JSON inside must be valid with proper quoting\n");
        prompt.push_str("3. Use the exact server ID and tool name from the list below\n");
        prompt.push_str("4. After outputting a tool call, STOP and wait for the result\n\n");
        
        // Collect first available server and tool for example
        let mut example_server = String::new();
        let mut example_tool = String::new();
        for (server_id, tools) in tool_descriptions {
            if !tools.is_empty() {
                example_server = server_id.clone();
                example_tool = tools[0].name.clone();
                break;
            }
        }
        
        if !example_server.is_empty() {
            prompt.push_str("**EXAMPLE** (using your actual available tools):\n");
            prompt.push_str(&format!("```\n<tool_call>{{\"server\": \"{}\", \"tool\": \"{}\", \"arguments\": {{}}}}</tool_call>\n```\n\n", example_server, example_tool));
        }
        
        prompt.push_str("## Available Tools\n\n");
        
        for (server_id, tools) in tool_descriptions {
            if tools.is_empty() {
                continue;
            }
            
            prompt.push_str(&format!("### Server: `{}`\n\n", server_id));
            
            for tool in tools {
                prompt.push_str(&format!("**{}**", tool.name));
                if let Some(desc) = &tool.description {
                    prompt.push_str(&format!(": {}", desc));
                }
                prompt.push('\n');
                
                if let Some(schema) = &tool.input_schema {
                    if let Some(properties) = schema.get("properties") {
                        if let Some(props) = properties.as_object() {
                            prompt.push_str("  Arguments:\n");
                            for (name, prop_schema) in props {
                                let prop_type = prop_schema.get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("any");
                                let prop_desc = prop_schema.get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");
                                let required = schema.get("required")
                                    .and_then(|r| r.as_array())
                                    .map(|arr| arr.iter().any(|v| v.as_str() == Some(name)))
                                    .unwrap_or(false);
                                let req_marker = if required { " [required]" } else { "" };
                                prompt.push_str(&format!("  - `{}` ({}){}: {}\n", name, prop_type, req_marker, prop_desc));
                            }
                        }
                    }
                }
                prompt.push('\n');
            }
        }
        
        prompt.push_str("\nRemember: When asked to use a tool, OUTPUT the <tool_call> tags directly - don't just describe them!\n");
    }
    
    prompt
}

#[tauri::command]
async fn search_history(
    query: String, 
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle
) -> Result<(), String> {
    // Ask Foundry Actor for embedding
    let (emb_tx, emb_rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetEmbedding { 
        text: query, 
        respond_to: emb_tx 
    }).await.map_err(|e| e.to_string())?;

    // Wait for embedding
    let embedding = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // Send to Vector Actor
    let (search_tx, search_rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::SearchHistory {
        query_vector: embedding,
        limit: 10,
        respond_to: search_tx
    }).await.map_err(|e| e.to_string())?;

    let results = search_rx.await.map_err(|_| "Vector actor died")?;
    
    app_handle.emit("sidebar-update", results).map_err(|e| e.to_string())?;
    
    Ok(())
}

#[tauri::command]
async fn get_all_chats(handles: State<'_, ActorHandles>) -> Result<Vec<protocol::ChatSummary>, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::GetAllChats { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn get_models(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn set_model(model: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::SetModel { model_id: model, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn get_cached_models(handles: State<'_, ActorHandles>) -> Result<Vec<CachedModel>, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetCachedModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn get_model_info(handles: State<'_, ActorHandles>) -> Result<Vec<ModelInfo>, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetModelInfo { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn chat(
    chat_id: Option<String>,
    title: Option<String>,
    message: String,
    history: Vec<ChatMessage>,
    reasoning_effort: String,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    approval_state: State<'_, ToolApprovalState>,
    app_handle: tauri::AppHandle
) -> Result<String, String> {
    let chat_id = chat_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let chat_id_return = chat_id.clone();
    let title = title.unwrap_or_else(|| message.chars().take(50).collect::<String>());
    
    // Get system prompt and server configs from settings
    let settings = settings_state.settings.read().await;
    let base_system_prompt = settings.system_prompt.clone();
    let server_configs = settings.mcp_servers.clone();
    drop(settings);
    
    // Get tool descriptions from MCP Host Actor
    let (tools_tx, tools_rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::GetAllToolDescriptions { respond_to: tools_tx })
        .await
        .map_err(|e| e.to_string())?;
    let tool_descriptions = tools_rx.await.map_err(|_| "MCP Host actor died".to_string())?;
    
    // Build the full system prompt with tool descriptions
    let system_prompt = build_system_prompt(&base_system_prompt, &tool_descriptions);
    
    // === LOGGING: System prompt and MCP tool descriptions ===
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║              CHAT CONTEXT - NEW MESSAGE                      ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║ BASE SYSTEM PROMPT ({} chars):                               ", base_system_prompt.len());
    println!("╟──────────────────────────────────────────────────────────────╢");
    for line in base_system_prompt.lines().take(5) {
        println!("║ {}", if line.len() > 60 { &line[..60] } else { line });
    }
    if base_system_prompt.lines().count() > 5 {
        println!("║ ... ({} more lines)", base_system_prompt.lines().count() - 5);
    }
    println!("╟──────────────────────────────────────────────────────────────╢");
    println!("║ MCP TOOL DESCRIPTIONS:                                       ");
    if tool_descriptions.is_empty() {
        println!("║   (no enabled MCP servers with tools)");
    } else {
        for (server_id, tools) in &tool_descriptions {
            println!("║   Server: {} ({} tools)", server_id, tools.len());
            for tool in tools {
                println!("║     - {}: {}", tool.name, tool.description.as_deref().unwrap_or("no description"));
            }
        }
    }
    println!("╟──────────────────────────────────────────────────────────────╢");
    println!("║ FINAL SYSTEM PROMPT LENGTH: {} chars                        ", system_prompt.len());
    println!("║ AUTO-APPROVE SERVERS:                                        ");
    for cfg in &server_configs {
        if cfg.auto_approve_tools {
            println!("║   - {} ({})", cfg.name, cfg.id);
        }
    }
    println!("╚══════════════════════════════════════════════════════════════╝\n");
    
    // Build full history with system prompt at the beginning
    let mut full_history = Vec::new();
    
    // Add system prompt if we have one
    if !system_prompt.is_empty() {
        full_history.push(ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
        });
    }
    
    // Add existing history (skip any existing system messages to avoid duplicates)
    for msg in history.iter() {
        if msg.role != "system" {
            full_history.push(msg.clone());
        }
    }
    
    // Add the new user message
    full_history.push(ChatMessage {
        role: "user".to_string(),
        content: message.clone(),
    });

    // Clone handles for the async task
    let foundry_tx = handles.foundry_tx.clone();
    let mcp_host_tx = handles.mcp_host_tx.clone();
    let vector_tx = handles.vector_tx.clone();
    let pending_approvals = approval_state.pending.clone();
    let chat_id_task = chat_id.clone();
    let title_task = title.clone();
    let message_task = message.clone();

    // Spawn the agentic loop task
    tauri::async_runtime::spawn(async move {
        run_agentic_loop(
            foundry_tx,
            mcp_host_tx,
            vector_tx,
            pending_approvals,
            app_handle,
            full_history,
            reasoning_effort,
            server_configs,
            chat_id_task,
            title_task,
            message_task,
        ).await;
    });

    Ok(chat_id_return)
}

#[tauri::command]
async fn delete_chat(id: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::DeleteChat { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn load_chat(id: String, handles: State<'_, ActorHandles>) -> Result<Option<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::GetChat { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn update_chat(id: String, title: Option<String>, pinned: Option<bool>, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::UpdateChatMetadata { id, title, pinned, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
fn log_to_terminal(message: String) {
    println!("[FRONTEND] {}", message);
}

// ============ RAG Commands ============

#[tauri::command]
async fn select_files() -> Result<Vec<String>, String> {
    // Note: File selection is handled directly by the frontend using the dialog plugin
    // This command is kept for potential future use
    Ok(Vec::new())
}

#[tauri::command]
async fn select_folder() -> Result<Option<String>, String> {
    // Similar to select_files - frontend will use dialog plugin directly
    Ok(None)
}

#[tauri::command]
async fn process_rag_documents(
    paths: Vec<String>,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<RagIndexResult, String> {
    println!("[RAG] Processing {} paths", paths.len());
    
    // Get the embedding model
    let model_guard = embedding_state.model.read().await;
    let embedding_model = model_guard.clone()
        .ok_or_else(|| "Embedding model not initialized".to_string())?;
    drop(model_guard);
    
    let (tx, rx) = oneshot::channel();
    handles.rag_tx.send(RagMsg::ProcessDocuments {
        paths,
        embedding_model,
        respond_to: tx,
    }).await.map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "RAG actor died".to_string())?
}

#[tauri::command]
async fn search_rag_context(
    query: String,
    limit: usize,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<RagChunk>, String> {
    println!("[RAG] Searching for context with query length: {}", query.len());
    
    // First, get embedding for the query
    let (emb_tx, emb_rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetEmbedding {
        text: query,
        respond_to: emb_tx,
    }).await.map_err(|e| e.to_string())?;
    
    let query_vector = emb_rx.await.map_err(|_| "Foundry actor died")?;
    
    // Then search the RAG index
    let (search_tx, search_rx) = oneshot::channel();
    handles.rag_tx.send(RagMsg::SearchDocuments {
        query_vector,
        limit,
        respond_to: search_tx,
    }).await.map_err(|e| e.to_string())?;
    
    search_rx.await.map_err(|_| "RAG actor died".to_string())
}

#[tauri::command]
async fn clear_rag_context(handles: State<'_, ActorHandles>) -> Result<bool, String> {
    println!("[RAG] Clearing context");
    
    let (tx, rx) = oneshot::channel();
    handles.rag_tx.send(RagMsg::ClearContext { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "RAG actor died".to_string())
}

// ============ Settings Commands ============

#[tauri::command]
async fn get_settings(settings_state: State<'_, SettingsState>) -> Result<AppSettings, String> {
    let guard = settings_state.settings.read().await;
    Ok(guard.clone())
}

#[tauri::command]
async fn save_app_settings(
    new_settings: AppSettings,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    // Save to file
    settings::save_settings(&new_settings).await?;
    
    // Update in-memory state
    let mut guard = settings_state.settings.write().await;
    *guard = new_settings;
    
    Ok(())
}

#[tauri::command]
async fn add_mcp_server(
    config: McpServerConfig,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    
    // Check for duplicate ID
    if guard.mcp_servers.iter().any(|s| s.id == config.id) {
        return Err(format!("Server with ID '{}' already exists", config.id));
    }
    
    guard.mcp_servers.push(config);
    settings::save_settings(&guard).await?;
    
    Ok(())
}

#[tauri::command]
async fn update_mcp_server(
    config: McpServerConfig,
    settings_state: State<'_, SettingsState>,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    let configs_for_sync;
    {
        let mut guard = settings_state.settings.write().await;
        
        if let Some(server) = guard.mcp_servers.iter_mut().find(|s| s.id == config.id) {
            *server = config;
            settings::save_settings(&guard).await?;
            configs_for_sync = guard.mcp_servers.clone();
        } else {
            return Err(format!("Server with ID '{}' not found", config.id));
        }
    }
    
    // Sync enabled servers after settings change
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::SyncEnabledServers { 
        configs: configs_for_sync, 
        respond_to: tx 
    }).await.map_err(|e| e.to_string())?;
    
    let results = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
    for (server_id, result) in results {
        match result {
            Ok(()) => println!("[Settings] Server {} sync successful", server_id),
            Err(e) => println!("[Settings] Server {} sync failed: {}", server_id, e),
        }
    }
    
    Ok(())
}

#[tauri::command]
async fn remove_mcp_server(
    server_id: String,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    
    let initial_len = guard.mcp_servers.len();
    guard.mcp_servers.retain(|s| s.id != server_id);
    
    if guard.mcp_servers.len() < initial_len {
        settings::save_settings(&guard).await?;
        Ok(())
    } else {
        Err(format!("Server with ID '{}' not found", server_id))
    }
}

#[tauri::command]
async fn update_system_prompt(
    prompt: String,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.system_prompt = prompt;
    settings::save_settings(&guard).await?;
    Ok(())
}

// ============ MCP Commands ============

#[tauri::command]
async fn sync_mcp_servers(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<(String, bool)>, String> {
    let settings = settings_state.settings.read().await;
    let configs = settings.mcp_servers.clone();
    drop(settings);
    
    println!("[MCP] Syncing {} server configs...", configs.len());
    
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::SyncEnabledServers { 
        configs, 
        respond_to: tx 
    }).await.map_err(|e| e.to_string())?;
    
    let results = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
    
    // Convert to (server_id, success) tuples
    Ok(results.into_iter().map(|(id, r)| (id, r.is_ok())).collect())
}

#[tauri::command]
async fn connect_mcp_server(
    server_id: String,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    // Get the server config from settings
    let settings = settings_state.settings.read().await;
    let config = settings.mcp_servers.iter()
        .find(|s| s.id == server_id)
        .cloned()
        .ok_or_else(|| format!("Server {} not found in settings", server_id))?;
    drop(settings);
    
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::ConnectServer { config, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

#[tauri::command]
async fn disconnect_mcp_server(
    server_id: String,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::DisconnectServer { server_id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

#[tauri::command]
async fn list_mcp_tools(
    server_id: String,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<McpTool>, String> {
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::ListTools { server_id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

#[tauri::command]
async fn execute_mcp_tool(
    server_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    handles: State<'_, ActorHandles>,
) -> Result<McpToolResult, String> {
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::ExecuteTool { 
        server_id, 
        tool_name, 
        arguments, 
        respond_to: tx 
    })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

#[tauri::command]
async fn get_mcp_server_status(
    server_id: String,
    handles: State<'_, ActorHandles>,
) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::GetServerStatus { server_id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    Ok(rx.await.map_err(|_| "MCP Host actor died".to_string())?)
}

#[tauri::command]
async fn get_all_mcp_tool_descriptions(
    handles: State<'_, ActorHandles>,
) -> Result<Vec<(String, Vec<McpTool>)>, String> {
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    Ok(rx.await.map_err(|_| "MCP Host actor died".to_string())?)
}

#[tauri::command]
fn detect_tool_calls(content: String) -> Vec<ParsedToolCall> {
    parse_tool_calls(&content)
}

/// Execute a tool call and return the result
#[tauri::command]
async fn execute_tool_call(
    server_id: String,
    tool_name: String,
    arguments: serde_json::Value,
    handles: State<'_, ActorHandles>,
) -> Result<String, String> {
    println!("[ToolCall] Executing {}::{} with args: {:?}", server_id, tool_name, arguments);
    
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::ExecuteTool { 
        server_id, 
        tool_name: tool_name.clone(),
        arguments, 
        respond_to: tx 
    })
        .await
        .map_err(|e| e.to_string())?;
    
    let result = rx.await.map_err(|_| "MCP Host actor died".to_string())??;
    
    // Convert the result to a string for display
    let result_text = result.content.iter()
        .filter_map(|c| c.text.as_ref())
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    
    if result.is_error {
        Err(format!("Tool error: {}", result_text))
    } else {
        Ok(result_text)
    }
}

/// Approve a pending tool call
#[tauri::command]
async fn approve_tool_call(
    approval_key: String,
    approval_state: State<'_, ToolApprovalState>,
) -> Result<bool, String> {
    println!("[ToolApproval] Approving tool call: {}", approval_key);
    
    let mut pending = approval_state.pending.write().await;
    if let Some(sender) = pending.remove(&approval_key) {
        let _ = sender.send(ToolApprovalDecision::Approved);
        Ok(true)
    } else {
        println!("[ToolApproval] No pending approval found for key: {}", approval_key);
        Err(format!("No pending approval found for key: {}", approval_key))
    }
}

/// Reject a pending tool call
#[tauri::command]
async fn reject_tool_call(
    approval_key: String,
    approval_state: State<'_, ToolApprovalState>,
) -> Result<bool, String> {
    println!("[ToolApproval] Rejecting tool call: {}", approval_key);
    
    let mut pending = approval_state.pending.write().await;
    if let Some(sender) = pending.remove(&approval_key) {
        let _ = sender.send(ToolApprovalDecision::Rejected);
        Ok(true)
    } else {
        println!("[ToolApproval] No pending approval found for key: {}", approval_key);
        Err(format!("No pending approval found for key: {}", approval_key))
    }
}

/// Get list of pending tool approval keys
#[tauri::command]
async fn get_pending_tool_approvals(
    approval_state: State<'_, ToolApprovalState>,
) -> Result<Vec<String>, String> {
    let pending = approval_state.pending.read().await;
    Ok(pending.keys().cloned().collect())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
             // Initialize channels
             let (vector_tx, vector_rx) = mpsc::channel(32);
             let (foundry_tx, foundry_rx) = mpsc::channel(32);
             let (rag_tx, rag_rx) = mpsc::channel(32);
             let (mcp_host_tx, mcp_host_rx) = mpsc::channel(32);
             
             // Store handles in state
             app.manage(ActorHandles { vector_tx, foundry_tx, rag_tx, mcp_host_tx });
             
             // Initialize shared embedding model state
             let embedding_model_state = EmbeddingModelState {
                 model: Arc::new(RwLock::new(None)),
             };
             let embedding_model_arc = embedding_model_state.model.clone();
             app.manage(embedding_model_state);
             
             // Initialize settings state (load from config file)
             let settings = tauri::async_runtime::block_on(async {
                 settings::load_settings().await
             });
             println!("Settings loaded: {} MCP servers configured", settings.mcp_servers.len());
             let settings_state = SettingsState {
                 settings: Arc::new(RwLock::new(settings)),
             };
             app.manage(settings_state);
             
             // Initialize tool approval state
             let approval_state = ToolApprovalState {
                 pending: Arc::new(RwLock::new(HashMap::new())),
             };
             app.manage(approval_state);

             let app_handle = app.handle();
             // Spawn Vector Actor
             tauri::async_runtime::spawn(async move {
                 println!("Starting Vector Actor...");
                 // Ensure data directory exists
                 let _ = tokio::fs::create_dir_all("./data").await;
                 
                 let actor = VectorActor::new(vector_rx, "./data/lancedb").await;
                 println!("Vector Actor initialized.");
                 actor.run().await;
             });

             // Spawn Foundry Actor
            let foundry_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                 println!("Starting Foundry Actor...");
                let actor = FoundryActor::new(foundry_rx, foundry_app_handle);
                 actor.run().await;
             });
             
             // Spawn RAG Actor
             tauri::async_runtime::spawn(async move {
                 println!("Starting RAG Actor...");
                 let actor = RagActor::new(rag_rx);
                 actor.run().await;
             });
             
             // Spawn MCP Host Actor
             tauri::async_runtime::spawn(async move {
                 println!("Starting MCP Host Actor...");
                 let actor = McpHostActor::new(mcp_host_rx);
                 actor.run().await;
             });
             
             // Initialize embedding model in background (shared between FoundryActor and RAG)
             tauri::async_runtime::spawn(async move {
                 println!("Initializing shared embedding model for RAG...");
                 use fastembed::{InitOptions, EmbeddingModel};
                 
                 match tokio::task::spawn_blocking(|| {
                     let mut options = InitOptions::default();
                     options.model_name = EmbeddingModel::AllMiniLML6V2;
                     options.show_download_progress = true;
                     TextEmbedding::try_new(options)
                 }).await {
                     Ok(Ok(model)) => {
                         let mut guard = embedding_model_arc.write().await;
                         *guard = Some(Arc::new(model));
                         println!("Shared embedding model initialized successfully");
                     }
                     Ok(Err(e)) => {
                         println!("ERROR: Failed to initialize embedding model: {}", e);
                     }
                     Err(e) => {
                         println!("ERROR: Embedding model task panicked: {}", e);
                     }
                 }
             });
             
             Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search_history, 
            chat, 
            get_models, 
            get_cached_models,
            get_model_info,
            set_model, 
            get_all_chats, 
            log_to_terminal, 
            delete_chat, 
            load_chat, 
            update_chat,
            // RAG commands
            select_files,
            select_folder,
            process_rag_documents,
            search_rag_context,
            clear_rag_context,
            // Settings commands
            get_settings,
            save_app_settings,
            add_mcp_server,
            update_mcp_server,
            remove_mcp_server,
            update_system_prompt,
            // MCP commands
            sync_mcp_servers,
            connect_mcp_server,
            disconnect_mcp_server,
            list_mcp_tools,
            execute_mcp_tool,
            get_mcp_server_status,
            get_all_mcp_tool_descriptions,
            detect_tool_calls,
            execute_tool_call,
            approve_tool_call,
            reject_tool_call,
            get_pending_tool_approvals
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
