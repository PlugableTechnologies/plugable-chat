//! Agentic loop execution for multi-turn tool calling.
//!
//! The agentic loop repeatedly calls the model, detects tool calls,
//! executes them, and continues until a final response is produced.
//!
//! ## Key Types
//! - `AgenticLoopConfig` - Configuration for the agentic loop
//! - `AgenticLoopHandles` - Actor channels and shared state
//! - `AgenticLoopAction` - Result of action detection (tool calls vs final response)
//!
//! ## Key Functions
//! - `run_agentic_loop()` - Main loop execution
//! - `detect_agentic_loop_action()` - Determine if response contains tool calls

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fastembed::TextEmbedding;
use serde_json::{json, Value};
use tauri::Emitter;
use tokio::sync::{mpsc, RwLock};

use crate::actors::database_toolbox_actor::DatabaseToolboxMsg;
use crate::actors::python_actor::PythonMsg;
use crate::actors::schema_vector_actor::SchemaVectorMsg;
use crate::app_state::{PendingApprovals, ToolApprovalDecision, TurnProgress};
use crate::cli::is_builtin_tool;
use crate::message_builders::{
    create_assistant_message_with_tool_calls, create_native_tool_result_message,
    should_use_native_tool_results,
};
use crate::model_profiles::resolve_profile;
use crate::protocol::{
    ChatMessage, FoundryMsg, McpHostMsg, ModelFamily, OpenAITool, ParsedToolCall,
    ToolCallsPendingEvent, ToolExecutingEvent, ToolFormat, ToolHeartbeatEvent,
    ToolLoopFinishedEvent, ToolResultEvent, VectorMsg,
};
use crate::python_helpers::{parse_python_execution_args, reconstruct_sql_from_malformed_args};
use crate::repetition_detector::RepetitionDetector;
use crate::settings::{ChatFormatName, McpServerConfig, ToolCallFormatConfig, ToolCallFormatName};
use crate::state_machine::AgenticStateMachine;
use crate::tool_execution::{
    dispatch_tool_call_to_executor, execute_python_code, execute_tool_search,
    resolve_mcp_server_for_tool,
};
use crate::tool_parsing::{format_tool_result, parse_tool_calls_for_model_profile};
use crate::tool_registry::SharedToolRegistry;
use crate::tools::code_execution::CodeExecutionInput;
use crate::tools::schema_search::{SchemaSearchExecutor, SchemaSearchInput};
use crate::tools::tool_search::ToolSearchInput;

// ============================================================================
// Types
// ============================================================================

/// Result of deciding what the agentic loop should do with a model response.
#[derive(Debug, PartialEq)]
pub enum AgenticLoopAction {
    /// No tool calls detected, this is the final response
    Final { response: String },
    /// Tool calls were detected and should be executed
    ToolCalls { calls: Vec<ParsedToolCall> },
}

/// Configuration for the agentic loop.
///
/// Groups the configuration parameters that define loop behavior.
#[derive(Clone)]
pub struct AgenticLoopConfig {
    /// Unique identifier for this chat session
    pub chat_id: String,
    /// Generation ID for cancellation tracking
    pub generation_id: u32,
    /// Chat title for display
    pub title: String,
    /// Original user message that started this turn
    pub original_message: String,
    /// Model name to use for inference
    pub model_name: String,
    /// Reasoning effort level (e.g., "low", "medium", "high")
    pub reasoning_effort: String,
    /// Whether Python tool mode is enabled (Code Mode)
    pub python_tool_mode: bool,
    /// Tool call format configuration
    pub format_config: ToolCallFormatConfig,
    /// Primary tool call format to try first
    pub primary_format: ToolCallFormatName,
    /// Whether tool_search is allowed within python_execution
    pub allow_tool_search_for_python: bool,
    /// Maximum number of tools to return from tool_search
    pub tool_search_max_results: usize,
    /// System prompt for this turn
    pub turn_system_prompt: String,
    /// Default chat format
    pub chat_format_default: ChatFormatName,
    /// Per-model chat format overrides
    pub chat_format_overrides: HashMap<String, ChatFormatName>,
    /// Enabled database source IDs
    pub enabled_db_sources: Vec<String>,
    /// MCP server configurations
    pub server_configs: Vec<McpServerConfig>,
}

/// Actor handles and shared state for the agentic loop.
///
/// Groups the communication channels to various actors.
pub struct AgenticLoopHandles {
    /// Channel to Foundry model gateway
    pub foundry_tx: mpsc::Sender<FoundryMsg>,
    /// Channel to MCP tool router
    pub mcp_host_tx: mpsc::Sender<McpHostMsg>,
    /// Channel to vector store actor
    pub vector_tx: mpsc::Sender<VectorMsg>,
    /// Channel to Python sandbox actor
    pub python_tx: mpsc::Sender<PythonMsg>,
    /// Channel to schema vector store actor
    pub schema_tx: mpsc::Sender<SchemaVectorMsg>,
    /// Channel to database toolbox actor
    pub database_toolbox_tx: mpsc::Sender<DatabaseToolboxMsg>,
    /// Shared tool registry
    pub tool_registry: SharedToolRegistry,
    /// CPU embedding model for search
    pub embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    /// Pending tool approvals map
    pub pending_approvals: PendingApprovals,
}

// ============================================================================
// Action Detection
// ============================================================================

