// =============================================================================
// Module Organization Strategy
// =============================================================================
// Tauri commands are organized into domain-specific modules under `commands/`
// to keep this file focused and maintainable. See `src/AGENTS.md` for details.
//
// - New commands go in `commands/*.rs`, NOT here
// - Commands are re-exported via `commands/mod.rs` and imported below via `use commands::*`
// - This file retains: module declarations, core agentic loop (`chat`), app init (`run`)
// =============================================================================

pub mod actors;
pub mod agentic_loop;
pub mod agentic_state;
pub mod app_state;
pub mod auto_discovery;
pub mod cli;
pub mod crash_handler;
pub mod demo_schema;
pub mod message_builders;
pub mod mid_turn_state;
pub mod model_profiles;
pub mod paths;
pub mod process_utils;
pub mod protocol;
pub mod python_helpers;
pub mod repetition_detector;
pub mod settings;
pub mod settings_state_machine;
pub mod state_machine;
pub mod system_prompt;
pub mod tabular_parser;
pub mod tool_execution;
pub mod tool_parsing;
pub mod tool_capability;
pub mod tool_registry;
pub mod tools;
pub mod commands;

#[cfg(test)]
mod tests;

use actors::database_toolbox_actor::DatabaseToolboxActor;
use actors::foundry::ModelGatewayActor;
use actors::mcp_host_actor::{McpToolRouterActor, McpTool};
use actors::python_actor::PythonSandboxActor;
use actors::rag::RagRetrievalActor;
use actors::schema_vector_actor::{SchemaVectorStoreActor, SchemaVectorMsg};
use actors::startup_actor::StartupCoordinatorActor;
use actors::vector_actor::ChatVectorStoreActor;
use app_state::{
    ActorHandles, CancellationState, EmbeddingModelState, GpuResourceGuard, HeartbeatState,
    LaunchConfigState, LoggingPersistence, SettingsState,
    SettingsStateMachineState, SystemPromptEvent, ToolApprovalState,
    ToolRegistryState, TurnProgress, TurnTrackerState,
};
use clap::Parser;
use cli::{apply_cli_overrides, parse_tool_filter, CliArgs};
use mcp_test_server::{
    run_with_args as run_mcp_test_server, CliArgs as McpTestCliArgs,
};
use crate::agentic_state::McpToolInfo;
use crate::protocol::{
    ChatMessage, FoundryMsg, McpHostMsg, ModelFamily, ModelInfo, OpenAITool,
    RagMsg, ToolFormat, ToolSchema,
};
use settings::ToolCallFormatName;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;
use tokio::sync::{mpsc, oneshot};
use tool_capability::ToolCapabilityResolver;
use tool_registry::{create_shared_registry, SharedToolRegistry};
use settings_state_machine::{SettingsStateMachine, ChatTurnContext};
use state_machine::AgenticStateMachine;
use tools::tool_search::precompute_tool_search_embeddings;
use tools::schema_search::select_columns_hybrid;
use uuid::Uuid;

// Extracted modules
use agentic_loop::{AgenticLoopConfig, AgenticLoopHandles, run_agentic_loop};
use auto_discovery::perform_auto_discovery_for_prompt;

// Import all Tauri commands from domain-specific modules (see commands/mod.rs)
// This keeps lib.rs lean while making commands available for the invoke_handler
use commands::*;

/// Fix the PATH environment variable for macOS GUI applications.
///
/// macOS GUI applications (launched from Finder, Spotlight, or Dock) don't inherit
/// the user's shell PATH from dotfiles (.zshrc, .bashrc, etc.). This causes issues
/// when trying to find executables like `foundry` that are installed via Homebrew
/// or other package managers.
///
/// This function spawns the user's default shell in login mode to source their
/// dotfiles, then extracts and applies the resulting PATH to the current process.
#[cfg(target_os = "macos")]
fn fix_macos_path_env() -> Result<(), String> {
    use std::process::Command;
    
    // Get the user's default shell from the SHELL environment variable
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    
    // Run the shell in login mode (-l) to source dotfiles, then echo PATH
    // We use -i for interactive mode which sources .zshrc/.bashrc
    // The printf ensures we get just the PATH without a trailing newline
    let output = Command::new(&shell)
        .args(["-l", "-i", "-c", "printf '%s' \"$PATH\""])
        .output()
        .map_err(|e| format!("Failed to spawn shell '{}': {}", shell, e))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Shell exited with error: {}", stderr));
    }
    
    let new_path = String::from_utf8_lossy(&output.stdout).to_string();
    
    if new_path.is_empty() {
        return Err("Shell returned empty PATH".to_string());
    }
    
    // Get the current PATH to compare
    let current_path = std::env::var("PATH").unwrap_or_default();
    
    // Only update if the new PATH is different and longer (more entries)
    if new_path != current_path && new_path.len() > current_path.len() {
        println!("[Launch] Fixing macOS PATH for GUI application");
        println!("[Launch] Old PATH entries: {}", current_path.matches(':').count() + 1);
        println!("[Launch] New PATH entries: {}", new_path.matches(':').count() + 1);
        std::env::set_var("PATH", &new_path);
    } else {
        println!("[Launch] PATH already contains shell entries, no fix needed");
    }
    
    Ok(())
}

/// Global toggle for verbose logging, enabled when LOG_VERBOSE (or PLUGABLE_LOG_VERBOSE)
/// is set to a truthy value such as 1/true/yes/on/debug.
pub fn is_verbose_logging_enabled() -> bool {
    static VERBOSE_LOGS_ENABLED: OnceLock<bool> = OnceLock::new();

    *VERBOSE_LOGS_ENABLED.get_or_init(|| {
        std::env::var("LOG_VERBOSE")
            .or_else(|_| std::env::var("PLUGABLE_LOG_VERBOSE"))
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on" | "debug"
                )
            })
            .unwrap_or(false)
    })
}

/// Maximum number of numeric columns to include per attached table in the system prompt.
/// Non-numeric columns (TEXT, DATE, BOOLEAN, etc.) are always included.
const MAX_NUMERIC_COLUMNS_PER_TABLE: usize = 10;

/// Maximum columns to fetch from semantic search when building attached table schemas.
/// This should be >= MAX_NUMERIC_COLUMNS_PER_TABLE to get enough candidates.
const SEMANTIC_COLUMN_SEARCH_LIMIT: usize = 30;

/// Build filtered schema_text for an attached table using the hybrid column selection strategy:
/// - ALL non-numeric columns are included (TEXT, DATE, BOOLEAN, etc.)
/// - Top N numeric columns are selected (via semantic search if available)
/// 
/// This is the same strategy used by the schema_search tool, ensuring consistent behavior.
fn build_filtered_schema_text(
    cached: &settings::CachedTableSchema,
    semantic_column_names: Option<&HashSet<String>>,
) -> String {
    let mut schema_text = format!("Table: {} [{} Syntax]\n", cached.fully_qualified_name, cached.sql_dialect);
    
    if let Some(ref desc) = cached.description {
        schema_text.push_str(&format!("Description: {}\n", desc));
    }
    
    // Build key columns section (always shown)
    let mut key_info = Vec::new();
    if !cached.primary_keys.is_empty() {
        key_info.push(format!("PK: {}", cached.primary_keys.join(", ")));
    }
    if !cached.partition_columns.is_empty() {
        key_info.push(format!("Partition: {}", cached.partition_columns.join(", ")));
    }
    if !cached.cluster_columns.is_empty() {
        key_info.push(format!("Cluster: {}", cached.cluster_columns.join(", ")));
    }
    if !key_info.is_empty() {
        schema_text.push_str(&format!("Key columns: {}\n", key_info.join(" | ")));
    }
    
    // Use the shared hybrid column selection strategy:
    // - All non-numeric columns (TEXT, DATE, BOOLEAN, etc.) are included
    // - Top N numeric columns are selected via semantic search
    let (selected_columns, numeric_count, non_numeric_count) = select_columns_hybrid(
        &cached.columns,
        semantic_column_names,
        MAX_NUMERIC_COLUMNS_PER_TABLE,
    );
    
    // Build columns section with enhanced metadata
    schema_text.push_str("Columns:\n\n");
    for col in &selected_columns {
        // Build type with special attributes
        let mut type_parts = vec![col.data_type.clone()];
        for attr in &col.special_attributes {
            match attr.as_str() {
                "primary_key" => type_parts.push("PK".to_string()),
                "partition" => type_parts.push("PART".to_string()),
                "cluster" => type_parts.push("CLUST".to_string()),
                "foreign_key" => type_parts.push("FK".to_string()),
                _ => {}
            }
        }
        
        schema_text.push_str(&format!("{} ({})", col.name, type_parts.join(" ")));
        
        // Add top values inline for enum-like columns (compact format)
        if !col.top_values.is_empty() {
            let vals: String = col.top_values.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            schema_text.push_str(&format!(" [{}]", vals));
        }
        
        if let Some(ref d) = col.description {
            schema_text.push_str(&format!(": {}", d));
        }
        schema_text.push('\n');
    }
    
    // Add truncation indicator for numeric columns if applicable
    let total_numeric = cached.columns.iter()
        .filter(|c| tools::schema_search::is_numeric_data_type(&c.data_type))
        .count();
    let omitted_numeric = total_numeric.saturating_sub(numeric_count);
    
    if omitted_numeric > 0 {
        schema_text.push_str(&format!(
            "... and {} more numeric columns (use schema_search tool for full list)\n",
            omitted_numeric
        ));
    }
    
    println!(
        "[build_filtered_schema_text] Table '{}': {} non-numeric + {} numeric columns (of {} total numeric)",
        cached.fully_qualified_name,
        non_numeric_count,
        numeric_count,
        total_numeric
    );
    
    schema_text
}

/// Keep the shared registry's database built-ins in sync with current settings.
async fn sync_registry_database_tools(
    registry: &SharedToolRegistry,
    always_on_builtin_tools: &[String],
) {
    let mut guard = registry.write().await;
    guard.set_schema_search_enabled(always_on_builtin_tools.contains(&"schema_search".to_string()));
    guard.set_sql_select_enabled(always_on_builtin_tools.contains(&"sql_select".to_string()));
}

/// Ensure sql_select is enabled (registry + persisted settings) after schema search.
async fn auto_enable_sql_select(
    registry: &SharedToolRegistry,
    settings_state: &State<'_, SettingsState>,
    settings_sm_state: &State<'_, SettingsStateMachineState>,
    launch_config: &State<'_, LaunchConfigState>,
    reason: &str,
) {
    {
        let mut guard = registry.write().await;
        guard.set_sql_select_enabled(true);
    }

    let mut settings_guard = settings_state.settings.write().await;
    if !settings_guard.always_on_builtin_tools.contains(&"sql_select".to_string()) {
        settings_guard.always_on_builtin_tools.push("sql_select".to_string());
        
        // Refresh the SettingsStateMachine (Tier 1)
        let mut sm_guard = settings_sm_state.machine.write().await;
        sm_guard.refresh(&settings_guard, &launch_config.tool_filter);

        if let Err(e) = settings::save_settings(&settings_guard).await {
            println!(
                "[Chat] Failed to persist auto-enabled sql_select ({}): {}",
                reason, e
            );
        } else {
            println!(
                "[Chat] sql_select auto-enabled after {}",
                reason
            );
        }
    }
}

