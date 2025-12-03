pub mod protocol;
pub mod actors;
pub mod settings;
pub mod tool_adapters;
pub mod model_profiles;
pub mod tool_registry;
pub mod tools;

use protocol::{VectorMsg, FoundryMsg, RagMsg, McpHostMsg, ChatMessage, CachedModel, ModelInfo, RagChunk, RagIndexResult, ParsedToolCall, parse_tool_calls, ToolCallsPendingEvent, ToolExecutingEvent, ToolResultEvent, ToolLoopFinishedEvent, OpenAITool};
use tool_adapters::{parse_tool_calls_for_model, format_tool_result};
use model_profiles::resolve_profile;
use tool_registry::{SharedToolRegistry, ToolSearchResult, create_shared_registry};
use tools::tool_search::{ToolSearchExecutor, ToolSearchInput, precompute_tool_embeddings};
use tools::code_execution::{CodeExecutionInput, CodeExecutionOutput, CodeExecutionExecutor};
use actors::vector_actor::VectorActor;
use actors::foundry_actor::FoundryActor;
use actors::rag_actor::RagActor;
use actors::mcp_host_actor::{McpHostActor, McpTool, McpToolResult};
use actors::python_actor::{PythonActor, PythonMsg};
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
    python_tx: mpsc::Sender<PythonMsg>,
}

