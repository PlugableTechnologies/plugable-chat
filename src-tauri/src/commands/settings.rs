//! Settings management Tauri commands.
//!
//! Commands for managing application settings, MCP server configurations,
//! system prompts, tool formats, and various feature toggles.

use crate::agentic_state;
use crate::app_state::{
    ActorHandles, EmbeddingModelState, LaunchConfigState, SettingsState, SettingsStateMachineState,
};
use crate::protocol::McpHostMsg;
use crate::settings::{
    self, enforce_python_name, AppSettings, ChatFormatName, McpServerConfig, ToolCallFormatConfig,
};
use crate::state_machine::{AgenticStateMachine, StatePreview};
use python_sandbox::sandbox::ALLOWED_MODULES as PYTHON_ALLOWED_MODULES;
use tauri::State;
use tokio::sync::oneshot;

use super::database::refresh_database_schemas_for_config;

/// Build a consistent key for tool-specific settings
pub fn tool_prompt_key(server_id: &str, tool_name: &str) -> String {
    format!("{}::{}", server_id, tool_name)
}

/// Get current application settings
#[tauri::command]
pub async fn get_settings(settings_state: State<'_, SettingsState>) -> Result<AppSettings, String> {
    let guard = settings_state.settings.read().await;
    Ok(guard.clone())
}

/// Get the default MCP test server configuration
#[tauri::command]
pub fn get_default_mcp_test_server() -> McpServerConfig {
    settings::default_mcp_test_server()
}

/// Get list of Python modules allowed in the sandbox
#[tauri::command]
pub fn get_python_allowed_imports() -> Vec<String> {
    PYTHON_ALLOWED_MODULES
        .iter()
        .map(|m| m.to_string())
        .collect()
}

/// Save application settings
#[tauri::command]
pub async fn save_app_settings(
    new_settings: AppSettings,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut normalized = new_settings;
    normalized.tool_call_formats.normalize();

    // Save to file
    settings::save_settings(&normalized).await?;

    // Update in-memory state
    let mut guard = settings_state.settings.write().await;
    *guard = normalized.clone();

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    let mode_changed = sm_guard.refresh(&normalized, &launch_config.tool_filter);
    if mode_changed {
        println!(
            "[SettingsStateMachine] Mode updated after settings save: {}",
            sm_guard.operational_mode().name()
        );
    }

    Ok(())
}

/// Add a new MCP server configuration
#[tauri::command]
pub async fn add_mcp_server(
    mut config: McpServerConfig,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    enforce_python_name(&mut config);

    let mut guard = settings_state.settings.write().await;

    // Check for duplicate ID
    if guard.mcp_servers.iter().any(|s| s.id == config.id) {
        return Err(format!("Server with ID '{}' already exists", config.id));
    }

    guard.mcp_servers.push(config);
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    Ok(())
}