/// Decide whether a response should trigger tool execution or be treated as final text.
///
/// This function examines the model's response and determines the next action:
/// - If Python tool mode is enabled, looks for Python code blocks
/// - If tool call formats are enabled, parses for tool call syntax
/// - Otherwise, treats the response as final text
pub fn detect_agentic_loop_action(
    model_response_text: &str,
    model_family: ModelFamily,
    tool_format: ToolFormat,
    python_tool_mode: bool,
    formats: &ToolCallFormatConfig,
    primary_format: ToolCallFormatName,
) -> AgenticLoopAction {
    let non_code_formats_enabled = formats.any_non_code();

    if python_tool_mode {
        if let Some(code_lines) = extract_python_program_from_response(model_response_text) {
            if is_valid_python_syntax_check(&code_lines) {
                return AgenticLoopAction::ToolCalls {
                    calls: vec![ParsedToolCall {
                        server: "builtin".to_string(),
                        tool: "python_execution".to_string(),
                        arguments: json!({ "code": code_lines }),
                        raw: "[python_program]".to_string(),
                        id: None,
                    }],
                };
            }
        }

        if !non_code_formats_enabled {
            return AgenticLoopAction::Final {
                response: model_response_text.to_string(),
            };
        }
    }

    if non_code_formats_enabled {
        let parsed_tool_calls = parse_tool_calls_for_model_profile(
            model_response_text,
            model_family,
            tool_format,
            formats,
            primary_format,
        );
        if !parsed_tool_calls.is_empty() {
            return AgenticLoopAction::ToolCalls {
                calls: parsed_tool_calls,
            };
        }
    }

    AgenticLoopAction::Final {
        response: model_response_text.to_string(),
    }
}