// Shared tool registry state
struct ToolRegistryState {
    registry: SharedToolRegistry,
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

/// Check if a tool call is for a built-in tool (code_execution or tool_search)
fn is_builtin_tool(tool_name: &str) -> bool {
    tool_name == "code_execution" || tool_name == "tool_search"
}

/// Execute the tool_search built-in tool
async fn execute_tool_search(
    input: ToolSearchInput,
    tool_registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
) -> Result<(String, Vec<ToolSearchResult>), String> {
    let executor = ToolSearchExecutor::new(tool_registry.clone(), embedding_model);
    let output = executor.execute(input).await?;
    
    // Materialize discovered tools
    executor.materialize_results(&output.tools).await;
    
    // Format result as JSON for the model
    let result_json = serde_json::to_string_pretty(&output.tools)
        .unwrap_or_else(|_| "[]".to_string());
    
    Ok((result_json, output.tools))
}

/// Execute the code_execution built-in tool
async fn execute_code_execution(
    input: CodeExecutionInput,
    exec_id: String,
    tool_registry: SharedToolRegistry,
    python_tx: &mpsc::Sender<PythonMsg>,
) -> Result<CodeExecutionOutput, String> {
    // Log the code about to be executed
    println!("[code_execution] exec_id={}", exec_id);
    println!("[code_execution] Code to execute ({} lines):", input.code.len());
    for (i, line) in input.code.iter().enumerate() {
        println!("[code_execution]   {}: {}", i + 1, line);
    }
    // Flush stdout to ensure logs appear immediately
    use std::io::Write;
    let _ = std::io::stdout().flush();
    
    // Get available tools for the execution context
    let available_tools = {
        let registry = tool_registry.read().await;
        registry.get_visible_tools()
    };
    
    println!("[code_execution] Available tools: {}", available_tools.len());
    let _ = std::io::stdout().flush();
    
    // Create execution context
    let context = CodeExecutionExecutor::create_context(
        exec_id.clone(),
        available_tools,
        input.context.clone(),
    );
    
    println!("[code_execution] Sending to Python actor...");
    let _ = std::io::stdout().flush();
    
    // Send to Python actor for execution
    let (respond_to, rx) = oneshot::channel();
    python_tx.send(PythonMsg::Execute {
        input,
        context,
        respond_to,
    }).await.map_err(|e| format!("Failed to send to Python actor: {}", e))?;
    
    println!("[code_execution] Waiting for Python actor response...");
    let _ = std::io::stdout().flush();
    
    let result = rx.await.map_err(|_| "Python actor died".to_string())?;
    
    println!("[code_execution] Python execution complete: success={}", result.as_ref().map(|r| r.success).unwrap_or(false));
    let _ = std::io::stdout().flush();
    
    result
}

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

/// Try to resolve an unknown server ID by finding which server has the given tool
async fn resolve_server_for_tool(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    tool_name: &str,
) -> Option<String> {
    println!("[resolve_server_for_tool] Searching for tool '{}' across servers...", tool_name);
    
    // Get all tool descriptions from connected servers
    let (tx, rx) = oneshot::channel();
    if mcp_host_tx.send(McpHostMsg::GetAllToolDescriptions { respond_to: tx }).await.is_err() {
        return None;
    }
    
    let tool_descriptions = match rx.await {
        Ok(descriptions) => descriptions,
        Err(_) => return None,
    };
    
    // Search for the tool in each server
    for (server_id, tools) in tool_descriptions {
        for tool in tools {
            if tool.name == tool_name {
                println!("[resolve_server_for_tool] Found tool '{}' on server '{}'", tool_name, server_id);
                return Some(server_id);
            }
        }
    }
    
    println!("[resolve_server_for_tool] Tool '{}' not found on any connected server", tool_name);
    None
}

/// Run the agentic loop: call model, detect tool calls, execute, repeat
async fn run_agentic_loop(
    foundry_tx: mpsc::Sender<FoundryMsg>,
    mcp_host_tx: mpsc::Sender<McpHostMsg>,
    vector_tx: mpsc::Sender<VectorMsg>,
    python_tx: mpsc::Sender<PythonMsg>,
    tool_registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    pending_approvals: PendingApprovals,
    app_handle: tauri::AppHandle,
    mut full_history: Vec<ChatMessage>,
    reasoning_effort: String,
    server_configs: Vec<McpServerConfig>,
    chat_id: String,
    title: String,
    original_message: String,
    openai_tools: Option<Vec<OpenAITool>>,
    model_name: String,
) {
    // Resolve model profile from model name
    let profile = resolve_profile(&model_name);
    let model_family = profile.family;
    let tool_format = profile.tool_format;
    let mut iteration = 0;
    let mut had_tool_calls = false;
    let mut final_response = String::new();
    
    println!("[AgenticLoop] Starting with model_family={:?}, tool_format={:?}", model_family, tool_format);
    
    loop {
        println!("\n[AgenticLoop] Iteration {} starting...", iteration);
        
        // Create channel for this iteration
        let (tx, mut rx) = mpsc::unbounded_channel();
        
        // Send chat request to Foundry
        if let Err(e) = foundry_tx.send(FoundryMsg::Chat {
            history: full_history.clone(),
            reasoning_effort: reasoning_effort.clone(),
            tools: openai_tools.clone(),
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
        
        // Check for tool calls in the response using model-appropriate parser
        let tool_calls = parse_tool_calls_for_model(&assistant_response, model_family, tool_format);
        
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
            // Resolve server ID if unknown
            // Built-in tools (code_execution, tool_search) use "builtin" as their server
            let resolved_server = if is_builtin_tool(&call.tool) {
                println!("[AgenticLoop] Built-in tool '{}' detected, using 'builtin' server", call.tool);
                "builtin".to_string()
            } else if call.server == "unknown" {
                match resolve_server_for_tool(&mcp_host_tx, &call.tool).await {
                    Some(server_id) => {
                        println!("[AgenticLoop] Resolved unknown server to '{}' for tool '{}'", server_id, call.tool);
                        server_id
                    }
                    None => {
                        println!("[AgenticLoop] ERROR: Could not resolve server for tool '{}', skipping", call.tool);
                        tool_results.push(format_tool_result(
                            call,
                            &format!("Could not find server for tool '{}'. Make sure an MCP server with this tool is connected.", call.tool),
                            true,
                            tool_format,
                        ));
                        continue;
                    }
                }
            } else {
                call.server.clone()
            };
            
            // Create a modified call with the resolved server
            let resolved_call = ParsedToolCall {
                server: resolved_server.clone(),
                tool: call.tool.clone(),
                arguments: call.arguments.clone(),
                raw: call.raw.clone(),
            };
            
            println!("[AgenticLoop] Processing tool call {}/{}: {}::{}", 
                idx + 1, tool_calls.len(), resolved_call.server, resolved_call.tool);
            
            // Check if this server allows auto-approve
            // Built-in tools are always auto-approved
            let auto_approve = if is_builtin_tool(&resolved_call.tool) {
                true
            } else {
                server_configs.iter()
                    .find(|s| s.id == resolved_call.server)
                    .map(|s| s.auto_approve_tools)
                    .unwrap_or(false)
            };
            
            if !auto_approve {
                println!("[AgenticLoop] Server {} requires manual approval, emitting pending event", resolved_call.server);
                
                // Create a unique approval key for this tool call
                let approval_key = format!("{}-{}-{}", chat_id, iteration, idx);
                
                // Emit pending event for manual approval
                let _ = app_handle.emit("tool-calls-pending", ToolCallsPendingEvent {
                    approval_key: approval_key.clone(),
                    calls: vec![resolved_call.clone()],
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
                        tool_results.push(format_tool_result(
                            &resolved_call,
                            "Tool execution was rejected by the user.",
                            true,
                            tool_format,
                        ));
                        continue;
                    }
                    Ok(Err(_)) => {
                        println!("[AgenticLoop] Approval channel closed unexpectedly");
                        tool_results.push(format_tool_result(
                            &resolved_call,
                            "Tool approval was cancelled.",
                            true,
                            tool_format,
                        ));
                        continue;
                    }
                    Err(_) => {
                        println!("[AgenticLoop] Approval timed out after 5 minutes");
                        tool_results.push(format_tool_result(
                            &resolved_call,
                            "Tool approval timed out. Tool call was skipped.",
                            true,
                            tool_format,
                        ));
                        continue;
                    }
                }
            }
            
            // Emit executing event
            let _ = app_handle.emit("tool-executing", ToolExecutingEvent {
                server: resolved_call.server.clone(),
                tool: resolved_call.tool.clone(),
                arguments: resolved_call.arguments.clone(),
            });
            
            // Execute the tool - check for built-in tools first
            let (result_text, is_error) = if is_builtin_tool(&resolved_call.tool) {
                match resolved_call.tool.as_str() {
                    "tool_search" => {
                        // Parse tool_search input
                        let input: ToolSearchInput = serde_json::from_value(resolved_call.arguments.clone())
                            .map_err(|e| format!("Invalid tool_search arguments: {}", e))
                            .unwrap_or(ToolSearchInput { queries: vec![], top_k: 3 });
                        
                        match execute_tool_search(input, tool_registry.clone(), embedding_model.clone()).await {
                            Ok((result, discovered_tools)) => {
                                println!("[AgenticLoop] tool_search found {} tools", discovered_tools.len());
                                (result, false)
                            }
                            Err(e) => {
                                println!("[AgenticLoop] tool_search failed: {}", e);
                                (e, true)
                            }
                        }
                    }
                    "code_execution" => {
                        // Parse code_execution input
                        let input: CodeExecutionInput = serde_json::from_value(resolved_call.arguments.clone())
                            .map_err(|e| format!("Invalid code_execution arguments: {}", e))
                            .unwrap_or(CodeExecutionInput { code: vec![], context: None });
                        
                        let exec_id = format!("{}-{}-{}", chat_id, iteration, idx);
                        match execute_code_execution(input, exec_id, tool_registry.clone(), &python_tx).await {
                            Ok(output) => {
                                println!("[AgenticLoop] code_execution completed: {} chars stdout", output.stdout.len());
                                let result = if output.success {
                                    output.stdout
                                } else {
                                    format!("Error: {}", output.stderr)
                                };
                                (result, !output.success)
                            }
                            Err(e) => {
                                println!("[AgenticLoop] code_execution failed: {}", e);
                                (e, true)
                            }
                        }
                    }
                    _ => {
                        // Unknown built-in tool
                        (format!("Unknown built-in tool: {}", resolved_call.tool), true)
                    }
                }
            } else {
                // Execute MCP tool
                match execute_tool_internal(&mcp_host_tx, &resolved_call).await {
                    Ok(result) => {
                        println!("[AgenticLoop] Tool {} succeeded: {} chars", resolved_call.tool, result.len());
                        (result, false)
                    }
                    Err(e) => {
                        println!("[AgenticLoop] Tool {} failed: {}", resolved_call.tool, e);
                        (e, true)
                    }
                }
            };
            
            // Emit result event
            let _ = app_handle.emit("tool-result", ToolResultEvent {
                server: resolved_call.server.clone(),
                tool: resolved_call.tool.clone(),
                result: result_text.clone(),
                is_error,
            });
            
            // Format and collect tool result using model-appropriate format
            tool_results.push(format_tool_result(&resolved_call, &result_text, is_error, tool_format));
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

/// Build the full system prompt with tool capabilities
fn build_system_prompt(
    base_prompt: &str, 
    tool_descriptions: &[(String, Vec<McpTool>)],
    server_configs: &[McpServerConfig],
) -> String {
    let mut prompt = base_prompt.to_string();
    
    // Categorize servers by defer status
    let mut active_servers: Vec<(&String, &Vec<McpTool>)> = Vec::new();
    let mut deferred_servers: Vec<(&String, &Vec<McpTool>)> = Vec::new();
    
    for (server_id, tools) in tool_descriptions {
        if tools.is_empty() {
            continue;
        }
        let is_deferred = server_configs.iter()
            .find(|c| c.id == *server_id)
            .map(|c| c.defer_tools)
            .unwrap_or(true); // Default to deferred if config not found
        
        if is_deferred {
            deferred_servers.push((server_id, tools));
        } else {
            active_servers.push((server_id, tools));
        }
    }
    
    let has_active_tools = !active_servers.is_empty();
    let has_deferred_tools = !deferred_servers.is_empty();
    let has_mcp_tools = has_active_tools || has_deferred_tools;
    
    let _active_tool_count: usize = active_servers.iter().map(|(_, t)| t.len()).sum();
    let _deferred_tool_count: usize = deferred_servers.iter().map(|(_, t)| t.len()).sum();
    
    // ===== CRITICAL: Tool Selection Decision Tree =====
    prompt.push_str("\n\n## CRITICAL: Tool Selection Guide\n\n");
    prompt.push_str("You have TWO distinct capabilities. Choose the right one:\n\n");
    
    prompt.push_str("### 1. `code_execution` (Built-in Python Sandbox)\n");
    prompt.push_str("**USE FOR:** Pure calculations, math, string manipulation, data transformations, logic\n");
    prompt.push_str("**LIMITATIONS:** \n");
    prompt.push_str("- ❌ CANNOT access internet, databases, files, APIs, or any external systems\n");
    prompt.push_str("- ❌ CANNOT call MCP tools UNLESS you first discover them via `tool_search`\n");
    prompt.push_str("- ✅ CAN use: math, json, random, re, datetime, collections, itertools, functools, statistics, decimal, fractions, hashlib, base64\n\n");
    
    if has_mcp_tools {
        prompt.push_str("### 2. MCP Tools (External Capabilities)\n");
        prompt.push_str("**USE FOR:** Anything requiring external access - databases, APIs, files, web, etc.\n");
        prompt.push_str("**HOW TO USE:**\n");
        if has_deferred_tools {
            prompt.push_str("1. First call `tool_search` to discover available tools for your task\n");
            prompt.push_str("2. Then call the discovered tools directly OR via `code_execution` for complex workflows\n\n");
        } else if has_active_tools {
            prompt.push_str("- Call active MCP tools directly (listed below)\n\n");
        }
        
        prompt.push_str("### Decision Tree:\n");
        prompt.push_str("```\n");
        prompt.push_str("Need external data/access? (database, API, files, web)\n");
        prompt.push_str("├── YES → Use tool_search first, then call discovered MCP tools\n");
        prompt.push_str("└── NO → Pure calculation/logic?\n");
        prompt.push_str("    ├── YES → Use code_execution\n");
        prompt.push_str("    └── NO → Just answer from knowledge\n");
        prompt.push_str("```\n\n");
        
        prompt.push_str("### COMMON MISTAKES TO AVOID:\n");
        prompt.push_str("- ❌ Using `code_execution` alone for tasks needing external data (it will fail)\n");
        prompt.push_str("- ❌ Calling MCP tools without discovering them via `tool_search` first\n");
        prompt.push_str("- ❌ Thinking `code_execution` can access databases/APIs (it cannot by itself)\n");
        prompt.push_str("- ✅ For external access: `tool_search` → discover tools → call them (directly or via code_execution)\n\n");
    }
    
    // Tool calling format instructions
    prompt.push_str("## Tool Calling Format\n\n");
    prompt.push_str("To call a tool, use this EXACT format:\n");
    prompt.push_str("<tool_call>{\"name\": \"TOOL_NAME\", \"arguments\": {\"arg1\": \"value1\"}}</tool_call>\n\n");
    
    prompt.push_str("RULES:\n");
    prompt.push_str("- Call tools immediately when they can help - don't just describe what you would do\n");
    prompt.push_str("- Each argument value must be a SIMPLE value (string, number, boolean), never nested objects\n\n");
    
    // Code execution details
    prompt.push_str("## code_execution Tool\n\n");
    prompt.push_str("Sandboxed Python execution for calculations and data processing.\n");
    prompt.push_str("**You must `import` modules before using them.**\n\n");
    prompt.push_str("Example (pure calculation):\n");
    prompt.push_str("<tool_call>{\"name\": \"code_execution\", \"arguments\": {\"code\": [\"import math\", \"result = math.sqrt(17 * 23 + 456)\", \"print(f'Answer: {result:.2f}')\"]}}");
    prompt.push_str("</tool_call>\n\n");
    
    // Tool discovery section - critical when there are deferred tools
    if has_deferred_tools {
        prompt.push_str("## Tool Discovery (REQUIRED for External Access)\n\n");
        prompt.push_str("**Before using any MCP tool, you MUST discover it first:**\n\n");
        prompt.push_str("<tool_call>{\"name\": \"tool_search\", \"arguments\": {\"queries\": [\"describe what you need\"]}}");
        prompt.push_str("</tool_call>\n\n");
        
        // List what latent capabilities are available (high-level summary)
        prompt.push_str("**Available Tool Servers:**\n");
        for (server_id, tools) in &deferred_servers {
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            prompt.push_str(&format!("- `{}`: {} tools ({}) - use `tool_search` to discover\n", 
                server_id, tools.len(), tool_names.join(", ")));
        }
        prompt.push_str("\n");
        
        prompt.push_str("**Example queries for tool_search:**\n");
        prompt.push_str("- `{\"queries\": [\"execute SQL\", \"query database\"]}` - find data query tools\n");
        prompt.push_str("- `{\"queries\": [\"weather\", \"forecast\"]}` - find weather-related tools\n");
        prompt.push_str("- `{\"queries\": [\"list tables\", \"get schema\"]}` - find data exploration tools\n\n");
    }
    
    // Tool orchestration with code_execution (always show if there are MCP tools)
    if has_mcp_tools {
        prompt.push_str("## Calling Discovered Tools from Code (Advanced)\n\n");
        prompt.push_str("After discovering tools via `tool_search`, you can orchestrate them in `code_execution`:\n");
        prompt.push_str("```python\n");
        prompt.push_str("# Discovered MCP tools are available as async functions:\n");
        prompt.push_str("result = await sql_query(query=\"SELECT * FROM users LIMIT 5\")\n");
        prompt.push_str("# Then process with Python:\n");
        prompt.push_str("for row in result[\"rows\"]:\n");
        prompt.push_str("    print(row)\n");
        prompt.push_str("```\n\n");
        prompt.push_str("**This is the ONLY way to access external data from code_execution** - by calling discovered MCP tools.\n\n");
    }
    
    // List ACTIVE MCP tools in full detail (these can be called immediately)
    if has_active_tools {
        prompt.push_str("## Active MCP Tools (Ready to Use)\n\n");
        prompt.push_str("These tools can be called immediately without `tool_search`:\n\n");
        
        for (server_id, tools) in &active_servers {
            prompt.push_str(&format!("### Server: `{}`\n\n", server_id));
            
            // Include server environment variables as context for the model
            if let Some(config) = server_configs.iter().find(|c| c.id == **server_id) {
                if !config.env.is_empty() {
                    prompt.push_str("**Server Configuration** (use these values for this server's tools):\n");
                    for (key, value) in &config.env {
                        // Skip sensitive keys
                        let key_lower = key.to_lowercase();
                        if key_lower.contains("secret") || key_lower.contains("password") || key_lower.contains("token") || key_lower.contains("key") {
                            continue;
                        }
                        prompt.push_str(&format!("- `{}`: `{}`\n", key, value));
                    }
                    prompt.push_str("\n");
                }
            }
            
            for tool in *tools {
                prompt.push_str(&format!("**{}**", tool.name));
                if let Some(desc) = &tool.description {
                    prompt.push_str(&format!(": {}", desc));
                }
                prompt.push('\n');
                
                if let Some(schema) = &tool.input_schema {
                    if let Some(properties) = schema.get("properties") {
                        if let Some(props) = properties.as_object() {
                            let required_fields: Vec<&str> = schema.get("required")
                                .and_then(|r| r.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();
                            
                            prompt.push_str("  Arguments:\n");
                            for (name, prop_schema) in props {
                                let prop_type = prop_schema.get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("string");
                                let prop_desc = prop_schema.get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");
                                let is_required = required_fields.contains(&name.as_str());
                                let req_marker = if is_required { " [REQUIRED]" } else { "" };
                                
                                prompt.push_str(&format!("  - `{}` ({}){}: {}\n", 
                                    name, prop_type, req_marker, prop_desc));
                            }
                        }
                    }
                }
                prompt.push('\n');
            }
        }
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
    tool_registry_state: State<'_, ToolRegistryState>,
    embedding_state: State<'_, EmbeddingModelState>,
    app_handle: tauri::AppHandle
) -> Result<String, String> {
    let chat_id = chat_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let chat_id_return = chat_id.clone();
    let title = title.unwrap_or_else(|| message.chars().take(50).collect::<String>());
    
    // Get server configs from settings
    let settings = settings_state.settings.read().await;
    let configured_system_prompt = settings.system_prompt.clone();
    let server_configs = settings.mcp_servers.clone();
    drop(settings);
    
    // Get tool descriptions from MCP Host Actor
    let (tools_tx, tools_rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::GetAllToolDescriptions { respond_to: tools_tx })
        .await
        .map_err(|e| e.to_string())?;
    let tool_descriptions = tools_rx.await.map_err(|_| "MCP Host actor died".to_string())?;
    
    // Check if there are any MCP tools available
    let has_mcp_tools = tool_descriptions.iter().any(|(_, tools)| !tools.is_empty());
    
    // Always use the configured system prompt (which should explain tool capabilities)
    let base_system_prompt = configured_system_prompt;
    
    // Build the tools list:
    // 1. Always include code_execution (for deterministic operations)
    // 2. Include tool_search when MCP servers with tools are available
    // 3. Include all MCP tools
    let mut openai_tools: Vec<OpenAITool> = Vec::new();
    
    // Always add code_execution built-in tool
    let code_exec_tool = tool_registry::code_execution_tool();
    openai_tools.push(OpenAITool::from_tool_schema(&code_exec_tool));
    println!("[Chat] Added code_execution built-in tool");
    
    // Add tool_search when MCP tools are available (for discovery)
    if has_mcp_tools {
        let tool_search_tool = tool_registry::tool_search_tool();
        openai_tools.push(OpenAITool::from_tool_schema(&tool_search_tool));
        println!("[Chat] Added tool_search built-in tool (MCP tools available)");
    }
    
    // Add MCP tools to the OpenAI tools list and register them in the tool registry
    // so they're available for code_execution and tool_search
    {
        let mut registry = tool_registry_state.registry.write().await;
        
        // Clear any previously materialized tools (fresh start for this chat)
        registry.clear_materialized();
        
        for (server_id, tools) in &tool_descriptions {
            // Get the defer_tools setting from server config (default to false if not found)
            let defer = server_configs.iter()
                .find(|c| c.id == *server_id)
                .map(|c| c.defer_tools)
                .unwrap_or(false);
            
            let mode = if defer { "DEFERRED" } else { "ACTIVE" };
            println!("[Chat] Registering {} tools from {} [{}]", tools.len(), server_id, mode);
            
            // Register MCP tools in the registry
            registry.register_mcp_tools(server_id, tools, defer);
            
            // Only add to OpenAI tools list for direct model access if NOT deferred
            // Deferred tools are discovered via tool_search
            if !defer {
                for tool in tools {
                    openai_tools.push(OpenAITool::from_mcp(server_id, tool));
                }
            }
        }
        
        let stats = registry.stats();
        println!("[Chat] Tool registry: {} internal, {} domain, {} deferred, {} materialized",
            stats.internal_tools, stats.domain_tools, stats.deferred_tools, stats.materialized_tools);
    }
    
    // Pre-compute embeddings for all domain tools so tool_search can find them
    if !tool_descriptions.is_empty() {
        match precompute_tool_embeddings(
            tool_registry_state.registry.clone(),
            embedding_state.model.clone(),
        ).await {
            Ok(count) => println!("[Chat] Pre-computed embeddings for {} tools", count),
            Err(e) => println!("[Chat] Warning: Failed to pre-compute tool embeddings: {}", e),
        }
    }
    
    let has_tools = !openai_tools.is_empty();
    println!("[Chat] Total tools available: {} (built-in + {} MCP tools)", 
        openai_tools.len(), 
        tool_descriptions.iter().map(|(_, t)| t.len()).sum::<usize>());
    
    // Build the full system prompt with tool descriptions
    // Note: We still include text-based tool instructions as a fallback for models
    // that don't support native tool calling
    let system_prompt = build_system_prompt(&base_system_prompt, &tool_descriptions, &server_configs);
    
    // === LOGGING: System prompt construction ===
    let auto_approve_servers: Vec<&str> = server_configs.iter()
        .filter(|c| c.auto_approve_tools)
        .map(|c| c.id.as_str())
        .collect();
    let tool_count: usize = tool_descriptions.iter().map(|(_, tools)| tools.len()).sum();
    let server_count = tool_descriptions.len();
    
    println!("\n[Chat] System prompt: base={}chars, servers={}, tools={}, auto_approve={:?}",
        base_system_prompt.len(), server_count, tool_count, auto_approve_servers);
    println!("[Chat] Final system prompt ({} chars):\n{}", system_prompt.len(), system_prompt);
    
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

    // Get current model info for model-specific handling
    let (model_info_tx, model_info_rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetModelInfo { respond_to: model_info_tx })
        .await
        .map_err(|e| e.to_string())?;
    let model_info_list = model_info_rx.await.map_err(|_| "Foundry actor died".to_string())?;
    
    // Get the current model name for profile resolution
    let model_name = model_info_list.first()
        .map(|m| m.id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    
    println!("[Chat] Using model: {}", model_name);

    // Clone handles for the async task
    let foundry_tx = handles.foundry_tx.clone();
    let mcp_host_tx = handles.mcp_host_tx.clone();
    let vector_tx = handles.vector_tx.clone();
    let python_tx = handles.python_tx.clone();
    let pending_approvals = approval_state.pending.clone();
    let tool_registry = tool_registry_state.registry.clone();
    let embedding_model = embedding_state.model.clone();
    let chat_id_task = chat_id.clone();
    let title_task = title.clone();
    let message_task = message.clone();
    let openai_tools_task = if has_tools { Some(openai_tools) } else { None };

    // Spawn the agentic loop task
    tauri::async_runtime::spawn(async move {
        run_agentic_loop(
            foundry_tx,
            mcp_host_tx,
            vector_tx,
            python_tx,
            tool_registry,
            embedding_model,
            pending_approvals,
            app_handle,
            full_history,
            reasoning_effort,
            server_configs,
            chat_id_task,
            title_task,
            message_task,
            openai_tools_task,
            model_name,
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

/// Result of syncing an MCP server - includes error message if failed
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpSyncResult {
    pub server_id: String,
    pub success: bool,
    pub error: Option<String>,
}

#[tauri::command]
async fn sync_mcp_servers(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<McpSyncResult>, String> {
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
    
    // Convert to McpSyncResult with error messages
    let sync_results: Vec<McpSyncResult> = results.into_iter().map(|(id, r)| {
        match r {
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
        }
    }).collect();
    
    Ok(sync_results)
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

/// Test an MCP server config and return its tools without storing the connection
#[tauri::command]
async fn test_mcp_server_config(
    config: McpServerConfig,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<McpTool>, String> {
    println!("[MCP] Testing server config: {} ({})", config.name, config.id);
    
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::TestServerConfig { config, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "MCP Host actor died".to_string())?
}

/// Get a preview of the final system prompt with MCP tool descriptions
#[tauri::command]
async fn get_system_prompt_preview(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<String, String> {
    // Get current settings
    let settings = settings_state.settings.read().await;
    let base_prompt = settings.system_prompt.clone();
    let server_configs = settings.mcp_servers.clone();
    drop(settings);
    
    // Get current tool descriptions from connected servers
    let (tx, rx) = oneshot::channel();
    handles.mcp_host_tx.send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    let tool_descriptions = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
    
    // Build the full system prompt
    let preview = build_system_prompt(&base_prompt, &tool_descriptions, &server_configs);
    
    Ok(preview)
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
             let (python_tx, python_rx) = mpsc::channel(32);
             
             // Store handles in state
             app.manage(ActorHandles { vector_tx, foundry_tx, rag_tx, mcp_host_tx, python_tx });
             
             // Initialize shared embedding model state
             let embedding_model_state = EmbeddingModelState {
                 model: Arc::new(RwLock::new(None)),
             };
             let embedding_model_arc = embedding_model_state.model.clone();
             app.manage(embedding_model_state);
             
             // Initialize shared tool registry
             let tool_registry = create_shared_registry();
             let tool_registry_state = ToolRegistryState {
                 registry: tool_registry.clone(),
             };
             app.manage(tool_registry_state);
             
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
             
             // Spawn Python Actor for code execution
             let python_tool_registry = tool_registry.clone();
             tauri::async_runtime::spawn(async move {
                 println!("Starting Python Actor...");
                 let actor = PythonActor::new(python_rx, python_tool_registry);
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
            test_mcp_server_config,
            get_system_prompt_preview,
            detect_tool_calls,
            execute_tool_call,
            approve_tool_call,
            reject_tool_call,
            get_pending_tool_approvals
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