/// Update an existing MCP server configuration
#[tauri::command]
pub async fn update_mcp_server(
    mut config: McpServerConfig,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    enforce_python_name(&mut config);

    let all_configs_for_sync;
    {
        let mut guard = settings_state.settings.write().await;

        if let Some(server) = guard.mcp_servers.iter_mut().find(|s| s.id == config.id) {
            *server = config;
            settings::save_settings(&guard).await?;
            all_configs_for_sync = guard.get_all_mcp_configs();

            // Refresh the SettingsStateMachine (Tier 1)
            let mut sm_guard = settings_sm_state.machine.write().await;
            sm_guard.refresh(&guard, &launch_config.tool_filter);
        } else {
            return Err(format!("Server with ID '{}' not found", config.id));
        }
    }

    // Sync enabled servers after settings change
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::SyncEnabledServers {
            configs: all_configs_for_sync,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let results = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
    for (server_id, result) in results {
        match result {
            Ok(()) => println!("[Settings] Server {} sync successful", server_id),
            Err(e) => println!("[Settings] Server {} sync failed: {}", server_id, e),
        }
    }

    Ok(())
}

/// Remove an MCP server configuration
#[tauri::command]
pub async fn remove_mcp_server(
    server_id: String,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;

    let initial_len = guard.mcp_servers.len();
    guard.mcp_servers.retain(|s| s.id != server_id);
    let prefix = format!("{}::", server_id);
    guard
        .tool_system_prompts
        .retain(|key, _| !key.starts_with(&prefix));

    if guard.mcp_servers.len() < initial_len {
        settings::save_settings(&guard).await?;

        // Refresh the SettingsStateMachine (Tier 1)
        let mut sm_guard = settings_sm_state.machine.write().await;
        sm_guard.refresh(&guard, &launch_config.tool_filter);

        // Explicitly disconnect the removed server
        let (tx, rx) = oneshot::channel();
        let _ = handles
            .mcp_host_tx
            .send(McpHostMsg::DisconnectServer {
                server_id: server_id.clone(),
                respond_to: tx,
            })
            .await;
        let _ = rx.await;

        Ok(())
    } else {
        Err(format!("Server with ID '{}' not found", server_id))
    }
}

/// Update the global system prompt
#[tauri::command]
pub async fn update_system_prompt(
    prompt: String,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.system_prompt = prompt;
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    Ok(())
}

/// Update a tool-specific system prompt
#[tauri::command]
pub async fn update_tool_system_prompt(
    server_id: String,
    tool_name: String,
    prompt: String,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    let key = tool_prompt_key(&server_id, &tool_name);

    if prompt.trim().is_empty() {
        guard.tool_system_prompts.remove(&key);
    } else {
        guard.tool_system_prompts.insert(key, prompt);
    }

    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    Ok(())
}

/// Update tool call format configuration
#[tauri::command]
pub async fn update_tool_call_formats(
    config: ToolCallFormatConfig,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut normalized = config;
    normalized.normalize();
    let mut guard = settings_state.settings.write().await;
    guard.tool_call_formats = normalized.clone();
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!(
        "[Settings] tool_call_formats updated: primary={:?}, enabled={:?}",
        normalized.primary, normalized.enabled
    );
    Ok(())
}

/// Update chat format for a specific model
#[tauri::command]
pub async fn update_chat_format(
    model_id: String,
    format: ChatFormatName,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;

    // Store override only when different from default to keep config small
    if format == guard.chat_format_default {
        guard.chat_format_overrides.remove(&model_id);
    } else {
        guard.chat_format_overrides.insert(model_id.clone(), format);
    }

    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!(
        "[Settings] chat_format updated: model_id={} format={:?}",
        model_id, format
    );
    Ok(())
}

/// Update RAG chunk minimum relevancy threshold
#[tauri::command]
pub async fn update_rag_chunk_min_relevancy(
    value: f32,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.rag_chunk_min_relevancy = value;
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] rag_chunk_min_relevancy updated to: {}", value);
    Ok(())
}

/// Update schema relevancy threshold
#[tauri::command]
pub async fn update_schema_relevancy_threshold(
    value: f32,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.schema_relevancy_threshold = value;
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] schema_relevancy_threshold updated to: {}", value);
    Ok(())
}

/// Update RAG dominant threshold
#[tauri::command]
pub async fn update_rag_dominant_threshold(
    value: f32,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.rag_dominant_threshold = value;
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] rag_dominant_threshold updated to: {}", value);
    Ok(())
}

// ============ Always-On Configuration Commands ============

/// Update always-on built-in tools list
#[tauri::command]
pub async fn update_always_on_builtin_tools(
    tools: Vec<String>,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.always_on_builtin_tools = tools.clone();
    settings::save_settings(&guard).await?;

    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] always_on_builtin_tools updated: {:?}", tools);
    Ok(())
}

/// Update always-on MCP tools list
#[tauri::command]
pub async fn update_always_on_mcp_tools(
    tools: Vec<String>,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.always_on_mcp_tools = tools.clone();
    settings::save_settings(&guard).await?;

    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] always_on_mcp_tools updated: {:?}", tools);
    Ok(())
}