/// Extract a Python program from the model response.
/// Prefers fenced ```python blocks, falls back to treating the whole message as code.
fn extract_python_program_from_response(response: &str) -> Option<Vec<String>> {
    use crate::tool_parsing::detect_python_code;

    let trimmed = response.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Prefer structured detections (fenced blocks, explicit python, dedented snippets)
    let detected_blocks = detect_python_code(trimmed);
    if let Some(block) = detected_blocks
        .iter()
        .find(|b| b.explicit_python)
        .or_else(|| detected_blocks.first())
    {
        let lines: Vec<String> = block
            .code
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        if !lines.is_empty() {
            return Some(lines);
        }
    }

    // Fallback: only accept inline snippets that clearly look like Python.
    // Do NOT treat arbitrary multi-line text as code.
    let looks_like_inline_python = regex::Regex::new(r"(?m)^\s*[A-Za-z_][A-Za-z0-9_]*\s*=\s*.+")
        .map(|re| re.is_match(trimmed))
        .unwrap_or(false)
        || trimmed.contains("print(")
        || trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.starts_with("def ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("while ")
        || trimmed.starts_with("if ")
        || trimmed.starts_with("with ");

    if looks_like_inline_python {
        return Some(
            trimmed
                .lines()
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect(),
        );
    }

    None
}

/// Quick syntax validation for Python code before execution to avoid looping on non-code text.
fn is_valid_python_syntax_check(code_lines: &[String]) -> bool {
    use rustpython_parser::{ast, Parse};

    let code = code_lines.join("\n");
    match ast::Suite::parse(&code, "<embedded>") {
        Ok(_) => true,
        Err(err) => {
            println!(
                "[PythonSyntaxCheck] Skipping python_execution due to parse error: {}",
                err
            );
            false
        }
    }
}

// ============================================================================
// Tool Execution Helpers
// ============================================================================

// NOTE: Tool approval is handled inline in run_agentic_loop using oneshot channels.
// The approval flow:
// 1. Create oneshot channel (tx, rx)
// 2. Store tx in pending_approvals HashMap
// 3. Emit "tool-calls-pending" event to frontend
// 4. Wait on rx with timeout
// 5. Frontend calls approve_tool_call or reject_tool_call which sends to tx

/// Execute a built-in tool call (tool_search, python_execution, schema_search, sql_select).
///
/// Returns `(result_text, is_error)`.
pub async fn execute_builtin_tool_call(
    tool_name: &str,
    arguments: &Value,
    handles: &AgenticLoopHandles,
    config: &AgenticLoopConfig,
    loop_iteration_index: usize,
    call_index: usize,
) -> (String, bool) {
    use std::io::Write;

    match tool_name {
        "tool_search" => {
            println!("[AgenticLoop] Executing built-in: tool_search");
            let _ = std::io::stdout().flush();
            let exec_start = std::time::Instant::now();

            // Parse tool_search input
            let input: ToolSearchInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid tool_search arguments: {}", e))
                .unwrap_or(ToolSearchInput {
                    queries: vec![],
                    top_k: config.tool_search_max_results,
                });

            match execute_tool_search(
                input,
                handles.tool_registry.clone(),
                handles.embedding_model.clone(),
                config.tool_search_max_results,
            )
            .await
            {
                Ok((result, discovered_tools)) => {
                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] tool_search completed in {:.2}s, found {} tools",
                        elapsed.as_secs_f64(),
                        discovered_tools.len()
                    );
                    (result, false)
                }
                Err(e) => {
                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] tool_search failed in {:.2}s: {}",
                        elapsed.as_secs_f64(),
                        e
                    );
                    (e, true)
                }
            }
        }

        "python_execution" => {
            println!("[AgenticLoop] Executing built-in: python_execution");
            let _ = std::io::stdout().flush();
            let exec_start = std::time::Instant::now();

            let input: CodeExecutionInput = parse_python_execution_args(arguments);
            let exec_id = format!(
                "{}-{}-{}",
                config.chat_id, loop_iteration_index, call_index
            );
            let code_lines = input.code.len();
            println!(
                "[AgenticLoop] python_execution triggered (exec_id={}, code_lines={})",
                exec_id, code_lines
            );

            match execute_python_code(
                input,
                exec_id,
                handles.tool_registry.clone(),
                &handles.python_tx,
                config.allow_tool_search_for_python,
            )
            .await
            {
                Ok(output) => {
                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] {} python_execution completed in {:.2}s",
                        if output.success { "OK" } else { "WARN" },
                        elapsed.as_secs_f64()
                    );

                    let has_stdout = !output.stdout.trim().is_empty();
                    let has_stderr = !output.stderr.trim().is_empty();
                    let result = if output.success {
                        match (has_stdout, has_stderr) {
                            (true, true) => {
                                format!("STDOUT:\n{}\n\nSTDERR:\n{}", output.stdout, output.stderr)
                            }
                            (true, false) => output.stdout,
                            (false, true) => format!("(no stdout)\nSTDERR:\n{}", output.stderr),
                            (false, false) => "(execution completed with no output)".to_string(),
                        }
                    } else {
                        format!("Error: {}", output.stderr)
                    };
                    (result, !output.success)
                }
                Err(e) => {
                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] python_execution failed in {:.2}s: {}",
                        elapsed.as_secs_f64(),
                        e
                    );
                    (e, true)
                }
            }
        }

        "schema_search" => {
            println!("[AgenticLoop] Executing built-in: schema_search");
            let _ = std::io::stdout().flush();
            let exec_start = std::time::Instant::now();

            let input: SchemaSearchInput = serde_json::from_value(arguments.clone())
                .unwrap_or_else(|e| {
                    println!(
                        "[AgenticLoop] Failed to parse schema_search args: {}, using defaults",
                        e
                    );
                    SchemaSearchInput {
                        query: String::new(),
                        max_tables: 10,
                        max_columns_per_table: 25,
                        min_relevance: 0.3,
                    }
                });

            let executor =
                SchemaSearchExecutor::new(handles.schema_tx.clone(), handles.embedding_model.clone());

            match executor.execute(input).await {
                Ok(mut output) => {
                    // Filter by enabled sources
                    let enabled: std::collections::HashSet<String> =
                        config.enabled_db_sources.iter().cloned().collect();
                    output.tables.retain(|t| enabled.contains(&t.source_id));

                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] schema_search completed in {:.2}s: {} tables found",
                        elapsed.as_secs_f64(),
                        output.tables.len()
                    );
                    (
                        serde_json::to_string_pretty(&output).unwrap_or_default(),
                        false,
                    )
                }
                Err(e) => {
                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] schema_search failed in {:.2}s: {}",
                        elapsed.as_secs_f64(),
                        e
                    );
                    (e, true)
                }
            }
        }

        "sql_select" => {
            println!("[AgenticLoop] Executing built-in: sql_select");
            let _ = std::io::stdout().flush();
            let exec_start = std::time::Instant::now();

            // Parse arguments, with reconstruction fallback for malformed SQL
            let sql = parse_sql_select_arguments(arguments);

            if sql.is_empty() {
                return (
                    "Error: No SQL query provided. Please provide a 'sql' argument.".to_string(),
                    true,
                );
            }

            // Resolve source_id from table names in the SQL query
            let source_id = match resolve_source_from_sql(
                &sql,
                &handles.schema_tx,
                &config.enabled_db_sources,
            )
            .await
            {
                Ok(id) => id,
                Err(e) => {
                    println!("[AgenticLoop] Failed to resolve source from SQL: {}", e);
                    // Return structured error for recovery
                    let error_json = serde_json::json!({
                        "sql_executed": sql,
                        "error": e,
                        "tables_extracted": extract_table_names_from_sql(&sql),
                    });
                    return (serde_json::to_string(&error_json).unwrap_or(e), true);
                }
            };

            // Execute via database toolbox
            let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
            if handles
                .database_toolbox_tx
                .send(DatabaseToolboxMsg::ExecuteSql {
                    source_id: source_id.clone(),
                    sql: sql.clone(),
                    parameters: vec![],
                    reply_to: respond_tx,
                })
                .await
                .is_err()
            {
                return ("Error: Failed to send query to database".to_string(), true);
            }

            match respond_rx.await {
                Ok(Ok(result)) => {
                    let elapsed = exec_start.elapsed();
                    let row_count = result.rows.len();
                    println!(
                        "[AgenticLoop] sql_select completed in {:.2}s: {} rows (source: {})",
                        elapsed.as_secs_f64(),
                        row_count,
                        source_id
                    );
                    (
                        serde_json::to_string_pretty(&result).unwrap_or_default(),
                        false,
                    )
                }
                Ok(Err(e)) => {
                    let elapsed = exec_start.elapsed();
                    println!(
                        "[AgenticLoop] sql_select failed in {:.2}s (source: {}): {}",
                        elapsed.as_secs_f64(),
                        source_id,
                        e
                    );
                    // Return structured error for recovery
                    let error_json = serde_json::json!({
                        "sql_executed": sql,
                        "error": e,
                    });
                    (serde_json::to_string(&error_json).unwrap_or(e), true)
                }
                Err(_) => ("Error: Database actor died".to_string(), true),
            }
        }

        _ => {
            // Unknown built-in tool
            (
                format!("Error: Unknown built-in tool '{}'", tool_name),
                true,
            )
        }
    }
}

/// Parse sql_select arguments, handling malformed input.
/// Returns the SQL query string.
fn parse_sql_select_arguments(arguments: &Value) -> String {
    // Try standard format first
    if let Some(sql) = arguments.get("sql").and_then(|v| v.as_str()) {
        return sql.to_string();
    }

    // Try reconstruction for malformed arguments
    if let Some(reconstructed) = reconstruct_sql_from_malformed_args(arguments) {
        println!(
            "[AgenticLoop] Reconstructed SQL: {}...",
            reconstructed.chars().take(50).collect::<String>()
        );
        return reconstructed;
    }

    String::new()
}