/// Build Python context JSON from parsed tabular files.
/// 
/// Creates variables for each file:
/// - `headers1`, `headers2`, etc. - Tuples of column names
/// - `rows1`, `rows2`, etc. - Lists of typed tuples
/// 
/// Values are pre-converted to int/float/datetime/None to reduce model errors.
fn build_tabular_python_context(files: &[tabular_parser::TabularFileData]) -> Option<serde_json::Value> {
    if files.is_empty() {
        return None;
    }

    let mut context = serde_json::Map::new();

    for (index, file) in files.iter().enumerate() {
        let var_index = index + 1; // 1-indexed

        // Add headers as tuple
        let headers_key = format!("headers{}", var_index);
        let headers_value: Vec<serde_json::Value> = file
            .headers
            .iter()
            .map(|h| serde_json::Value::String(h.clone()))
            .collect();
        context.insert(headers_key, serde_json::Value::Array(headers_value));

        // Add rows as list of tuples (typed values)
        let rows_key = format!("rows{}", var_index);
        let rows_value: Vec<serde_json::Value> = file
            .rows
            .iter()
            .map(|row| {
                serde_json::Value::Array(
                    row.iter()
                        .map(|cell| typed_value_to_json(cell))
                        .collect(),
                )
            })
            .collect();
        context.insert(rows_key, serde_json::Value::Array(rows_value));
    }

    Some(serde_json::Value::Object(context))
}

/// Convert a TypedValue to a JSON value for Python context injection.
fn typed_value_to_json(value: &tabular_parser::TypedValue) -> serde_json::Value {
    match value {
        tabular_parser::TypedValue::Null => serde_json::Value::Null,
        tabular_parser::TypedValue::Bool(b) => serde_json::Value::Bool(*b),
        tabular_parser::TypedValue::Int(i) => serde_json::json!(i),
        tabular_parser::TypedValue::Float(f) => serde_json::json!(f),
        tabular_parser::TypedValue::DateTime(s) => {
            // Wrap datetime as a special object for Python parsing
            serde_json::json!({
                "__datetime__": s
            })
        }
        tabular_parser::TypedValue::String(s) => serde_json::Value::String(s.clone()),
    }
}

// NOTE: The following functions have been moved to agentic_loop.rs:
// - extract_python_program_from_response()
// - is_valid_python_syntax_check()
// - AgenticLoopAction enum
// - detect_agentic_loop_action()
// - run_agentic_loop()
// See agentic_loop.rs for the implementation.

fn tool_schema_to_mcp_tool(schema: &ToolSchema) -> McpTool {
    McpTool {
        name: schema.name.clone(),
        description: schema.description.clone(),
        input_schema: Some(schema.parameters.clone()),
        input_examples: if schema.input_examples.is_empty() {
            None
        } else {
            Some(schema.input_examples.clone())
        },
        allowed_callers: schema.allowed_callers.clone(),
    }
}