/// Update always-on database tables list
#[tauri::command]
pub async fn update_always_on_tables(
    tables: Vec<settings::AlwaysOnTableConfig>,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.always_on_tables = tables.clone();
    settings::save_settings(&guard).await?;

    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] always_on_tables updated: {} tables", tables.len());
    Ok(())
}

/// Update always-on RAG paths list
#[tauri::command]
pub async fn update_always_on_rag_paths(
    paths: Vec<String>,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.always_on_rag_paths = paths.clone();
    settings::save_settings(&guard).await?;

    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] always_on_rag_paths updated: {:?}", paths);
    Ok(())
}

/// Get a preview of all possible states for the settings UI
#[tauri::command]
pub async fn get_state_machine_preview(
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
) -> Result<Vec<StatePreview>, String> {
    let guard = settings_state.settings.read().await;

    // Use three-tier hierarchy: SettingsStateMachine provides capabilities
    let settings_sm_guard = settings_sm_state.machine.read().await;

    // Create a minimal PromptContext for preview
    let prompt_context = agentic_state::PromptContext {
        base_prompt: guard.system_prompt.clone(),
        has_attachments: false,
        attached_tables: Vec::new(),
        attached_tools: Vec::new(),
        mcp_context: agentic_state::McpToolContext::default(),
        tool_call_format: guard.tool_call_formats.primary,
        model_tool_format: None,
        custom_tool_prompts: guard.tool_system_prompts.clone(),
        python_primary: guard.is_builtin_always_on("python_execution"),
    };

    let machine = AgenticStateMachine::new_from_settings_sm(&settings_sm_guard, prompt_context);

    let previews = machine.get_possible_states();
    println!(
        "[Settings] State machine preview: {} possible states (mode: {})",
        previews.len(),
        settings_sm_guard.operational_mode().name()
    );
    Ok(previews)
}

/// Update database toolbox configuration
#[tauri::command]
pub async fn update_database_toolbox_config(
    app_handle: tauri::AppHandle,
    config: settings::DatabaseToolboxConfig,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<(), String> {
    use crate::actors::database_toolbox_actor::DatabaseToolboxMsg;

    let mut guard = settings_state.settings.write().await;
    guard.database_toolbox = config.clone();
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] database_toolbox config updated");
    drop(guard);

    // If toolbox is disabled or has no enabled sources, ensure it's stopped
    if !config.enabled || config.sources.iter().all(|s| !s.enabled) {
        let (tx, rx) = oneshot::channel();
        let _ = handles
            .database_toolbox_tx
            .send(DatabaseToolboxMsg::Stop { reply_to: tx })
            .await;
        let _ = rx.await;
        println!("[Settings] database_toolbox stopped because it is disabled");
        return Ok(());
    }

    let refresh_summary =
        refresh_database_schemas_for_config(&app_handle, &handles, &embedding_state, &config).await?;

    if !refresh_summary.errors.is_empty() {
        let joined = refresh_summary.errors.join("; ");
        println!(
            "[Settings] Schema refresh completed with errors after config save: {}",
            &joined
        );
        return Err(format!(
            "Schema refresh failed after saving database settings: {}. Fix the MCP database configuration here and try again.",
            joined
        ));
    }

    Ok(())
}

/// Get launch overrides from CLI
#[tauri::command]
pub async fn get_launch_overrides(
    launch_config: State<'_, LaunchConfigState>,
) -> Result<LaunchOverridesPayload, String> {
    Ok(LaunchOverridesPayload {
        model: launch_config.launch_overrides.model.clone(),
        initial_prompt: launch_config.launch_overrides.initial_prompt.clone(),
    })
}

/// Payload for launch overrides
#[derive(Clone, serde::Serialize)]
pub struct LaunchOverridesPayload {
    pub model: Option<String>,
    pub initial_prompt: Option<String>,
}