/// Extract table names from a SQL query.
/// Handles patterns like FROM table, FROM schema.table, JOIN table, etc.
fn extract_table_names_from_sql(sql: &str) -> Vec<String> {
    let mut tables = Vec::new();
    let sql_tokens: Vec<&str> = sql.split_whitespace().collect();
    
    // Find positions of FROM and JOIN keywords
    for (i, token) in sql_tokens.iter().enumerate() {
        let token_upper = token.to_uppercase();
        if (token_upper == "FROM" || token_upper == "JOIN") && i + 1 < sql_tokens.len() {
            // Next token is the table name
            let table_token = sql_tokens[i + 1];
            // Clean up the table name (remove trailing commas, parentheses, etc.)
            let table_name = table_token
                .trim_matches(|c| c == '(' || c == ')' || c == ',' || c == ';')
                .to_string();
            
            // Skip if it's a subquery or keyword
            if !table_name.is_empty() 
                && !table_name.starts_with('(')
                && !["SELECT", "WHERE", "ON", "AS", "LEFT", "RIGHT", "INNER", "OUTER", "CROSS", "NATURAL"]
                    .contains(&table_name.to_uppercase().as_str())
            {
                tables.push(table_name);
            }
        }
    }
    
    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    tables.retain(|t| seen.insert(t.clone()));
    
    tables
}

/// Resolve source_id from table names in SQL using the schema vector store.
async fn resolve_source_from_sql(
    sql: &str,
    schema_tx: &mpsc::Sender<SchemaVectorMsg>,
    enabled_db_sources: &[String],
) -> Result<String, String> {
    let table_names = extract_table_names_from_sql(sql);
    
    if table_names.is_empty() {
        return Err("Could not extract table name from SQL query. Ensure the query has a FROM or JOIN clause.".to_string());
    }
    
    // Use the first table to determine the source
    let first_table = &table_names[0];
    
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if schema_tx
        .send(SchemaVectorMsg::LookupTableSource {
            table_name: first_table.clone(),
            enabled_sources: enabled_db_sources.to_vec(),
            respond_to: reply_tx,
        })
        .await
        .is_err()
    {
        return Err("Failed to send lookup request to schema store".to_string());
    }
    
    match reply_rx.await {
        Ok(Ok((source_id, _fq_name))) => Ok(source_id),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Schema store lookup failed".to_string()),
    }
}

// ============================================================================
// Main Loop
// ============================================================================

/// Maximum number of tool call iterations before stopping (safety limit).
const MAX_LOOP_ITERATIONS: usize = 20;