#[tauri::command]
async fn chat(
    chat_id: Option<String>,
    title: Option<String>,
    message: String,
    history: Vec<ChatMessage>,
    reasoning_effort: String,
    model: String, // Frontend is source of truth for model selection
    attached_files: Vec<String>,
    attached_tables: Vec<crate::settings_state_machine::AttachedTableInfo>,
    attached_tools: Vec<String>,
    attached_tabular_files: Vec<String>, // Paths to CSV/TSV/XLS/XLSX files for Python analysis
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    approval_state: State<'_, ToolApprovalState>,
    tool_registry_state: State<'_, ToolRegistryState>,
    embedding_state: State<'_, EmbeddingModelState>,
    launch_config: State<'_, LaunchConfigState>,
    cancellation_state: State<'_, CancellationState>,
    turn_tracker: State<'_, TurnTrackerState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    use std::io::Write;
    let chat_id = chat_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let chat_id_return = chat_id.clone();
    let title = title.unwrap_or_else(|| message.chars().take(50).collect::<String>());

    // Log incoming chat request
    let msg_preview: String = message.chars().take(128).collect();
    let msg_suffix = if message.len() > 128 { "..." } else { "" };

    // Set up cancellation signal for this generation
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let generation_id = {
        // Increment generation ID and store the cancel signal
        let mut gen_id = cancellation_state.current_generation_id.write().await;
        *gen_id = gen_id.wrapping_add(1);
        let current_generation = *gen_id;
        *cancellation_state.cancel_signal.write().await = Some(cancel_tx);
        current_generation
    };

    println!("\n[chat] =============================================================");
    println!(
        "[chat] 💬 New chat | id={} | gen={} | history_len={} | user_chars={} | preview=\"{}{}\"",
        chat_id,
        generation_id,
        history.len(),
        message.len(),
        msg_preview,
        msg_suffix
    );
    println!(
        "[chat] Cancellation channel ready for generation {}",
        generation_id
    );
    let _ = std::io::stdout().flush();

    let _verbose_logging = is_verbose_logging_enabled();

    let tool_filter = launch_config.tool_filter.clone();

    // Get server configs from settings
    let settings = settings_state.settings.read().await;
    let configured_system_prompt = settings.system_prompt.clone();
    let mut server_configs = settings.get_all_mcp_configs();
    let tool_search_max_results = settings.tool_search_max_results.max(1);
    let tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let tool_use_examples_max = settings.tool_use_examples_max;
    let database_toolbox_config = settings.database_toolbox.clone();
    
    // Always-on configuration
    let always_on_builtin_tools = settings.always_on_builtin_tools.clone();
    let always_on_mcp_tools = settings.always_on_mcp_tools.clone();
    let always_on_tables = settings.always_on_tables.clone();
    let always_on_rag_paths = settings.always_on_rag_paths.clone();

    // Log always-on configuration if any is set
    if !always_on_builtin_tools.is_empty() || !always_on_mcp_tools.is_empty() || !always_on_tables.is_empty() || !always_on_rag_paths.is_empty() {
        println!(
            "[Chat] Always-on: builtins={:?}, mcp_tools={:?}, tables={}, rag_paths={}",
            always_on_builtin_tools.len(),
            always_on_mcp_tools.len(),
            always_on_tables.len(),
            always_on_rag_paths.len()
        );
    }

    let chat_format_default = settings.chat_format_default;
    let chat_format_overrides = settings.chat_format_overrides.clone();
    let tool_system_prompts = settings.tool_system_prompts.clone();
    let python_tool_calling_enabled = settings.python_tool_calling_enabled;
    let internal_schema_search = settings.should_run_internal_schema_search();
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    
    // Derived flags for legacy compatibility within this function
    // A tool is active if it's Always On OR explicitly attached for this chat
    let is_builtin_active = |name: &str| {
        always_on_builtin_tools.contains(&name.to_string()) 
            || attached_tools.contains(&format!("builtin::{}", name)) 
            || attached_tools.contains(&name.to_string())
    };
    let python_execution_enabled = is_builtin_active("python_execution");
    let _tool_search_enabled = is_builtin_active("tool_search");
    let schema_search_enabled = is_builtin_active("schema_search");
    let sql_select_enabled = is_builtin_active("sql_select");
    
    drop(settings);

    // Build ChatTurnContext with attachments
    // Generate embedding for user prompt (for semantic column search)
    let user_prompt_embedding: Option<Vec<f32>> = if !message.trim().is_empty() && !attached_tables.is_empty() {
        // Use CPU model for semantic column search during chat (avoids evicting LLM from GPU)
        let model_guard = embedding_state.cpu_model.read().await;
        if let Some(model) = model_guard.as_ref() {
            let model_clone = Arc::clone(model);
            let query = message.clone();
            drop(model_guard);
            match tokio::task::spawn_blocking(move || model_clone.embed(vec![query], None)).await {
                Ok(Ok(embeddings)) => embeddings.into_iter().next(),
                Ok(Err(e)) => {
                    println!("[Chat] Warning: Failed to embed user prompt for column search: {}", e);
                    None
                }
                Err(e) => {
                    println!("[Chat] Warning: Embedding task failed: {}", e);
                    None
                }
            }
        } else {
            drop(model_guard);
            None
        }
    } else {
        None
    };

    let mut turn_attached_tables = Vec::new();
    for table in attached_tables {
        // Fetch full table schema from cache to build prompt context
        let (tx, rx) = oneshot::channel();
        if let Err(e) = handles.schema_tx.send(SchemaVectorMsg::GetTablesForSource {
            source_id: table.source_id.clone(),
            respond_to: tx,
        }).await {
            println!("[Chat] Warning: Failed to send GetTablesForSource: {}", e);
            turn_attached_tables.push(table);
            continue;
        }

        match rx.await {
            Ok(cached_tables) => {
                if let Some(cached) = cached_tables.into_iter().find(|t| t.fully_qualified_name == table.table_fq_name) {
                    // Use semantic column search if we have an embedding, otherwise use all columns
                    let semantic_columns: Option<HashSet<String>> = if let Some(ref embedding) = user_prompt_embedding {
                        let (col_tx, col_rx) = oneshot::channel();
                        if handles.schema_tx.send(SchemaVectorMsg::SearchColumns {
                            query_embedding: embedding.clone(),
                            table_fq_name: Some(cached.fully_qualified_name.clone()),
                            limit: SEMANTIC_COLUMN_SEARCH_LIMIT,
                            respond_to: col_tx,
                        }).await.is_ok() {
                            match col_rx.await {
                                Ok(results) => {
                                    let names: HashSet<String> = results.iter().map(|c| c.column_name.clone()).collect();
                                    if !names.is_empty() {
                                        println!("[Chat] Semantic column search for {}: {} relevant columns", 
                                            cached.fully_qualified_name, names.len());
                                        Some(names)
                                    } else {
                                        None
                                    }
                                }
                                Err(_) => None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    
                    // Use filtered schema to avoid overwhelming local models with massive column lists
                    let schema_text = build_filtered_schema_text(&cached, semantic_columns.as_ref());
                    
                    turn_attached_tables.push(crate::settings_state_machine::AttachedTableInfo {
                        source_id: table.source_id,
                        table_fq_name: table.table_fq_name,
                        column_count: table.column_count,
                        schema_text: Some(schema_text),
                    });
                } else {
                    turn_attached_tables.push(table);
                }
            }
            Err(_) => {
                turn_attached_tables.push(table);
            }
        }
    }

    // Parse tabular files for Python injection
    let mut parsed_tabular_files: Vec<tabular_parser::TabularFileData> = Vec::new();
    let mut turn_attached_tabular_files: Vec<crate::settings_state_machine::AttachedTabularFile> = Vec::new();
    
    for (index, file_path) in attached_tabular_files.iter().enumerate() {
        let path = std::path::Path::new(file_path);
        match tabular_parser::parse_tabular_file(path) {
            Ok(data) => {
                let variable_index = index + 1; // 1-indexed for user clarity
                turn_attached_tabular_files.push(crate::settings_state_machine::AttachedTabularFile {
                    file_path: data.file_path.clone(),
                    file_name: data.file_name.clone(),
                    headers: data.headers.clone(),
                    row_count: data.row_count,
                    variable_index,
                });
                parsed_tabular_files.push(data);
                println!("[Chat] Parsed tabular file {}: {} ({} rows, {} columns)", 
                    variable_index, file_path, 
                    turn_attached_tabular_files.last().unwrap().row_count,
                    turn_attached_tabular_files.last().unwrap().headers.len());
            }
            Err(e) => {
                println!("[Chat] Warning: Failed to parse tabular file '{}': {}", file_path, e);
            }
        }
    }

    let turn_context = ChatTurnContext {
        attached_files: attached_files.clone(),
        attached_tables: turn_attached_tables.clone(),
        attached_tools: attached_tools.clone(),
        attached_tabular_files: turn_attached_tabular_files.clone(),
    };

    // Let state machine compute turn-specific configuration
    let turn_config = {
        let sm_guard = settings_sm_state.machine.read().await;
        let settings_guard = settings_state.settings.read().await;
        sm_guard.compute_for_turn(&settings_guard, &tool_filter, &turn_context)
    };

    // Compute enabled database sources:
    // 1. Sources marked as enabled in settings (when db tools are always-on)
    // 2. PLUS sources whose tables are attached to this chat turn
    let mut enabled_db_sources: Vec<String> = server_configs
        .iter()
        .filter(|s| s.is_database_source && s.enabled)
        .map(|s| s.id.clone())
        .collect();
    
    // Add sources for attached tables (enables sql_select for per-chat attachments)
    // Use turn_attached_tables which was built from the original attached_tables
    for table in &turn_attached_tables {
        if !enabled_db_sources.contains(&table.source_id) {
            enabled_db_sources.push(table.source_id.clone());
        }
    }
    
    // Also enable these sources in server_configs so MCP sync doesn't disconnect them
    for source_id in &enabled_db_sources {
        if let Some(config) = server_configs.iter_mut().find(|s| &s.id == source_id && s.is_database_source) {
            if !config.enabled {
                println!("[Chat] Enabling database source '{}' for attached tables", source_id);
                config.enabled = true;
            }
        }
    }

    println!("[Chat] Turn Configuration: Mode={}, Enabled Tools={:?}, DB Sources={:?}", 
        turn_config.mode.name(), turn_config.enabled_tools, enabled_db_sources);

    // Initialize turn tracker for this generation
    {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut progress = turn_tracker.progress.write().await;
        *progress = TurnProgress {
            active: true,
            chat_id: Some(chat_id.clone()),
            generation_id,
            assistant_response: String::new(),
            last_token_index: 0,
            finished: false,
            had_tool_calls: false,
            timestamp_ms: now_ms,
        };
    }

    // Ensure database toolbox actor is initialized if database tools are enabled
    let db_tools_available = turn_config.mode.has_sql();
    if db_tools_available {
        if let Err(e) =
            ensure_toolbox_running(&handles.database_toolbox_tx, &database_toolbox_config).await
        {
            println!(
                "[Chat] Warning: Failed to ensure database toolbox is running: {}",
                e
            );
        }
    }

    // Ensure all enabled MCP servers (regular + database) are connected before proceeding with discovery
    let (sync_tx, sync_rx) = oneshot::channel();
    if let Err(e) = handles.mcp_host_tx.send(McpHostMsg::SyncEnabledServers {
        configs: server_configs.clone(),
        respond_to: sync_tx,
    }).await {
        println!("[Chat] Warning: Failed to send sync request to MCP Host: {}", e);
    } else {
        let _ = sync_rx.await;
    }

    // Look up model info for the frontend-provided model to check capabilities
    // Frontend is the source of truth for model selection
    let (current_model_info, _model_supports_native_tools) = {
        let (tx, rx) = oneshot::channel();
        if handles
            .foundry_tx
            .send(FoundryMsg::GetCurrentModel { respond_to: tx })
            .await
            .is_ok()
        {
            (rx.await.ok().flatten(), true) // Placeholder for now, real check below
        } else {
            (None, false)
        }
    };

    // Resolve model profile to get tool call format preference
    let profile = model_profiles::resolve_profile(&model);
    let resolved_model_tool_format = Some(profile.tool_call_format);

    // Native tool calling is only available if: format is enabled AND model supports it
    let model_supports_native_tools = current_model_info.as_ref().map(|m| m.tool_calling).unwrap_or(false);
    let native_tool_calling_enabled =
        format_config.native_enabled() && model_supports_native_tools;

    // Log model capabilities for debugging
    let model_id = current_model_info
        .as_ref()
        .map(|m| m.id.as_str())
        .unwrap_or("unknown");
    println!(
        "[chat] Model capabilities: model={}, native_enabled_in_config={}, model_supports_native={}, using_native={}",
        model_id,
        format_config.native_enabled(),
        model_supports_native_tools,
        native_tool_calling_enabled
    );

    // Ensure registry reflects Always On built-ins before building prompts
    sync_registry_database_tools(
        &tool_registry_state.registry,
        &always_on_builtin_tools,
    )
    .await;

    // Apply global tool_search flag to server defer settings (only if tool_search is actually available)
    let tool_search_allowed = tool_filter.builtin_allowed("tool_search");
    let tool_search_enabled = always_on_builtin_tools.contains(&"tool_search".to_string());
    if tool_search_enabled && tool_search_allowed {
        for config in &mut server_configs {
            config.defer_tools = true;
        }
    } else if !tool_search_enabled {
        // If tool search is explicitly disabled globally, we MUST surface regular tools or they'll be unreachable.
        // HOWEVER, database sources should stay deferred because they are only meant for sql_select context injection.
        for config in &mut server_configs {
            if !config.is_database_source {
                config.defer_tools = false;
            }
        }
    }
    // Otherwise, we respect the per-server config. This prevents bloating the prompt with 
    // database tools that are handled via sql_select.

    // Get tool descriptions from MCP Host Actor
    let (tools_tx, tools_rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::GetAllToolDescriptions {
            respond_to: tools_tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    let tool_descriptions = tools_rx
        .await
        .map_err(|_| "MCP Host actor died".to_string())?;

    // Apply launch-time filters and check enabled status
    let filtered_tool_descriptions: Vec<(String, Vec<McpTool>)> = tool_descriptions
        .into_iter()
        .filter_map(|(server_id, tools)| {
            // Check if server is enabled in settings and NOT a database source
            // (Database tools are handled separately via sql_select/schema_search)
            let is_enabled = server_configs
                .iter()
                .any(|c| c.id == server_id && c.enabled && !c.is_database_source);

            if !is_enabled {
                return None;
            }

            if !tool_filter.server_allowed(&server_id) {
                return None;
            }

            let filtered_tools: Vec<McpTool> = tools
                .into_iter()
                .filter(|t| tool_filter.tool_allowed(&server_id, &t.name))
                .collect();

            if filtered_tools.is_empty() {
                None
            } else {
                Some((server_id, filtered_tools))
            }
        })
        .collect();

    // Check if there are any MCP tools available
    let has_mcp_tools = filtered_tool_descriptions
        .iter()
        .any(|(_, tools)| !tools.is_empty());

    // Check if there are any deferred MCP tools (for tool_search discovery)
    let has_deferred_mcp_tools = filtered_tool_descriptions
        .iter()
        .any(|(server_id, tools)| {
            !tools.is_empty()
                && server_configs
                    .iter()
                    .find(|c| c.id == *server_id)
                    .map(|c| c.defer_tools)
                    .unwrap_or(true)
        });

    // Build the tools list:
    // 1. Include python_execution if enabled in settings
    // 2. Include tool_search when MCP servers with deferred tools are available
    // 3. Include all MCP tools
    let code_mode_possible = format_config.is_enabled(ToolCallFormatName::CodeMode)
        && python_execution_enabled
        && python_tool_calling_enabled
        && tool_filter.builtin_allowed("python_execution");
    // Primary affects prompting only; execution should honor any enabled format.
    // Native is available only if both enabled in config AND model supports it.
    let native_available = native_tool_calling_enabled;
    let primary_format_for_prompt =
        format_config.resolve_primary_for_prompt(code_mode_possible, native_available);
    // python_tool_mode controls DETECTION of Python code in responses.
    // This should be enabled when python_execution is available, regardless of prompt format.
    // Even if we prompt the model with Native/Hermes format, it may still output Python code blocks.
    // When it does, we should detect and execute them if python_execution is enabled.
    let python_tool_mode = python_execution_enabled
        && python_tool_calling_enabled
        && tool_filter.builtin_allowed("python_execution");
    // tool_search is only offered when explicitly enabled in settings AND there are deferred tools
    let allow_tool_search_for_python =
        python_tool_mode && tool_search_enabled && has_deferred_mcp_tools && tool_filter.builtin_allowed("tool_search");
    let non_code_formats_enabled = format_config.any_non_code();
    let legacy_tool_calls_enabled =
        non_code_formats_enabled && primary_format_for_prompt != ToolCallFormatName::CodeMode;
    let legacy_tool_search_enabled =
        legacy_tool_calls_enabled && tool_search_enabled && has_deferred_mcp_tools && tool_filter.builtin_allowed("tool_search");

    println!(
        "[chat] tool_call_formats: config_primary={:?}, resolved_primary={:?}, enabled={:?}, native_available={}, python_execution_enabled={}, python_tool_calling_enabled={}, python_tool_mode={}, code_mode_possible={}",
        format_config.primary,
        primary_format_for_prompt,
        format_config.enabled,
        native_available,
        python_execution_enabled,
        python_tool_calling_enabled,
        python_tool_mode,
        code_mode_possible
    );

    let mut openai_tools: Option<Vec<OpenAITool>> = if legacy_tool_calls_enabled {
        Some(Vec::new())
    } else {
        None
    };

    if let Some(list) = openai_tools.as_mut() {
        if legacy_tool_search_enabled {
            let tool_search_tool = tool_registry::tool_search_tool();
            list.push(OpenAITool::from_tool_schema(&tool_search_tool));
            println!("[Chat] Added tool_search built-in tool (legacy mode)");
        }
    }

    // Add MCP tools to the OpenAI tools list and register them in the tool registry
    // so they're available for python_execution and tool_search
    {
        let mut registry = tool_registry_state.registry.write().await;

        // Clear any previously registered tools (fresh start for this chat)
        registry.clear_domain_tools();

        for (server_id, tools) in &filtered_tool_descriptions {
            // Get the server config to extract defer_tools and python_name
            let config = server_configs.iter().find(|c| c.id == *server_id);
            let defer = config.map(|c| c.defer_tools).unwrap_or(false);
            let python_name = config
                .map(|c| c.get_python_name())
                .unwrap_or_else(|| settings::to_python_identifier(server_id));

            let mode = if defer { "DEFERRED" } else { "ACTIVE" };
            println!(
                "[Chat] Registering {} tools from {} [{}] (python_module={})",
                tools.len(),
                server_id,
                mode,
                python_name
            );

            // Register MCP tools in the registry with python module name
            registry.register_mcp_tools(server_id, &python_name, tools, defer);
        }

        let stats = registry.stats();
        println!(
            "[Chat] Tool registry: {} internal, {} domain, {} deferred, {} materialized",
            stats.internal_tools,
            stats.domain_tools,
            stats.deferred_tools,
            stats.materialized_tools
        );
    }

    // Pre-compute embeddings for all domain tools so tool_search can find them
    if !filtered_tool_descriptions.is_empty() {
        // Use CPU model for tool search embeddings during chat
        match precompute_tool_search_embeddings(
            tool_registry_state.registry.clone(),
            embedding_state.cpu_model.clone(),
        )
        .await
        {
            Ok(count) => println!("[Chat] Pre-computed embeddings for {} tools", count),
            Err(e) => println!(
                "[Chat] Warning: Failed to pre-compute tool embeddings: {}",
                e
            ),
        }
    }

    // Compute effective tables (explicit attachments + always-on tables)
    // Schema search only runs when we have effective tables to work with
    let has_effective_tables = !turn_attached_tables.is_empty() || !always_on_tables.is_empty();
    let should_run_schema_search = has_effective_tables 
        && (schema_search_enabled || internal_schema_search || sql_select_enabled);
    
    // Compute effective tools (explicit attachments + always-on tools)
    let has_effective_tools = !attached_tools.is_empty() 
        || !always_on_builtin_tools.is_empty() 
        || !always_on_mcp_tools.is_empty();
    let should_run_tool_search = tool_search_enabled 
        && tool_search_allowed 
        && (has_effective_tools || has_mcp_tools);
    
    println!(
        "[Chat] Auto-discovery gating: schema_search={} (effective_tables={}), tool_search={} (effective_tools={})",
        should_run_schema_search, has_effective_tables,
        should_run_tool_search, has_effective_tools
    );

    // Run auto-discovery (tool search + schema search) for this user prompt
    let auto_discovery = perform_auto_discovery_for_prompt(
        &message,
        should_run_tool_search, // Only run auto tool discovery if we have effective tools
        tool_search_max_results,
        has_mcp_tools,
        should_run_schema_search, // Only run auto schema search if we have effective tables
        settings_state.settings.read().await.schema_relevancy_threshold,
        &database_toolbox_config,
        &filtered_tool_descriptions,
        tool_registry_state.registry.clone(),
        embedding_state.cpu_model.clone(), // CPU model for search during chat
        handles.schema_tx.clone(),
        true,
    )
    .await;

    // Check if there are any attached documents (RAG indexed files)
    let has_attachments = {
        let (tx, rx) = oneshot::channel();
        if handles
            .rag_tx
            .send(RagMsg::GetIndexedFiles { respond_to: tx })
            .await
            .is_ok()
        {
            rx.await.map(|files| !files.is_empty()).unwrap_or(false)
        } else {
            false
        }
    };

    // If we already performed schema search for this prompt and found tables, surface sql_select immediately
    if let Some(ref output) = auto_discovery.schema_search_output {
        if !output.tables.is_empty() {
            auto_enable_sql_select(
                &tool_registry_state.registry,
                &settings_state,
                &settings_sm_state,
                &launch_config,
                "auto schema_search",
            )
            .await;
        }
    }

    // Resolve tool capabilities using centralized resolver
    // NOTE: Must be after auto_enable_sql_select so database tools are included
    let (resolved_capabilities, _model_tool_format) = {
        // Refresh settings to pick up any auto-enabled tools (like sql_select after schema search)
        let fresh_settings = settings_state.settings.read().await;
        let settings_for_resolver = fresh_settings.clone();
        drop(fresh_settings);
        
        let registry = tool_registry_state.registry.read().await;
        let default_model_info = ModelInfo {
            id: "unknown".to_string(),
            family: ModelFamily::Generic,
            tool_calling: false,
            tool_format: ToolFormat::TextBased,
            vision: false,
            reasoning: false,
            reasoning_format: protocol::ReasoningFormat::None,
            max_input_tokens: 4096,
            max_output_tokens: 2048,
            supports_tool_calling: false,
            supports_temperature: true,
            supports_top_p: true,
            supports_reasoning_effort: false,
        };
        let model_info = current_model_info.as_ref().unwrap_or(&default_model_info);
        let caps = ToolCapabilityResolver::resolve(
            &settings_for_resolver,
            model_info,
            &tool_filter,
            &server_configs,
            &registry,
        );
        (caps, Some(model_info.tool_format))
    };

    println!(
        "[Chat] Resolved capabilities: builtins={:?}, primary_format={:?}, use_native={}, active_mcp={}, deferred_mcp={}",
        resolved_capabilities.available_builtins,
        resolved_capabilities.primary_format,
        resolved_capabilities.use_native_tools,
        resolved_capabilities.active_mcp_tools.len(),
        resolved_capabilities.deferred_mcp_tools.len()
    );

    // Visible tools: always include enabled built-ins; defer MCP tools to tool_search unless materialized.
    let builtin_tools: Vec<(String, Vec<McpTool>)> = {
        let registry = tool_registry_state.registry.read().await;
        registry
            .get_internal_tools()
            .iter()
            .filter(|schema| {
                // If per-chat tools are attached, only allow those
                if !turn_config.enabled_tools.is_empty() {
                    return turn_config.enabled_tools.contains(&schema.name);
                }

                // Fallback: include tool only if it's in always_on_builtin_tools AND passes filter
                // Built-in tools require BOTH their *_enabled flag AND presence in always_on_builtin_tools
                let is_always_on = always_on_builtin_tools.contains(&schema.name);
                
                if schema.name == "python_execution" {
                    is_always_on && python_execution_enabled && tool_filter.builtin_allowed("python_execution")
                } else if schema.name == "tool_search" {
                    // tool_search only if always-on AND there are deferred tools to discover
                    is_always_on && has_deferred_mcp_tools && tool_filter.builtin_allowed("tool_search")
                } else if schema.name == "sql_select" {
                    is_always_on && sql_select_enabled && tool_filter.builtin_allowed("sql_select")
                } else if schema.name == "schema_search" {
                    is_always_on && schema_search_enabled && tool_filter.builtin_allowed("schema_search")
                } else {
                    // Unknown built-ins: require always_on and filter
                    is_always_on && tool_filter.builtin_allowed(&schema.name)
                }
            })
            .map(|schema| ("builtin".to_string(), vec![tool_schema_to_mcp_tool(schema)]))
            .collect()
    };

    let visible_tool_descriptions: Vec<(String, Vec<McpTool>)> = if tool_search_enabled && turn_config.enabled_tools.is_empty() {
        let mut list = builtin_tools;
        if !auto_discovery.discovered_tool_schemas.is_empty() {
            list.extend(auto_discovery.discovered_tool_schemas.clone());
        }
        list
    } else if !turn_config.enabled_tools.is_empty() {
        // If per-chat tools attached, filter MCP tools to only those explicitly attached
        let mut list = builtin_tools;
        for (server_id, tools) in &filtered_tool_descriptions {
            let server_attached_tools: Vec<McpTool> = tools.iter().filter(|t| {
                let key = format!("{}::{}", server_id, t.name);
                turn_config.enabled_tools.contains(&key)
            }).cloned().collect();
            if !server_attached_tools.is_empty() {
                list.push((server_id.clone(), server_attached_tools));
            }
        }
        list
    } else {
        let mut list = builtin_tools;
        list.extend(filtered_tool_descriptions.clone());
        list
    };

    // Include visible tools in legacy/native tool calling payloads
    if let Some(ref mut tools_list) = openai_tools {
        let registry = tool_registry_state.registry.read().await;
        let mut seen: HashSet<String> =
            tools_list.iter().map(|t| t.function.name.clone()).collect();

        for (server_id, schema) in registry.get_visible_tools_with_servers() {
            // Filter builtin tools based on their enabled state and tool filter
            if server_id == "builtin" {
                // If per-chat tools attached, only allow those
                if !turn_config.enabled_tools.is_empty() {
                    if !turn_config.enabled_tools.contains(&schema.name) {
                        continue;
                    }
                } else {
                    // Fallback: require tool to be in always_on_builtin_tools
                    let is_always_on = always_on_builtin_tools.contains(&schema.name);
                    
                    if schema.name == "python_execution" {
                        if !is_always_on || !python_execution_enabled || !tool_filter.builtin_allowed("python_execution") {
                            continue;
                        }
                    } else if schema.name == "tool_search" {
                        // tool_search only if always-on AND there are deferred tools to discover
                        if !is_always_on || !has_deferred_mcp_tools || !tool_filter.builtin_allowed("tool_search") {
                            continue;
                        }
                    } else if schema.name == "sql_select" {
                        if !is_always_on || !sql_select_enabled || !tool_filter.builtin_allowed("sql_select") {
                            continue;
                        }
                    } else if schema.name == "schema_search" {
                        if !is_always_on || !schema_search_enabled || !tool_filter.builtin_allowed("schema_search") {
                            continue;
                        }
                    } else if !is_always_on || !tool_filter.builtin_allowed(&schema.name) {
                        // Unknown built-ins: require always_on and filter
                        continue;
                    }
                }
            } else {
                // MCP tools - check if explicitly attached if any tools are attached
                if !turn_config.enabled_tools.is_empty() {
                    let key = format!("{}::{}", server_id, schema.name);
                    if !turn_config.enabled_tools.contains(&key) {
                        continue;
                    }
                }
            }
            // Build OpenAI tool; MCP tools get server prefix for routing (sanitized)
            let openai_tool = if server_id == "builtin" {
                OpenAITool::from_tool_schema(&schema)
            } else {
                OpenAITool::from_mcp_schema(&server_id, &schema)
            };

            if !seen.insert(openai_tool.function.name.clone()) {
                continue;
            }
            tools_list.push(openai_tool);
        }
    }

    if let Some(ref tools) = openai_tools {
        println!(
            "[Chat] Total tools available (legacy/native mode): {}",
            tools.len()
        );
    } else {
        println!(
            "[Chat] Tool calling via python_execution only: {} MCP servers registered",
            filtered_tool_descriptions.len()
        );
    }


    // Note: compact_prompt_enabled removed - now using capabilities.max_mcp_tools_in_prompt
    if tool_use_examples_enabled {
        println!(
            "[Chat] Tool examples enabled (max_per_tool={})",
            tool_use_examples_max
        );
    }
    println!(
        "[Chat] tool_search_max_results={}",
        tool_search_max_results
    );

    // === STATE MACHINE: Build system prompt from single source of truth ===
    
    // Get relevancy thresholds (now provided by SettingsStateMachine, keeping for potential future use)
    let _relevancy_thresholds = {
        let settings_guard = settings_state.settings.read().await;
        settings_guard.get_relevancy_thresholds()
    };
    
    // Determine which tools are active vs deferred
    // If tool_search is enabled, all MCP tools are deferred (discovered via tool_search)
    // Otherwise, they are active (shown immediately)
    let (active_tools, deferred_tools): (Vec<(String, Vec<McpTool>)>, Vec<(String, Vec<McpTool>)>) = 
        if tool_search_enabled {
            (Vec::new(), filtered_tool_descriptions.clone())
        } else {
            (filtered_tool_descriptions.clone(), Vec::new())
        };
    
    // Build MCP tool context for state machine
    let mcp_context = agentic_state::McpToolContext::from_tool_lists(
        &active_tools,
        &deferred_tools,
        &server_configs,
    );
    
    // Extract column info from parsed tabular files for prompt generation
    let tabular_column_info: Vec<Vec<crate::tabular_parser::ColumnInfo>> = parsed_tabular_files
        .iter()
        .map(|f| f.columns.clone())
        .collect();

    // Build prompt context - use the raw system prompt, let state machine add context
    let prompt_context = agentic_state::PromptContext {
        base_prompt: configured_system_prompt.clone(),
        has_attachments,
        attached_tables: turn_attached_tables.clone(),
        attached_tools: attached_tools.clone(),
        attached_tabular_files: turn_attached_tabular_files.clone(),
        tabular_column_info,
        mcp_context,
        tool_call_format: primary_format_for_prompt,
        model_tool_format: resolved_model_tool_format,
        custom_tool_prompts: tool_system_prompts.clone(),
        python_primary: python_tool_mode,
    };
    
    // Create state machine using three-tier hierarchy:
    // Tier 1 (SettingsStateMachine) provides capabilities and thresholds
    // Tier 2 (AgenticStateMachine) manages turn-level state
    let mut initial_state_machine = {
        let settings_sm_guard = settings_sm_state.machine.read().await;
        state_machine::AgenticStateMachine::new_from_settings_sm(
            &settings_sm_guard,
            prompt_context,
        )
    };

    // Compute turn-specific configuration (Tier 1 overrides)
    {
        let settings_guard = settings_state.settings.read().await;
        initial_state_machine.compute_turn_config(&settings_guard, &tool_filter);
    }

    // Update state machine with auto-discovery results
    let schema_relevancy = auto_discovery.schema_search_output.as_ref()
        .map(|o| o.tables.iter().map(|t| t.relevance).fold(0.0f32, f32::max))
        .unwrap_or(0.0);
    
    let discovered_tables = auto_discovery.schema_search_output.as_ref()
        .map(|o| o.tables.iter().map(|t| agentic_state::TableInfo {
            fully_qualified_name: t.table_name.clone(),
            source_id: t.source_id.clone(),
            sql_dialect: t.sql_dialect.clone(),
            relevancy: t.relevance,
            description: t.description.clone(),
            columns: t.relevant_columns.iter().map(|c| agentic_state::ColumnInfo {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: true, // Default to true if not known
                description: c.description.clone(),
                special_attributes: c.special_attributes.clone(),
                top_values: c.top_values.clone(),
            }).collect(),
        }).collect())
        .unwrap_or_default();

    // Initialize state based on discovery results
    initial_state_machine.compute_initial_state(
        0.0, // RAG relevancy (not yet searched at turn start)
        schema_relevancy,
        discovered_tables,
        Vec::new(), // RAG chunks
    );
    
    // Pass auto-discovery context to state machine (it owns prompt generation)
    initial_state_machine.set_auto_discovery_context(
        auto_discovery.tool_search_output.clone(),
        auto_discovery.schema_search_output.clone(),
    );
    
    // Build system prompt from state machine (single source of truth)
    let system_prompt = initial_state_machine.build_system_prompt();
    
    // Log operational mode from Tier 1
    let operational_mode_name = {
        let sm_guard = settings_sm_state.machine.read().await;
        sm_guard.operational_mode().name().to_string()
    };
    
    println!(
        "[Chat] System prompt from state machine: {} chars, state={}, mode={}",
        system_prompt.len(),
        initial_state_machine.current_state().name(),
        operational_mode_name
    );

    // === LOGGING: System prompt construction ===
    let auto_approve_servers: Vec<&str> = server_configs
        .iter()
        .filter(|c| c.auto_approve_tools)
        .map(|c| c.id.as_str())
        .collect();
    let tool_count: usize = visible_tool_descriptions
        .iter()
        .map(|(_, tools)| tools.len())
        .sum();
    let server_count = visible_tool_descriptions.len();

    println!(
        "[Chat] System prompt: {}chars, servers={}, tools={}, auto_approve={:?}",
        system_prompt.len(),
        server_count,
        tool_count,
        auto_approve_servers
    );
    // Note: Full system prompt logging is now handled in ModelGatewayActor with diff logic.

    // Emit the exact system prompt for UI visibility (matches what the model receives)
    let _ = app_handle.emit(
        "system-prompt",
        SystemPromptEvent {
            chat_id: chat_id.clone(),
            generation_id,
            prompt: system_prompt.clone(),
        },
    );

    // Build full history with system prompt at the beginning
    let mut full_history = Vec::new();

    // Add system prompt if we have one
    if !system_prompt.is_empty() {
        full_history.push(ChatMessage {
            role: "system".to_string(),
            content: system_prompt.clone(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
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
        system_prompt: None,
        tool_calls: None,
        tool_call_id: None,
    });

    // Use the frontend-provided model (frontend is source of truth)
    let model_name = model.clone();
    println!("[Chat] Using model: {} (frontend-provided)", model_name);

    // Build agentic loop handles (actor channels and shared state)
    let agentic_handles = AgenticLoopHandles {
        foundry_tx: handles.foundry_tx.clone(),
        mcp_host_tx: handles.mcp_host_tx.clone(),
        vector_tx: handles.vector_tx.clone(),
        python_tx: handles.python_tx.clone(),
        schema_tx: handles.schema_tx.clone(),
        database_toolbox_tx: handles.database_toolbox_tx.clone(),
        tool_registry: tool_registry_state.registry.clone(),
        // Use CPU model for embeddings during chat (avoids evicting LLM from GPU)
        embedding_model: embedding_state.cpu_model.clone(),
        pending_approvals: approval_state.pending.clone(),
    };

    // Check if python_execution is in the native tools list
    // This enables fallback detection of ```python blocks when model doesn't use native format
    let python_execution_in_native_tools = openai_tools
        .as_ref()
        .map(|tools| tools.iter().any(|t| t.function.name == "python_execution"))
        .unwrap_or(false);

    // Build agentic loop config (behavior parameters)
    let agentic_config = AgenticLoopConfig {
        chat_id: chat_id.clone(),
        generation_id,
        title: title.clone(),
        original_message: message.clone(),
        model_name,
        reasoning_effort,
        python_tool_mode,
        format_config: format_config.clone(),
        primary_format: primary_format_for_prompt,
        allow_tool_search_for_python,
        tool_search_max_results,
        turn_system_prompt: system_prompt.clone(),
        chat_format_default,
        chat_format_overrides: chat_format_overrides.clone(),
        enabled_db_sources,
        server_configs: server_configs.clone(), // Combined list!
        tabular_context: build_tabular_python_context(&parsed_tabular_files),
        python_execution_in_native_tools,
    };

    let turn_progress = turn_tracker.progress.clone();

    // Spawn the agentic loop task with state machine (single source of truth)
    tauri::async_runtime::spawn(async move {
        run_agentic_loop(
            agentic_handles,
            agentic_config,
            app_handle,
            full_history,
            cancel_rx,
            openai_tools,
            turn_progress,
            initial_state_machine,  // Pass state machine instead of thresholds
        )
        .await;
    });

    Ok(chat_id_return)
}

#[tauri::command]
async fn get_system_prompt_preview(
    user_prompt: String,
    attached_files: Vec<String>,
    attached_tables: Vec<crate::settings_state_machine::AttachedTableInfo>,
    attached_tools: Vec<String>,
    attached_tabular_files: Vec<String>, // Paths to CSV/TSV/XLS/XLSX files
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    launch_config: State<'_, LaunchConfigState>,
    tool_registry_state: State<'_, ToolRegistryState>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<String, String> {
    // 1. Get current settings and model info
    let settings = settings_state.settings.read().await;
    let base_prompt = settings.system_prompt.clone();
    let server_configs = settings.mcp_servers.clone();
    let tool_system_prompts = settings.tool_system_prompts.clone();
    let database_toolbox_config = settings.database_toolbox.clone();
    // Always-on configuration for gating auto-discovery
    let always_on_builtin_tools = settings.always_on_builtin_tools.clone();
    let always_on_mcp_tools = settings.always_on_mcp_tools.clone();
    let always_on_tables = settings.always_on_tables.clone();

    // Derived flags for legacy compatibility within this function
    let is_builtin_active = |name: &str| {
        always_on_builtin_tools.contains(&name.to_string()) 
            || attached_tools.contains(&format!("builtin::{}", name)) 
            || attached_tools.contains(&name.to_string())
    };
    let schema_search_enabled = is_builtin_active("schema_search");
    let sql_select_enabled = is_builtin_active("sql_select");
    let tool_search_enabled = is_builtin_active("tool_search");

    let settings_for_resolver = settings.clone();
    drop(settings);

    // 2. Build turn context and configuration
    // Generate embedding for user prompt (for semantic column search)
    // Use CPU model to avoid evicting LLM from GPU
    let user_prompt_embedding: Option<Vec<f32>> = if !user_prompt.trim().is_empty() && !attached_tables.is_empty() {
        let model_guard = embedding_state.cpu_model.read().await;
        if let Some(model) = model_guard.as_ref() {
            let model_clone = Arc::clone(model);
            let query = user_prompt.clone();
            drop(model_guard);
            match tokio::task::spawn_blocking(move || model_clone.embed(vec![query], None)).await {
                Ok(Ok(embeddings)) => embeddings.into_iter().next(),
                _ => None,
            }
        } else {
            drop(model_guard);
            None
        }
    } else {
        None
    };

    let mut turn_attached_tables = Vec::new();
    for table in attached_tables {
        // Fetch full table schema from cache to build prompt context
        let (tx, rx) = oneshot::channel();
        if let Err(_) = handles.schema_tx.send(SchemaVectorMsg::GetTablesForSource {
            source_id: table.source_id.clone(),
            respond_to: tx,
        }).await {
            turn_attached_tables.push(table);
            continue;
        }

        if let Ok(cached_tables) = rx.await {
            if let Some(cached) = cached_tables.into_iter().find(|t| t.fully_qualified_name == table.table_fq_name) {
                // Use semantic column search if we have an embedding
                let semantic_columns: Option<HashSet<String>> = if let Some(ref embedding) = user_prompt_embedding {
                    let (col_tx, col_rx) = oneshot::channel();
                    if handles.schema_tx.send(SchemaVectorMsg::SearchColumns {
                        query_embedding: embedding.clone(),
                        table_fq_name: Some(cached.fully_qualified_name.clone()),
                        limit: SEMANTIC_COLUMN_SEARCH_LIMIT,
                        respond_to: col_tx,
                    }).await.is_ok() {
                        match col_rx.await {
                            Ok(results) => {
                                let names: HashSet<String> = results.iter().map(|c| c.column_name.clone()).collect();
                                if !names.is_empty() { Some(names) } else { None }
                            }
                            Err(_) => None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                
                // Use filtered schema to avoid overwhelming local models with massive column lists
                let schema_text = build_filtered_schema_text(&cached, semantic_columns.as_ref());
                
                turn_attached_tables.push(crate::settings_state_machine::AttachedTableInfo {
                    source_id: table.source_id,
                    table_fq_name: table.table_fq_name,
                    column_count: table.column_count,
                    schema_text: Some(schema_text),
                });
            } else {
                turn_attached_tables.push(table);
            }
        } else {
            turn_attached_tables.push(table);
        }
    }

    // Build tabular file metadata for preview (lightweight)
    let turn_attached_tabular_files: Vec<crate::settings_state_machine::AttachedTabularFile> = attached_tabular_files
        .iter()
        .enumerate()
        .map(|(idx, path)| {
            let file_name = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            crate::settings_state_machine::AttachedTabularFile {
                file_path: path.clone(),
                file_name,
                headers: Vec::new(), // Not needed for preview
                row_count: 0,
                variable_index: idx + 1,
            }
        })
        .collect();

    let turn_context = ChatTurnContext {
        attached_files: attached_files.clone(),
        attached_tables: turn_attached_tables,
        attached_tools: attached_tools.clone(),
        attached_tabular_files: turn_attached_tabular_files,
    };

    let tool_filter = launch_config.tool_filter.clone();
    let settings_sm = SettingsStateMachine::from_settings(&settings_for_resolver, &tool_filter);
    let turn_config = settings_sm.compute_for_turn(&settings_for_resolver, &tool_filter, &turn_context);

    // 3. Resolve tools and discovery
    let (tools_tx, tools_rx) = oneshot::channel();
    if let Err(e) = handles.mcp_host_tx.send(McpHostMsg::GetAllToolDescriptions {
        respond_to: tools_tx,
    }).await {
        return Err(format!("Failed to get tool descriptions: {}", e));
    }
    let tool_descriptions = tools_rx.await.map_err(|_| "MCP Host actor died")?;

    let filtered_tool_descriptions: Vec<(String, Vec<McpTool>)> = tool_descriptions
        .into_iter()
        .filter_map(|(server_id, tools)| {
            let is_enabled = server_configs.iter().any(|c| c.id == server_id && c.enabled);
            if !is_enabled || !tool_filter.server_allowed(&server_id) {
                return None;
            }
            let infos: Vec<McpTool> = tools.into_iter().filter(|t| tool_filter.builtin_allowed(&t.name)).collect();
            if infos.is_empty() { None } else { Some((server_id, infos)) }
        })
        .collect();

    // Gate auto-discovery based on effective attachments (explicit + always-on)
    let has_effective_tables = !turn_context.attached_tables.is_empty() || !always_on_tables.is_empty();
    let internal_schema_search = settings_for_resolver.should_run_internal_schema_search();
    let should_run_schema_search = has_effective_tables
        && (schema_search_enabled || internal_schema_search || sql_select_enabled);
    
    let has_effective_tools = !attached_tools.is_empty() 
        || !always_on_builtin_tools.is_empty() 
        || !always_on_mcp_tools.is_empty();
    let should_run_tool_search = tool_search_enabled
        && turn_config.enabled_tools.is_empty() 
        && (has_effective_tools || !filtered_tool_descriptions.is_empty());

    let auto_discovery = perform_auto_discovery_for_prompt(
        &user_prompt,
        should_run_tool_search,
        settings_for_resolver.tool_search_max_results,
        !filtered_tool_descriptions.is_empty(),
        should_run_schema_search,
        settings_for_resolver.schema_relevancy_threshold,
        &database_toolbox_config,
        &filtered_tool_descriptions,
        tool_registry_state.registry.clone(),
        embedding_state.cpu_model.clone(), // CPU model for search during chat
        handles.schema_tx.clone(),
        false, // do_not_materialize
    ).await;

    let has_attachments = !attached_files.is_empty();

    let (resolved_capabilities, model_tool_format) = {
        let registry = tool_registry_state.registry.read().await;
        let (tx, rx) = oneshot::channel();
        let fetched_model_info = if handles.foundry_tx.send(FoundryMsg::GetCurrentModel { respond_to: tx }).await.is_ok() {
            rx.await.ok().flatten()
        } else {
            None
        };
        let default_model_info = ModelInfo {
            id: "unknown".to_string(),
            family: ModelFamily::Generic,
            tool_calling: false,
            tool_format: ToolFormat::TextBased,
            vision: false,
            reasoning: false,
            reasoning_format: protocol::ReasoningFormat::None,
            max_input_tokens: 4096,
            max_output_tokens: 2048,
            supports_tool_calling: false,
            supports_temperature: true,
            supports_top_p: true,
            supports_reasoning_effort: false,
        };
        let model_info = fetched_model_info.as_ref().unwrap_or(&default_model_info);
        let caps = ToolCapabilityResolver::resolve(&settings_for_resolver, model_info, &tool_filter, &server_configs, &registry);
        (caps, Some(model_info.tool_format))
    };

    let empty_tools: Vec<(String, Vec<McpTool>)> = Vec::new();
    let mut mcp_context = agentic_state::McpToolContext::from_tool_lists(
        if tool_search_enabled { &empty_tools } else { &filtered_tool_descriptions },
        if tool_search_enabled { &filtered_tool_descriptions } else { &empty_tools },
        &server_configs,
    );

    // If turn-specific tools are attached, override the context
    if !turn_config.enabled_tools.is_empty() {
        let mut active_mcp = Vec::new();
        for (server_id, tools) in filtered_tool_descriptions {
            let attached: Vec<McpToolInfo> = tools.into_iter()
                .filter(|t| turn_config.enabled_tools.contains(&format!("{}::{}", server_id, t.name)))
                .map(|t| McpToolInfo::from_mcp_tool(&t))
                .collect();
            if !attached.is_empty() {
                active_mcp.push((server_id, attached));
            }
        }
        mcp_context.active_tools = active_mcp;
        mcp_context.deferred_tools = Vec::new();
    }

    let mut initial_state_machine = AgenticStateMachine::new_from_settings_sm(
        &settings_sm,
        crate::agentic_state::PromptContext {
            base_prompt: base_prompt.clone(),
            attached_tables: turn_context.attached_tables.clone(),
            attached_tools: attached_tools,
            attached_tabular_files: turn_context.attached_tabular_files.clone(),
            tabular_column_info: Vec::new(), // Not needed for preview
            mcp_context,
            tool_call_format: resolved_capabilities.primary_format,
            model_tool_format,
            custom_tool_prompts: tool_system_prompts,
            python_primary: resolved_capabilities.available_builtins.contains(tool_capability::BUILTIN_PYTHON_EXECUTION),
            has_attachments,
        },
    );

    // Initialize with turn config
    initial_state_machine.compute_turn_config(&settings_for_resolver, &tool_filter);

    // Extract schema search results for state machine initialization
    let schema_relevancy = auto_discovery.schema_search_output.as_ref()
        .map(|o| o.tables.iter().map(|t| t.relevance).fold(0.0f32, f32::max))
        .unwrap_or(0.0);
    
    let discovered_tables = auto_discovery.schema_search_output.as_ref()
        .map(|o| o.tables.iter().map(|t| agentic_state::TableInfo {
            fully_qualified_name: t.table_name.clone(),
            source_id: t.source_id.clone(),
            sql_dialect: t.sql_dialect.clone(),
            relevancy: t.relevance,
            description: t.description.clone(),
            columns: t.relevant_columns.iter().map(|c| agentic_state::ColumnInfo {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: true,
                description: c.description.clone(),
                special_attributes: c.special_attributes.clone(),
                top_values: c.top_values.clone(),
            }).collect(),
        }).collect())
        .unwrap_or_default();

    initial_state_machine.compute_initial_state(
        0.0,
        schema_relevancy,
        discovered_tables,
        Vec::new(),
    );

    initial_state_machine.set_auto_discovery_context(auto_discovery.tool_search_output, auto_discovery.schema_search_output);

    Ok(initial_state_machine.build_system_prompt())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Fix PATH for macOS GUI applications (required for finding 'foundry' CLI in production builds).
    // macOS GUI apps don't inherit shell PATH from dotfiles (.zshrc, .bashrc, etc.).
    // This spawns the user's login shell to extract the correct PATH and sets it.
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = fix_macos_path_env() {
            eprintln!("[Launch] Warning: Failed to fix PATH environment: {}", e);
        }
    }

    let cli_args = CliArgs::try_parse().unwrap_or_else(|e| {
        println!("[Launch] CLI parse warning: {}", e);
        // Fall back to defaults (no overrides) if parsing fails
        CliArgs::parse_from(["plugable-chat"])
    });
    if cli_args.run_mcp_test_server {
        let mut server_args = McpTestCliArgs::default();
        server_args.host = cli_args.mcp_test_host.clone();
        server_args.port = cli_args.mcp_test_port;
        server_args.run_all_on_start = cli_args.mcp_test_run_all_on_start;
        server_args.print_prompt = cli_args.mcp_test_print_prompt;
        server_args.open_ui = cli_args.mcp_test_open_ui;
        server_args.serve_ui = cli_args.mcp_test_serve_ui;

        println!(
            "[Launch] Starting dev MCP test server at http://{}:{} (ui={}, open_ui={}, run_all_on_start={})",
            server_args.host, server_args.port, server_args.serve_ui, server_args.open_ui, server_args.run_all_on_start
        );

        if let Err(e) = tauri::async_runtime::block_on(run_mcp_test_server(server_args)) {
            eprintln!("[Launch] MCP test server exited with error: {}", e);
            std::process::exit(1);
        }
        return;
    }
    let cli_args_for_setup = cli_args.clone();
    let launch_filter = parse_tool_filter(&cli_args);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // Set window title with version number
            if let Some(window) = app.get_webview_window("main") {
                let git_count = option_env!("PLUGABLE_CHAT_GIT_COUNT")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                let version_title = format!("Plugable Chat v0.{:03} - Microsoft Foundry", git_count);
                if let Err(e) = window.set_title(&version_title) {
                    eprintln!("[Launch] Failed to set window title: {}", e);
                }
            }

            // Initialize channels
            let (vector_tx, vector_rx) = mpsc::channel(32);
            let (foundry_tx, foundry_rx) = mpsc::channel(32);
            let (rag_tx, rag_rx) = mpsc::channel(32);
            let (mcp_host_tx, mcp_host_rx) = mpsc::channel(32);
            let (python_tx, python_rx) = mpsc::channel(32);
            let (database_toolbox_tx, database_toolbox_rx) = mpsc::channel(32);
            let (schema_tx, schema_rx) = mpsc::channel(32);
            let (startup_tx, startup_rx) = mpsc::channel(32);
            let python_mcp_host_tx = mcp_host_tx.clone();
            let mcp_host_tx_for_db = mcp_host_tx.clone();
            let mcp_host_tx_for_handles = mcp_host_tx.clone();
            let startup_tx_for_foundry = startup_tx.clone();
            let startup_tx_for_handles = startup_tx.clone();
            let logging_persistence = Arc::new(LoggingPersistence::default());
            let logging_persistence_for_foundry = logging_persistence.clone();
            
            // Create GPU resource guard for serializing GPU operations
            let gpu_guard = Arc::new(GpuResourceGuard::new());
            let gpu_guard_for_foundry = gpu_guard.clone();

            // Store handles in state
            app.manage(ActorHandles {
                vector_tx,
                foundry_tx,
                rag_tx,
                mcp_host_tx: mcp_host_tx_for_handles,
                python_tx,
                database_toolbox_tx: database_toolbox_tx.clone(),
                schema_tx: schema_tx.clone(),
                startup_tx: startup_tx_for_handles,
                logging_persistence,
                gpu_guard,
            });

            // Initialize shared embedding model state
            // NOTE: GPU EMBEDDING DISABLED - Only CPU model is used.
            // The gpu_model field is kept for API compatibility but will always be None.
            // To re-enable GPU embedding, see foundry_actor.rs and Cargo.toml.
            let embedding_model_state = EmbeddingModelState {
                gpu_model: Arc::new(RwLock::new(None)), // DISABLED - always None
                cpu_model: Arc::new(RwLock::new(None)),
            };
            let gpu_embedding_model_arc = embedding_model_state.gpu_model.clone();
            let cpu_embedding_model_arc = embedding_model_state.cpu_model.clone();
            app.manage(embedding_model_state);
            let embedding_model_arc_for_python = cpu_embedding_model_arc.clone();

            // Initialize shared tool registry
            let tool_registry = create_shared_registry();
            let tool_registry_state = ToolRegistryState {
                registry: tool_registry.clone(),
            };
            app.manage(tool_registry_state);

            // Initialize settings state (load from config file)
            let mut app_settings =
                tauri::async_runtime::block_on(async { settings::load_settings().await });
            let launch_overrides = apply_cli_overrides(&cli_args_for_setup, &mut app_settings);
            println!(
                "Settings loaded: {} MCP servers configured",
                app_settings.mcp_servers.len()
            );
            // Create SettingsStateMachine (Tier 1 of the three-tier hierarchy)
            let settings_sm = SettingsStateMachine::from_settings(&app_settings, &launch_filter);
            println!(
                "[SettingsStateMachine] Initialized with mode: {} (capabilities: {:?})",
                settings_sm.operational_mode().name(),
                settings_sm.enabled_capabilities()
            );
            
            let settings_state = SettingsState {
                settings: Arc::new(RwLock::new(app_settings)),
            };
            app.manage(settings_state);
            
            // Manage the settings state machine
            let settings_sm_state = SettingsStateMachineState {
                machine: Arc::new(RwLock::new(settings_sm)),
            };
            app.manage(settings_sm_state);

            // Launch config state (tool filters + overrides)
            app.manage(LaunchConfigState {
                tool_filter: launch_filter.clone(),
                launch_overrides: launch_overrides.clone(),
            });
            if launch_filter.allow_all() {
                println!("[Launch] Tool filter: all tools allowed");
            } else {
                println!(
                    "[Launch] Tool filter active (builtins={:?}, servers={:?}, tools={:?})",
                    launch_filter.allowed_builtins,
                    launch_filter.allowed_servers,
                    launch_filter.allowed_tools
                );
            }
            if launch_overrides.model.is_some() || launch_overrides.initial_prompt.is_some() {
                println!(
                    "[Launch] CLI overrides applied (model_override={}, initial_prompt={})",
                    launch_overrides.model.as_deref().unwrap_or("none"),
                    if launch_overrides.initial_prompt.is_some() {
                        "provided"
                    } else {
                        "none"
                    }
                );
            }

            // Initialize tool approval state
            let approval_state = ToolApprovalState {
                pending: Arc::new(RwLock::new(HashMap::new())),
            };
            app.manage(approval_state);

            // Initialize cancellation state for stream abort
            let cancellation_state = CancellationState {
                cancel_signal: Arc::new(RwLock::new(None)),
                current_generation_id: Arc::new(RwLock::new(0)),
            };
            app.manage(cancellation_state);

            // Track turn progress for reconnect/replay
            let turn_tracker_state = TurnTrackerState {
                progress: Arc::new(RwLock::new(TurnProgress::default())),
            };
            app.manage(turn_tracker_state);

            // Track frontend heartbeat (1s cadence) for backend-side logging
            let heartbeat_state = HeartbeatState::default();
            app.manage(heartbeat_state.clone());
            const FRONTEND_HEARTBEAT_TIMEOUT_MS: u64 = 4000;
            tauri::async_runtime::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(1));
                loop {
                    ticker.tick().await;
                    let now = Instant::now();
                    let last_opt = {
                        let guard = heartbeat_state.last_frontend_beat.read().await;
                        *guard
                    };

                    if let Some(last) = last_opt {
                        let gap = now.saturating_duration_since(last);
                        if gap.as_millis() as u64 >= FRONTEND_HEARTBEAT_TIMEOUT_MS {
                            let mut logged_guard = heartbeat_state.logged_unresponsive.write().await;
                            if !*logged_guard {
                                println!(
                                    "[Heartbeat] Frontend heartbeat missing for {} ms",
                                    gap.as_millis()
                                );
                                *logged_guard = true;
                            }
                        } else {
                            // Recovered; allow future gaps to log again
                            let mut logged_guard = heartbeat_state.logged_unresponsive.write().await;
                            if *logged_guard {
                                *logged_guard = false;
                            }
                        }
                    } else {
                        // No heartbeat seen yet since app start
                        let gap = now.saturating_duration_since(heartbeat_state.start_instant);
                        if gap.as_millis() as u64 >= FRONTEND_HEARTBEAT_TIMEOUT_MS {
                            let mut logged_never_guard = heartbeat_state.logged_never_seen.write().await;
                            if !*logged_never_guard {
                                println!(
                                    "[Heartbeat] No frontend heartbeat received yet ({} ms since start)",
                                    gap.as_millis()
                                );
                                *logged_never_guard = true;
                            }
                        }
                    }
                }
            });

            let app_handle = app.handle();
            // Spawn Vector Actor
            tauri::async_runtime::spawn(async move {
                // Get writable data directory with fallback chain
                let writable = paths::ensure_writable_dir(
                    paths::get_data_dir().join("lancedb"),
                    "chat-vectors",
                )
                .await;

                if writable.is_fallback {
                    if let Some(reason) = &writable.fallback_reason {
                        println!("[VectorActor] {}", reason);
                    }
                }

                let actor = ChatVectorStoreActor::new(vector_rx, &writable.path.to_string_lossy()).await;
                actor.run().await;
            });

            // Spawn Foundry Actor (manages embedding model initialization)
            // NOTE: GPU EMBEDDING DISABLED - Only CPU embedding is active.
            // The GPU model Arc is still passed for API compatibility but won't be populated.
            let foundry_app_handle = app_handle.clone();
            let gpu_embedding_model_arc_for_foundry = gpu_embedding_model_arc.clone();
            let cpu_embedding_model_arc_for_foundry = cpu_embedding_model_arc.clone();
            tauri::async_runtime::spawn(async move {
                let actor = ModelGatewayActor::new(
                    foundry_rx,
                    foundry_app_handle,
                    gpu_embedding_model_arc_for_foundry,
                    cpu_embedding_model_arc_for_foundry,
                    logging_persistence_for_foundry,
                    gpu_guard_for_foundry,
                    Some(startup_tx_for_foundry),
                );
                actor.run().await;
            });

            // Spawn RAG Actor
            let rag_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let actor = RagRetrievalActor::new(
                    rag_rx,
                    Some(rag_app_handle),
                );
                actor.run().await;
            });

            // Spawn Startup Coordinator Actor
            let startup_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let actor = StartupCoordinatorActor::new(startup_rx, startup_app_handle);
                actor.run().await;
            });

            // Spawn MCP Host Actor
            tauri::async_runtime::spawn(async move {
                let actor = McpToolRouterActor::new(mcp_host_rx);
                actor.run().await;
            });

            // Spawn Python Actor for code execution
            let python_tool_registry = tool_registry.clone();
            tauri::async_runtime::spawn(async move {
                let actor = PythonSandboxActor::new(
                    python_rx,
                    python_tool_registry,
                    python_mcp_host_tx,
                    embedding_model_arc_for_python,
                );
                actor.run().await;
            });

            // Embedding model initialization is now handled by ModelGatewayActor
            // after it detects available execution providers from Foundry Local

            // Spawn Database Toolbox Actor
            let database_toolbox_state = Arc::new(RwLock::new(
                actors::database_toolbox_actor::DatabaseToolboxState::default(),
            ));
            let db_state_clone = database_toolbox_state.clone();
            tauri::async_runtime::spawn(async move {
                let actor = DatabaseToolboxActor::new(
                    database_toolbox_rx,
                    db_state_clone,
                    mcp_host_tx_for_db,
                );
                actor.run().await;
            });

            // Spawn Schema Vector Store Actor
            tauri::async_runtime::spawn(async move {
                // Get writable data directory with fallback chain
                let writable = paths::ensure_writable_dir(
                    paths::get_data_dir().join("lancedb"),
                    "schema-vectors",
                )
                .await;

                if writable.is_fallback {
                    if let Some(reason) = &writable.fallback_reason {
                        println!("[SchemaVectorActor] {}", reason);
                    }
                }

                let actor = SchemaVectorStoreActor::new(schema_rx, &writable.path.to_string_lossy()).await;
                actor.run().await;
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
            // Model loading commands
            download_model,
            load_model,
            get_loaded_models,
            reload_foundry,
            get_catalog_models,
            unload_model,
            get_foundry_service_status,
            get_current_model,
            get_model_state,
            remove_cached_model,
            cancel_generation,
            get_turn_status,
            // RAG commands
            select_files,
            select_folder,
            process_rag_documents,
            search_rag_context,
            clear_rag_context,
            remove_rag_file,
            get_rag_indexed_files,
            get_test_data_directory,
            parse_tabular_headers,
            // Settings commands
            get_settings,
            get_default_mcp_test_server,
            get_python_allowed_imports,
            save_app_settings,
            add_mcp_server,
            update_mcp_server,
            remove_mcp_server,
            update_system_prompt,
            update_tool_system_prompt,
            update_tool_call_formats,
            update_chat_format,
            update_rag_chunk_min_relevancy,
            update_schema_relevancy_threshold,
            update_rag_dominant_threshold,
            // Always-on configuration commands
            update_always_on_builtin_tools,
            update_always_on_mcp_tools,
            update_always_on_tables,
            update_always_on_rag_paths,
            get_state_machine_preview,
            update_database_toolbox_config,
            get_cached_database_schemas,
            refresh_database_schemas,
            refresh_database_schema_for_source,
            search_database_tables,
            set_schema_table_enabled,
            check_table_name_conflicts,
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
            get_pending_tool_approvals,
            get_current_model,
            get_launch_overrides,
            heartbeat_ping,
            // Startup coordination commands
            frontend_ready,
            get_startup_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod inline_tests {
    use crate::settings::{AppSettings, ToolCallFormatName, ToolCallFormatConfig, McpServerConfig};
    use crate::protocol::{ToolFormat, ParsedToolCall};
    use crate::tool_capability::ToolLaunchFilter;
    use crate::python_helpers::{fix_python_indentation, strip_unsupported_python};
    use crate::agentic_loop::{AgenticLoopAction, detect_agentic_loop_action};
    use crate::tool_parsing::format_tool_result;

    // Helper to create test ResolvedToolCapabilities
    use super::*;
    use serde_json::json;

    // Alias for compatibility with existing test code
    type AgenticAction = AgenticLoopAction;
    fn detect_agentic_action(
        response: &str,
        model_family: crate::protocol::ModelFamily,
        tool_format: ToolFormat,
        python_tool_mode: bool,
        formats: &crate::settings::ToolCallFormatConfig,
        primary_format: ToolCallFormatName,
    ) -> AgenticLoopAction {
        // Pass false for python_execution_in_native_tools in legacy tests
        detect_agentic_loop_action(response, model_family, tool_format, python_tool_mode, formats, primary_format, false)
    }

    fn hermes_call(name: &str, args: serde_json::Value) -> String {
        format!(
            "<tool_call>{{\"name\": \"{}\", \"arguments\": {}}}</tool_call>",
            name,
            args.to_string()
        )
    }

    fn unwrap_tool_calls(action: AgenticAction) -> Vec<ParsedToolCall> {
        match action {
            AgenticAction::ToolCalls { calls } => calls,
            AgenticAction::Final { response } => {
                panic!("expected tool calls, got final response: {}", response)
            }
        }
    }

    #[test]
    fn test_fix_python_indentation_if_else() {
        // Tests if/else - the `else` keyword signals dedent
        let input = vec![
            "if x > 0:".to_string(),
            "print('positive')".to_string(),
            "else:".to_string(),
            "print('not positive')".to_string(),
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "if x > 0:");
        assert_eq!(result[1], "    print('positive')");
        assert_eq!(result[2], "else:");
        assert_eq!(result[3], "    print('not positive')");
    }

    #[test]
    fn test_fix_python_indentation_nested() {
        let input = vec![
            "for i in range(10):".to_string(),
            "if i % 2 == 0:".to_string(),
            "print('even')".to_string(),
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "for i in range(10):");
        assert_eq!(result[1], "    if i % 2 == 0:");
        assert_eq!(result[2], "        print('even')");
    }

    #[test]
    fn test_fix_python_indentation_preserves_existing() {
        // When input has existing indentation, preserve it as-is
        // This prevents incorrectly indenting top-level code after functions
        let input = vec![
            "for i in range(10):".to_string(),
            "    print(i)".to_string(), // Already indented
            "print('done')".to_string(), // Top-level, should stay top-level
        ];

        let result = fix_python_indentation(&input);

        // With existing indentation, we preserve the structure exactly
        assert_eq!(result[0], "for i in range(10):");
        assert_eq!(result[1], "    print(i)"); // Preserved
        assert_eq!(result[2], "print('done')"); // Stays at top level!
    }

    #[test]
    fn test_fix_python_indentation_function_def() {
        let input = vec!["def foo():".to_string(), "return 42".to_string()];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "def foo():");
        assert_eq!(result[1], "    return 42");
    }

    #[test]
    fn test_fix_python_indentation_function_with_return_dedent() {
        // return statement signals end of block
        let input = vec![
            "def foo():".to_string(),
            "x = 1".to_string(),
            "return x".to_string(),
            "y = 2".to_string(), // After return, this is at previous level
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "def foo():");
        assert_eq!(result[1], "    x = 1");
        assert_eq!(result[2], "    return x");
        assert_eq!(result[3], "y = 2"); // Dedented after return
    }

    #[test]
    fn test_try_except() {
        let input = vec![
            "try:".to_string(),
            "x = int(s)".to_string(),
            "except:".to_string(),
            "x = 0".to_string(),
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "try:");
        assert_eq!(result[1], "    x = int(s)");
        assert_eq!(result[2], "except:");
        assert_eq!(result[3], "    x = 0");
    }

    #[test]
    fn test_dice_roll_example() {
        // The exact case from the bug report
        // NOTE: The algorithm will over-indent lines 9-10 because it can't know
        // where the for loop ends without explicit indentation. However, the code
        // will still execute correctly because the result is still computed.
        let input = vec![
            "from random import randint".to_string(),
            "total_rolls = 10000".to_string(),
            "success_count = 0".to_string(),
            "for _ in range(total_rolls):".to_string(),
            "roll1 = randint(1, 6)".to_string(),
            "roll2 = randint(1, 6)".to_string(),
            "if roll1 + roll2 == 7:".to_string(),
            "success_count += 1".to_string(),
            "probability = success_count / total_rolls * 100".to_string(),
            "print(f'Percentage: {probability:.2f}%')".to_string(),
        ];

        let result = fix_python_indentation(&input);

        // Print for debugging
        for (i, line) in result.iter().enumerate() {
            println!("{}: {:?}", i, line);
        }

        // First 4 lines
        assert_eq!(result[0], "from random import randint");
        assert_eq!(result[1], "total_rolls = 10000");
        assert_eq!(result[2], "success_count = 0");
        assert_eq!(result[3], "for _ in range(total_rolls):");

        // Lines inside for loop - these MUST be indented
        assert_eq!(result[4], "    roll1 = randint(1, 6)");
        assert_eq!(result[5], "    roll2 = randint(1, 6)");
        assert_eq!(result[6], "    if roll1 + roll2 == 7:");
        assert_eq!(result[7], "        success_count += 1");

        // These lines will be over-indented (inside the if block)
        // but the code will still work because we're just computing and printing
        // This is a limitation of the auto-fix without more context
    }

    #[test]
    fn test_strip_unsupported_await() {
        let input = vec![
            "result = await list_dataset_ids()".to_string(),
            "print(result)".to_string(),
        ];

        let result = strip_unsupported_python(&input);

        assert_eq!(result[0], "result = list_dataset_ids()");
        assert_eq!(result[1], "print(result)");
    }

    #[test]
    fn test_strip_unsupported_await_with_args() {
        let input = vec!["data = await sql_select(query=\"SELECT * FROM users\")".to_string()];

        let result = strip_unsupported_python(&input);

        assert_eq!(
            result[0],
            "data = sql_select(query=\"SELECT * FROM users\")"
        );
    }

    #[test]
    fn test_strip_unsupported_preserves_comments() {
        let input = vec![
            "# await is used for async operations".to_string(),
            "result = await foo()".to_string(),
        ];

        let result = strip_unsupported_python(&input);

        // Comment preserved as-is
        assert_eq!(result[0], "# await is used for async operations");
        // await stripped from code
        assert_eq!(result[1], "result = foo()");
    }

    #[test]
    fn test_strip_unsupported_no_await() {
        let input = vec!["x = 1 + 2".to_string(), "print(x)".to_string()];

        let result = strip_unsupported_python(&input);

        // No change when no await present
        assert_eq!(result[0], "x = 1 + 2");
        assert_eq!(result[1], "print(x)");
    }


    #[test]
    fn test_centralized_system_prompt_via_state_machine() {
        let base_prompt = "Custom system prompt";
        let mut tool_prompts = HashMap::new();
        tool_prompts.insert("srv1::tool_a".to_string(), "Execute with caution".to_string());

        let mut server_config = McpServerConfig::new("srv1".to_string(), "Server 1".to_string());
        server_config.enabled = true;
        server_config.env.insert("API_URL".to_string(), "https://api.example.com".to_string());
        let server_configs = vec![server_config.clone()];

        let active_tools = vec![("srv1".to_string(), vec![McpTool {
            name: "tool_a".to_string(),
            description: Some("Useful tool".to_string()),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "param1": {"type": "string", "description": "A parameter"}
                }
            })),
            input_examples: None,
            allowed_callers: None,
        }])];

        let mut app_settings = AppSettings::default();
        app_settings.mcp_servers = server_configs.clone();
        let settings_sm = SettingsStateMachine::from_settings(&app_settings, &ToolLaunchFilter::default());
        let sm = AgenticStateMachine::new_from_settings_sm(
            &settings_sm,
            crate::agentic_state::PromptContext {
                base_prompt: base_prompt.to_string(),
                attached_tables: Vec::new(),
                attached_tools: Vec::new(),
                attached_tabular_files: Vec::new(),
                tabular_column_info: Vec::new(),
                mcp_context: crate::agentic_state::McpToolContext::from_tool_lists(
                    &active_tools,
                    &Vec::new(),
                    &server_configs,
                ),
                tool_call_format: ToolCallFormatName::Hermes,
                model_tool_format: None,
                custom_tool_prompts: tool_prompts,
                python_primary: false,
                has_attachments: false,
            },
        );

        let prompt = sm.build_system_prompt();

        assert!(prompt.contains(base_prompt));
        assert!(prompt.contains("Execute with caution"));
        assert!(prompt.contains("API_URL=https://api.example.com"));
        assert!(prompt.contains("tool_a"));
        assert!(prompt.contains("param1"));
        assert!(prompt.contains("## Tool Calling Format"));
    }

    #[test]
    fn detect_agentic_action_prefers_python_mode() {
        let response = "```python\nprint('hi')\n```";
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            true,
            &formats,
            ToolCallFormatName::CodeMode,
        );

        let calls = unwrap_tool_calls(action);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "python_execution");
        assert!(calls[0].arguments.get("code").is_some());
    }

    #[test]
    fn detect_agentic_action_rejects_invalid_python_syntax() {
        let response =
            "```python\nThe result of the expression 1 + 1 is 2.\n```"; // Not valid Python code
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            true,
            &formats,
            ToolCallFormatName::CodeMode,
        );

        match action {
            AgenticAction::Final { .. } => {}
            other => panic!("expected final response due to parse failure, got {:?}", other),
        }
    }

    #[test]
    fn detect_agentic_action_ignores_plaintext_multiline() {
        let response = "plaintext\n75992863";
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            true,
            &formats,
            ToolCallFormatName::CodeMode,
        );

        match action {
            AgenticAction::Final { .. } => {}
            other => panic!("expected final response (not code), got {:?}", other),
        }
    }

    #[test]
    fn detect_agentic_action_ignores_natural_language_with_parentheses() {
        let response = "The result of the mathematical expression 343 + (343423 * 343343) + (34234 / 2343) is 117911883446.61118.";
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            true,
            &formats,
            ToolCallFormatName::CodeMode,
        );

        match action {
            AgenticAction::Final { .. } => {}
            other => panic!("expected final response, got {:?}", other),
        }
    }

    #[test]
    fn detect_agentic_action_final_without_tools() {
        let response = "No tools needed here.";
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            false,
            &formats,
            formats.primary,
        );

        match action {
            AgenticAction::Final {
                response: final_response,
            } => {
                assert_eq!(final_response, "No tools needed here.");
            }
            other => panic!("expected final response, got {:?}", other),
        }
    }

    #[test]
    fn detect_agentic_action_parses_hermes_calls() {
        let response = hermes_call("server___echo", json!({ "text": "hello" }));
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            &response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            false,
            &formats,
            ToolCallFormatName::Hermes,
        );

        let calls = unwrap_tool_calls(action);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "server");
        assert_eq!(calls[0].tool, "echo");
        assert_eq!(
            calls[0].arguments.get("text").and_then(|v| v.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn simulate_one_turn_formats_tool_result() {
        let response = hermes_call("builtin___echo", json!({ "text": "hi" }));
        let formats = ToolCallFormatConfig::default();
        let action = detect_agentic_action(
            &response,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            false,
            &formats,
            ToolCallFormatName::Hermes,
        );
        let calls = unwrap_tool_calls(action);
        let formatted = format_tool_result(&calls[0], "echo: hi", false, ToolFormat::Hermes, None, None);

        assert!(
            formatted.contains("echo: hi"),
            "formatted result should include tool output"
        );
        assert!(
            formatted.contains("<tool_response>"),
            "Hermes formatting should wrap in tool_response tags"
        );
    }

}