/// Run the agentic loop: call model, detect tool calls, execute, repeat.
///
/// This is the core execution loop that:
/// 1. Sends the current history to the model
/// 2. Receives and processes the streaming response
/// 3. Detects tool calls in the response
/// 4. Executes tools (with approval if required)
/// 5. Adds results to history and continues until a final response
pub async fn run_agentic_loop(
    handles: AgenticLoopHandles,
    config: AgenticLoopConfig,
    app_handle: tauri::AppHandle,
    mut full_history: Vec<ChatMessage>,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    mut openai_tools: Option<Vec<OpenAITool>>,
    turn_progress: Arc<RwLock<TurnProgress>>,
    mut state_machine: AgenticStateMachine,
) {
    use std::io::Write;

    // Derive native tool calling from format config
    let native_tool_calling_enabled = config.format_config.native_enabled();

    // Resolve model profile from model name
    let profile = resolve_profile(&config.model_name);
    let model_family = profile.model_family;
    let tool_format = profile.tool_call_format;
    let mut loop_iteration_index = 0;
    let mut had_tool_calls = false;
    let mut final_response = String::new();

    // Track repeated errors to detect when model is stuck
    let mut last_error_signature: Option<String> = None;
    let mut tools_disabled_due_to_repeated_error = false;

    let verbose_logging = crate::is_verbose_logging_enabled();
    
    // Test emit to verify app_handle works in spawned task
    println!("[AgenticLoop] Testing event emit from spawned task...");
    match app_handle.emit("agentic-loop-started", &config.chat_id) {
        Ok(_) => println!("[AgenticLoop] ✅ Test emit succeeded"),
        Err(e) => println!("[AgenticLoop] ❌ Test emit failed: {}", e),
    }

    // Current system prompt - regenerated by state machine after transitions
    #[allow(unused_assignments)]
    let mut current_system_prompt = config.turn_system_prompt.clone();

    println!(
        "[AgenticLoop] Starting with model_family={:?}, tool_format={:?}, python_tool_mode={}, primary_format={:?}, state={:?}",
        model_family, tool_format, config.python_tool_mode, config.primary_format, state_machine.current_state().name()
    );
    let _ = std::io::stdout().flush();

    loop {
        println!(
            "\n[AgenticLoop] Iteration {} starting...",
            loop_iteration_index
        );
        let iteration_start = std::time::Instant::now();
        let _ = std::io::stdout().flush();

        // Log materialized tools from previous iteration
        if loop_iteration_index > 0 {
            let registry = handles.tool_registry.read().await;
            let stats = registry.stats();
            if stats.materialized_tools > 0 {
                println!(
                    "[AgenticLoop] {} materialized tools available from previous iteration",
                    stats.materialized_tools
                );
            }
        }

        // Build chat request
        let chat_format = config
            .chat_format_overrides
            .get(&config.model_name)
            .cloned()
            .unwrap_or(config.chat_format_default);

        // Create streaming channel and cancellation for this iteration
        let (token_tx, token_rx) = tokio::sync::mpsc::unbounded_channel();
        let (iter_cancel_tx, iter_cancel_rx) = tokio::sync::watch::channel(false);

        // Forward external cancellation to iteration cancellation
        let mut external_cancel = cancel_rx.clone();
        let iter_cancel_fwd = iter_cancel_tx.clone();
        tokio::spawn(async move {
            while external_cancel.changed().await.is_ok() {
                if *external_cancel.borrow() {
                    let _ = iter_cancel_fwd.send(true);
                    break;
                }
            }
        });

        // Clone iter_cancel_rx before moving into the request
        let iter_cancel_for_stream = iter_cancel_rx.clone();
        
        let chat_request = FoundryMsg::Chat {
            model: config.model_name.clone(),
            chat_history_messages: full_history.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            native_tool_specs: openai_tools.clone(),
            native_tool_calling_enabled,
            chat_format_default: chat_format,
            chat_format_overrides: config.chat_format_overrides.clone(),
            respond_to: token_tx,
            stream_cancel_rx: iter_cancel_for_stream,
        };
        let mut token_rx = token_rx;

        println!("[AgenticLoop] Sending chat request to Foundry...");
        let _ = std::io::stdout().flush();

        if handles.foundry_tx.send(chat_request).await.is_err() {
            println!("[AgenticLoop] ERROR: Failed to send to Foundry");
            let _ = app_handle.emit(
                "chat-error",
                serde_json::json!({ "error": "Failed to send to model gateway" }),
            );
            break;
        }

        println!("[AgenticLoop] Request sent, waiting for tokens...");
        let _ = std::io::stdout().flush();

        // Receive streaming response
        let mut model_response_text = String::new();
        let mut token_count = 0;
        let mut first_token_received = false;
        let iteration_start_time = std::time::Instant::now();
        let mut repetition_detector = RepetitionDetector::new();
        #[allow(unused_assignments)]
        let mut early_stopped_for_tool = false;
        let mut iter_cancel_check = iter_cancel_rx.clone();

        // Token streaming loop
        loop {
            tokio::select! {
                _ = iter_cancel_check.changed() => {
                    if *iter_cancel_check.borrow() {
                        if *cancel_rx.borrow() {
                            println!("[AgenticLoop] User cancellation received!");
                        } else {
                            println!("[AgenticLoop] Internal early-stop cancellation triggered.");
                            // Note: early_stopped_for_tool not set here since we break immediately
                        }
                        break;
                    }
                }
                token_result = token_rx.recv() => {
                    match token_result {
                        Some(token) => {
                            if !first_token_received {
                                first_token_received = true;
                                let ttft = iteration_start_time.elapsed();
                                println!("[AgenticLoop] First token received! TTFT: {:.2}s", ttft.as_secs_f64());
                            }

                            model_response_text.push_str(&token);
                            token_count += 1;

                            // Update TurnProgress for reconciliation
                            if let Ok(mut progress) = turn_progress.try_write() {
                                progress.assistant_response.push_str(&token);
                                progress.last_token_index = token_count;
                                progress.timestamp_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis())
                                    .unwrap_or(0);
                            }

                            // Repetition detection
                            repetition_detector.push(&token);
                            if let Some((pattern, repetitions)) = repetition_detector.detect_loop() {
                                let score = pattern.len() * repetitions;
                                println!("[AgenticLoop] LOOP DETECTED: '{}' repeated {} times (score={})", pattern, repetitions, score);
                                // Emit model-stuck event for frontend
                                let _ = app_handle.emit(
                                    "model-stuck",
                                    serde_json::json!({
                                        "pattern": pattern,
                                        "repetitions": repetitions,
                                        "score": score
                                    }),
                                );
                                // Cancel generation to prevent infinite loop
                                let _ = iter_cancel_tx.send(true);
                            }

                            // Emit token to frontend
                            match app_handle.emit("chat-token", &token) {
                                Ok(_) => {
                                    if token_count == 1 {
                                        println!("[AgenticLoop] ✅ First token emitted to frontend");
                                    }
                                }
                                Err(e) => {
                                    println!("[AgenticLoop] ❌ Failed to emit token {}: {}", token_count, e);
                                }
                            }

                            // Early tool call detection to prevent hallucination
                            if !early_stopped_for_tool
                                && model_response_text.contains("</tool_call>")
                            {
                                println!("[AgenticLoop] Detected complete tool call during streaming, stopping early.");
                                let _ = iter_cancel_tx.send(true);
                                early_stopped_for_tool = true;
                            }

                            if verbose_logging && token_count % 50 == 0 {
                                println!(
                                    "[AgenticLoop] Receiving: {} tokens, {} chars",
                                    token_count,
                                    model_response_text.len()
                                );
                            }
                        }
                        None => {
                            println!("[AgenticLoop] Channel closed, stream complete");
                            break;
                        }
                    }
                }
            }
        }

        let stream_elapsed = iteration_start.elapsed();
        println!(
            "[AgenticLoop] Response complete: {} tokens, {} chars in {:.2}s",
            token_count,
            model_response_text.len(),
            stream_elapsed.as_secs_f64()
        );

        // Always log the full response for debugging
        println!(
            "[AgenticLoop] Full model response:\n---\n{}\n---",
            model_response_text
        );

        // Detect action (tool calls vs final response)
        let action = if tools_disabled_due_to_repeated_error {
            AgenticLoopAction::Final {
                response: model_response_text.clone(),
            }
        } else {
            detect_agentic_loop_action(
                &model_response_text,
                model_family,
                tool_format,
                config.python_tool_mode,
                &config.format_config,
                config.primary_format,
            )
        };

        let parsed_tool_calls = match action {
            AgenticLoopAction::Final { response } => {
                println!("[AgenticLoop] No tool calls detected, loop complete");
                final_response = response;
                break;
            }
            AgenticLoopAction::ToolCalls { calls } => calls,
        };

        // Safety: max iterations
        if loop_iteration_index >= MAX_LOOP_ITERATIONS {
            println!(
                "[AgenticLoop] Max iterations ({}) reached, stopping",
                MAX_LOOP_ITERATIONS
            );
            final_response = model_response_text.clone();

            // Emit warning
            let _ = app_handle.emit(
                "chat-warning",
                serde_json::json!({
                    "message": format!("Stopped after {} iterations (safety limit)", MAX_LOOP_ITERATIONS)
                }),
            );
            break;
        }

        println!(
            "[AgenticLoop] Found {} tool call(s)",
            parsed_tool_calls.len()
        );
        had_tool_calls = true;

        // Check if native format
        let use_native_results =
            should_use_native_tool_results(native_tool_calling_enabled, &parsed_tool_calls);
        if use_native_results {
            println!("[AgenticLoop] Using native OpenAI tool result format");
        }

        // Resolve servers for tools
        let mut resolved_tool_calls: Vec<ParsedToolCall> = Vec::new();
        for call in &parsed_tool_calls {
            let resolved_server = if is_builtin_tool(&call.tool) {
                "builtin".to_string()
            } else if call.server == "unknown" {
                match resolve_mcp_server_for_tool(&handles.mcp_host_tx, &call.tool).await {
                    Some(server_id) => {
                        println!(
                            "[AgenticLoop] Resolved unknown server to '{}' for tool '{}'",
                            server_id, call.tool
                        );
                        server_id
                    }
                    None => {
                        println!(
                            "[AgenticLoop] ERROR: Could not resolve server for tool '{}', skipping",
                            call.tool
                        );
                        continue;
                    }
                }
            } else {
                call.server.clone()
            };

            resolved_tool_calls.push(ParsedToolCall {
                server: resolved_server,
                tool: call.tool.clone(),
                arguments: call.arguments.clone(),
                raw: call.raw.clone(),
                id: call.id.clone(),
            });
        }

        // Add assistant message with tool calls to history
        let assistant_msg = create_assistant_message_with_tool_calls(
            &model_response_text,
            &resolved_tool_calls,
            use_native_results,
            None,
        );
        full_history.push(assistant_msg);

        // Execute each tool call
        let mut tool_results: Vec<(ParsedToolCall, String, bool)> = Vec::new();
        let mut executed_any = false;

        for (idx, resolved_tool_call) in resolved_tool_calls.iter().enumerate() {
            // Check if blocked by state machine
            if !state_machine.is_tool_allowed(&resolved_tool_call.tool) {
                let current_state = state_machine.current_state().name();
                println!(
                    "[AgenticLoop] Tool '{}' blocked by state machine (state: {})",
                    resolved_tool_call.tool,
                    current_state
                );
                // Emit tool-blocked event for frontend
                let _ = app_handle.emit(
                    "tool-blocked",
                    serde_json::json!({
                        "tool": resolved_tool_call.tool,
                        "state": current_state,
                        "message": format!("Tool '{}' not allowed in '{}' state", resolved_tool_call.tool, current_state)
                    }),
                );
                continue;
            }

            // Check if approval required
            let requires_approval = if resolved_tool_call.server == "builtin" {
                false
            } else {
                !config
                    .server_configs
                    .iter()
                    .find(|c| c.id == resolved_tool_call.server)
                    .map(|c| c.auto_approve_tools)
                    .unwrap_or(false)
            };

            if requires_approval {
                // Emit pending event and wait for approval
                let approval_key = format!(
                    "{}:{}:{}:{}",
                    config.chat_id, config.generation_id, loop_iteration_index, idx
                );
                println!(
                    "[AgenticLoop] Server {} requires manual approval",
                    resolved_tool_call.server
                );

                // Create oneshot channel for this approval
                let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
                {
                    let mut approvals = handles.pending_approvals.write().await;
                    approvals.insert(approval_key.clone(), approval_tx);
                }

                // Emit pending event
                let _ = app_handle.emit(
                    "tool-calls-pending",
                    ToolCallsPendingEvent {
                        approval_key: approval_key.clone(),
                        calls: vec![resolved_tool_call.clone()],
                        iteration: loop_iteration_index,
                    },
                );

                println!("[AgenticLoop] Waiting for approval on key: {}", approval_key);

                // Wait for decision with timeout
                let approval_result = tokio::time::timeout(
                    Duration::from_secs(300),
                    approval_rx,
                )
                .await;

                match approval_result {
                    Ok(Ok(ToolApprovalDecision::Approved)) => {
                        println!("[AgenticLoop] Tool call approved by user");
                    }
                    Ok(Ok(ToolApprovalDecision::Rejected)) => {
                        println!("[AgenticLoop] Tool call rejected by user");
                        continue;
                    }
                    Ok(Err(_)) => {
                        println!("[AgenticLoop] Approval channel closed");
                        continue;
                    }
                    Err(_) => {
                        println!("[AgenticLoop] Approval timed out");
                        // Remove from pending
                        let mut approvals = handles.pending_approvals.write().await;
                        approvals.remove(&approval_key);
                        continue;
                    }
                }
            }

            // Emit executing event
            let _ = app_handle.emit(
                "tool-executing",
                ToolExecutingEvent {
                    server: resolved_tool_call.server.clone(),
                    tool: resolved_tool_call.tool.clone(),
                    arguments: resolved_tool_call.arguments.clone(),
                },
            );

            println!(
                "[AgenticLoop] Processing tool call {}/{}: {}::{}",
                idx + 1,
                resolved_tool_calls.len(),
                resolved_tool_call.server,
                resolved_tool_call.tool
            );

            // Start heartbeat
            let heartbeat_handle = app_handle.clone();
            let heartbeat_server = resolved_tool_call.server.clone();
            let heartbeat_tool = resolved_tool_call.tool.clone();
            let (heartbeat_stop_tx, mut heartbeat_stop_rx) = tokio::sync::oneshot::channel::<()>();
            let heartbeat_start = std::time::Instant::now();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                let mut beat_counter: u64 = 0;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            beat_counter += 1;
                            let _ = heartbeat_handle.emit(
                                "tool-heartbeat",
                                ToolHeartbeatEvent {
                                    server: heartbeat_server.clone(),
                                    tool: heartbeat_tool.clone(),
                                    elapsed_ms: heartbeat_start.elapsed().as_millis() as u64,
                                    beat: beat_counter,
                                },
                            );
                        }
                        _ = &mut heartbeat_stop_rx => {
                            break;
                        }
                    }
                }
            });

            // Execute the tool
            let (result_text, is_error) = if is_builtin_tool(&resolved_tool_call.tool) {
                execute_builtin_tool_call(
                    &resolved_tool_call.tool,
                    &resolved_tool_call.arguments,
                    &handles,
                    &config,
                    loop_iteration_index,
                    idx,
                )
                .await
            } else {
                // MCP tool execution
                match dispatch_tool_call_to_executor(&handles.mcp_host_tx, resolved_tool_call).await
                {
                    Ok(result) => {
                        println!(
                            "[AgenticLoop] MCP tool {} completed: {} chars",
                            resolved_tool_call.tool,
                            result.len()
                        );
                        (result, false)
                    }
                    Err(e) => {
                        println!(
                            "[AgenticLoop] MCP tool {} failed: {}",
                            resolved_tool_call.tool, e
                        );
                        (e, true)
                    }
                }
            };

            // Stop heartbeat
            let _ = heartbeat_stop_tx.send(());

            // Emit result
            let _ = app_handle.emit(
                "tool-result",
                ToolResultEvent {
                    server: resolved_tool_call.server.clone(),
                    tool: resolved_tool_call.tool.clone(),
                    result: result_text.clone(),
                    is_error,
                },
            );

            // Clone result for state machine before moving into tool_results
            let result_for_state = result_text.clone();
            tool_results.push((resolved_tool_call.clone(), result_text, is_error));
            executed_any = true;

            // Handle state machine transitions via events
            if resolved_tool_call.tool == "sql_select" && !is_error {
                use crate::agentic_state::{StateEvent, SqlResults};
                let prev_state = state_machine.current_state().name().to_string();
                // Parse row count from result if possible
                let row_count = serde_json::from_str::<serde_json::Value>(&result_for_state)
                    .ok()
                    .and_then(|v| v.get("row_count").and_then(|c| c.as_u64()))
                    .unwrap_or(0) as usize;
                state_machine.handle_event(StateEvent::SqlExecuted {
                    results: SqlResults {
                        columns: vec![],
                        rows: vec![],
                        row_count,
                        truncated: false,
                    },
                    row_count,
                });
                let new_state = state_machine.current_state().name();
                if prev_state != new_state {
                    println!(
                        "[AgenticLoop] State transition: {} -> {} (sql_select completed)",
                        prev_state, new_state
                    );
                }
            } else if resolved_tool_call.tool == "sql_select" && is_error {
                // SQL failed - transition to error recovery state
                use crate::agentic_state::StateEvent;
                let prev_state = state_machine.current_state().name().to_string();
                
                // Parse error details from the result
                let (sql, error, source_id) = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result_for_state) {
                    (
                        parsed.get("sql_executed").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        parsed.get("error").and_then(|v| v.as_str()).unwrap_or(&result_for_state).to_string(),
                        parsed.get("source_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    )
                } else {
                    (String::new(), result_for_state.clone(), String::new())
                };
                
                state_machine.handle_event(StateEvent::SqlFailed {
                    sql,
                    error,
                    source_id,
                });
                
                let new_state = state_machine.current_state().name();
                println!(
                    "[AgenticLoop] State transition: {} -> {} (sql_select FAILED - enabling retry)",
                    prev_state, new_state
                );
            } else if resolved_tool_call.tool == "schema_search" && !is_error {
                use crate::agentic_state::StateEvent;
                state_machine.handle_event(StateEvent::SchemaSearched {
                    tables: vec![],
                    max_relevancy: 0.0,
                });
            } else if resolved_tool_call.tool == "python_execution" {
                use crate::agentic_state::StateEvent;
                state_machine.handle_event(StateEvent::PythonExecuted {
                    stdout: result_for_state.clone(),
                    stderr: String::new(),
                });
            } else if resolved_tool_call.tool == "tool_search" {
                use crate::agentic_state::StateEvent;
                state_machine.handle_event(StateEvent::ToolSearchCompleted {
                    discovered: vec![],
                    schemas: vec![],
                });
            }
        }

        if !executed_any {
            println!("[AgenticLoop] No tools executed (all require approval), stopping loop");
            break;
        }

        // Add tool results to history
        if use_native_results {
            println!(
                "[AgenticLoop] Adding {} native tool result messages to history",
                tool_results.len()
            );
            for (call, result, _is_error) in &tool_results {
                if let Some(ref tool_call_id) = call.id {
                    let result_msg = create_native_tool_result_message(tool_call_id, result);
                    full_history.push(result_msg);
                }
            }
        } else {
            // Text-based format: append results to a user message
            let mut combined_results = String::new();
            for (call, result, is_error) in &tool_results {
                let schema_context = state_machine.get_compact_schema_context();
                let formatted = format_tool_result(
                    call,
                    result,
                    *is_error,
                    tool_format,
                    Some(&config.original_message),
                    schema_context.as_deref(),
                );
                combined_results.push_str(&formatted);
                combined_results.push_str("\n\n");
            }

            full_history.push(ChatMessage {
                role: "user".to_string(),
                content: combined_results,
                system_prompt: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Check for repeated errors
        for (call, result, is_error) in &tool_results {
            if *is_error {
                let error_sig = format!("{}::{}", call.tool, result.chars().take(100).collect::<String>());
                if last_error_signature.as_ref() == Some(&error_sig) {
                    println!(
                        "[AgenticLoop] REPEATED ERROR DETECTED: Tool '{}' failed with same error twice",
                        call.tool
                    );
                    println!("[AgenticLoop] Disabling tool calling, prompting model to answer directly");
                    tools_disabled_due_to_repeated_error = true;
                    openai_tools = None;
                    break;
                }
                last_error_signature = Some(error_sig);
            }
        }

        // Update system prompt from state machine if changed
        let should_continue = state_machine.should_continue_loop();
        println!(
            "[AgenticLoop] State machine: state={}, should_continue={}",
            state_machine.current_state().name(),
            should_continue
        );

        if !should_continue {
            final_response = model_response_text.clone();
            break;
        }

        // Update system prompt in history based on current state
        let new_prompt = state_machine.build_system_prompt();
        if new_prompt != current_system_prompt {
            current_system_prompt = new_prompt.clone();
            if let Some(first_msg) = full_history.first_mut() {
                if first_msg.role == "system" || first_msg.system_prompt.is_some() {
                    first_msg.system_prompt = Some(new_prompt.clone());
                    println!(
                        "[AgenticLoop] System prompt updated for state: {} ({} chars)",
                        state_machine.current_state().name(),
                        new_prompt.len()
                    );
                }
            }
        }

        println!(
            "[AgenticLoop] Continuing to iteration {} (state: {})...",
            loop_iteration_index + 1,
            state_machine.current_state().name()
        );
        loop_iteration_index += 1;
    }

    println!(
        "[AgenticLoop] Loop complete after {} iterations, had_tool_calls={}",
        loop_iteration_index, had_tool_calls
    );

    // Emit loop finished
    let _ = app_handle.emit(
        "tool-loop-finished",
        ToolLoopFinishedEvent {
            iterations: loop_iteration_index,
            had_tool_calls,
        },
    );

    // Save chat to vector store
    save_chat_to_vector_store(
        &handles.vector_tx,
        &config.chat_id,
        &config.title,
        &config.original_message,
        &final_response,
        &handles.embedding_model,
    )
    .await;

    // Emit chat-saved event for frontend
    let _ = app_handle.emit("chat-saved", &config.chat_id);

    // Mark turn as complete in TurnProgress
    {
        let mut progress = turn_progress.write().await;
        progress.active = false;
        progress.finished = true;
        progress.had_tool_calls = had_tool_calls;
        progress.assistant_response = final_response.clone();
        progress.timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
    }

    // Signal chat completion to frontend
    println!("[AgenticLoop] Emitting chat-finished event to frontend");
    match app_handle.emit("chat-finished", ()) {
        Ok(_) => println!("[AgenticLoop] ✅ chat-finished event emitted successfully"),
        Err(e) => println!("[AgenticLoop] ❌ Failed to emit chat-finished: {}", e),
    }
}

/// Save the chat to the vector store for semantic search.
async fn save_chat_to_vector_store(
    vector_tx: &mpsc::Sender<VectorMsg>,
    chat_id: &str,
    title: &str,
    user_message: &str,
    assistant_response: &str,
    embedding_model: &Arc<RwLock<Option<Arc<TextEmbedding>>>>,
) {
    // Combine for embedding
    let content = format!("User: {}\n\nAssistant: {}", user_message, assistant_response);

    // Get embedding
    let model_guard = embedding_model.read().await;
    let embedding = if let Some(model) = model_guard.as_ref() {
        match model.embed(vec![content.clone()], None) {
            Ok(embeddings) if !embeddings.is_empty() => Some(embeddings[0].clone()),
            _ => None,
        }
    } else {
        None
    };
    drop(model_guard);

    // Save to vector store
    let _ = vector_tx
        .send(VectorMsg::UpsertChatRecord {
            id: chat_id.to_string(),
            title: title.to_string(),
            content,
            messages: String::new(), // Full history would be serialized here
            embedding_vector: embedding,
            pinned: false,
            model: None,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_final_response() {
        let action = detect_agentic_loop_action(
            "Hello, how can I help you?",
            ModelFamily::Phi,
            ToolFormat::Hermes,
            false,
            &ToolCallFormatConfig::default(),
            ToolCallFormatName::Hermes,
        );

        match action {
            AgenticLoopAction::Final { response } => {
                assert_eq!(response, "Hello, how can I help you?");
            }
            AgenticLoopAction::ToolCalls { .. } => {
                panic!("Expected Final, got ToolCalls");
            }
        }
    }

    #[test]
    fn test_detect_tool_call() {
        let response = r#"<tool_call>{"name": "sql_select", "arguments": {"sql": "SELECT 1"}}</tool_call>"#;
        let mut config = ToolCallFormatConfig::default();
        config.enabled = vec![ToolCallFormatName::Hermes];

        let action = detect_agentic_loop_action(
            response,
            ModelFamily::Phi,
            ToolFormat::Hermes,
            false,
            &config,
            ToolCallFormatName::Hermes,
        );

        match action {
            AgenticLoopAction::ToolCalls { calls } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].tool, "sql_select");
            }
            AgenticLoopAction::Final { .. } => {
                panic!("Expected ToolCalls, got Final");
            }
        }
    }
}
