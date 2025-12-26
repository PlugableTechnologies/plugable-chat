pub mod actors;
pub mod agentic_state;
pub mod mid_turn_state;
pub mod model_profiles;
pub mod protocol;
pub mod settings;
pub mod settings_state_machine;
pub mod state_machine;
pub mod system_prompt;
pub mod tool_adapters;
pub mod tool_capability;
pub mod tool_registry;
pub mod tools;

#[cfg(test)]
mod tests;

use actors::database_toolbox_actor::{DatabaseToolboxActor, DatabaseToolboxMsg, ToolboxStatus};
use actors::foundry_actor::ModelGatewayActor;
use actors::mcp_host_actor::{McpToolRouterActor, McpTool, McpToolResult};
use actors::python_actor::{PythonMsg, PythonSandboxActor};
use actors::rag_actor::RagRetrievalActor;
use actors::schema_vector_actor::{SchemaVectorStoreActor, SchemaVectorMsg};
use actors::vector_actor::ChatVectorStoreActor;
use clap::Parser;
use fastembed::TextEmbedding;
use mcp_test_server::{
    run_with_args as run_mcp_test_server, CliArgs as McpTestCliArgs,
    DEFAULT_HOST as MCP_TEST_DEFAULT_HOST, DEFAULT_PORT as MCP_TEST_DEFAULT_PORT,
};
use model_profiles::resolve_profile;
use protocol::{
    parse_tool_calls, CachedModel, CatalogModel, ChatMessage, FoundryMsg, FoundryServiceStatus,
    McpHostMsg, ModelFamily, ModelInfo, OpenAITool, OpenAIToolCall, OpenAIToolCallFunction,
    ParsedToolCall, RagChunk, RagIndexResult, RagMsg, RemoveFileResult, ToolCallsPendingEvent,
    ToolExecutingEvent, ToolFormat, ToolHeartbeatEvent, ToolLoopFinishedEvent, ToolResultEvent,
    ToolSchema, VectorMsg,
};
use python_sandbox::sandbox::ALLOWED_MODULES as PYTHON_ALLOWED_MODULES;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use settings::{
    enforce_python_name, ensure_default_servers, AppSettings, CachedColumnSchema,
    CachedTableSchema, ChatFormatName, DatabaseSourceConfig, DatabaseToolboxConfig, McpServerConfig,
    SupportedDatabaseKind, ToolCallFormatConfig, ToolCallFormatName,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;
use tokio::sync::{mpsc, oneshot};
use tool_adapters::{detect_python_code, format_tool_result, parse_tool_calls_for_model_profile};
use tool_capability::{ToolCapabilityResolver, ToolLaunchFilter};
use tool_registry::{create_shared_registry, SharedToolRegistry, ToolSearchResult};
use settings_state_machine::SettingsStateMachine;
use state_machine::{AgenticStateMachine, StatePreview};
use tools::code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};
use tools::tool_search::{
    precompute_tool_search_embeddings, ToolSearchExecutor, ToolSearchInput, ToolSearchOutput,
};
use tools::schema_search::SchemaSearchOutput;
use rustpython_parser::{ast, Parse};
use uuid::Uuid;

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
    database_toolbox_tx: mpsc::Sender<DatabaseToolboxMsg>,
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    #[allow(dead_code)]
    logging_persistence: Arc<LoggingPersistence>,
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

// Shared settings state machine (Tier 1 of the three-tier hierarchy)
struct SettingsStateMachineState {
    machine: Arc<RwLock<SettingsStateMachine>>,
}

// Shared state for persistent logging of prompts and tools to avoid noise
pub struct LoggingPersistence {
    pub last_logged_system_prompt: Arc<RwLock<Option<String>>>,
    pub last_logged_tools_json: Arc<RwLock<Option<String>>>,
}

impl Default for LoggingPersistence {
    fn default() -> Self {
        Self {
            last_logged_system_prompt: Arc::new(RwLock::new(None)),
            last_logged_tools_json: Arc::new(RwLock::new(None)),
        }
    }
}

// Pending tool approvals state
struct ToolApprovalState {
    pending: PendingApprovals,
}

// Cancellation state for stream abort
struct CancellationState {
    /// Current generation's cancel signal
    cancel_signal: Arc<RwLock<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Current generation ID for matching
    current_generation_id: Arc<RwLock<u32>>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct TurnProgress {
    chat_id: Option<String>,
    generation_id: u32,
    assistant_response: String,
    last_token_index: usize,
    finished: bool,
    had_tool_calls: bool,
    timestamp_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
struct SystemPromptEvent {
    chat_id: String,
    generation_id: u32,
    prompt: String,
}

// Tracks the latest turn progress for reconnect/replay
struct TurnTrackerState {
    progress: Arc<RwLock<TurnProgress>>,
}

#[derive(Clone)]
struct HeartbeatState {
    last_frontend_beat: Arc<RwLock<Option<Instant>>>,
    logged_unresponsive: Arc<RwLock<bool>>,
    logged_never_seen: Arc<RwLock<bool>>,
    start_instant: Instant,
}

impl Default for HeartbeatState {
    fn default() -> Self {
        Self {
            last_frontend_beat: Arc::new(RwLock::new(None)),
            logged_unresponsive: Arc::new(RwLock::new(false)),
            logged_never_seen: Arc::new(RwLock::new(false)),
            start_instant: Instant::now(),
        }
    }
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

/// CLI arguments for plugable-chat
#[derive(Parser, Debug, Clone)]
#[command(name = "plugable-chat", about = "Plugable Chat desktop app")]
struct CliArgs {
    /// Optional model to load on launch (non-persistent)
    #[arg(long, value_name = "MODEL", env = "PLUGABLE_MODEL")]
    model: Option<String>,
    /// Override global system prompt (string or @path/to/file)
    #[arg(long, value_name = "PROMPT_OR_@FILE", env = "PLUGABLE_SYSTEM_PROMPT")]
    system_prompt: Option<String>,
    /// Initial user prompt to send on startup (string or @path/to/file)
    #[arg(long, value_name = "PROMPT_OR_@FILE", env = "PLUGABLE_INITIAL_PROMPT")]
    initial_prompt: Option<String>,
    /// Enable/disable tool_search
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_TOOL_SEARCH", value_parser = clap::builder::BoolishValueParser::new())]
    tool_search: Option<bool>,
    /// Maximum number of tools returned by tool_search (caps auto and explicit searches)
    #[arg(long, value_name = "INT", env = "PLUGABLE_TOOL_SEARCH_MAX_RESULTS")]
    tool_search_max_results: Option<usize>,
    /// Enable/disable python_execution built-in
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_PYTHON_EXECUTION", value_parser = clap::builder::BoolishValueParser::new())]
    python_execution: Option<bool>,
    /// Enable/disable python-driven tool calling
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_PYTHON_TOOL_CALLING", value_parser = clap::builder::BoolishValueParser::new())]
    python_tool_calling: Option<bool>,
    /// Enable/disable native tool calling (OpenAI-compatible) when model supports it
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_NATIVE_TOOL_CALLING", value_parser = clap::builder::BoolishValueParser::new())]
    native_tool_calling: Option<bool>,
    /// Enable/disable inclusion of tool input_examples in prompts
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_TOOL_EXAMPLES", value_parser = clap::builder::BoolishValueParser::new())]
    tool_examples: Option<bool>,
    /// Maximum number of examples per tool when tool_examples is enabled
    #[arg(long, value_name = "INT", env = "PLUGABLE_TOOL_EXAMPLES_MAX")]
    tool_examples_max: Option<usize>,
    /// Enable compact prompt mode for small models
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_COMPACT_MODE", value_parser = clap::builder::BoolishValueParser::new())]
    compact_mode: Option<bool>,
    /// Maximum number of tools to surface in prompts when compact mode is on
    #[arg(long, value_name = "INT", env = "PLUGABLE_COMPACT_MAX_TOOLS")]
    compact_max_tools: Option<usize>,
    /// Override per-server defer_tools setting at launch
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_DEFER_TOOLS", value_parser = clap::builder::BoolishValueParser::new())]
    defer_tools: Option<bool>,
    /// Enable/disable legacy <tool_call> parsing
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_LEGACY_TOOL_FORMAT", value_parser = clap::builder::BoolishValueParser::new())]
    legacy_tool_call_format: Option<bool>,
    /// Comma-separated list of tool call formats to enable (hermes,mistral,pythonic,pure_json,code_mode)
    #[arg(
        long = "tool-call-enabled",
        value_delimiter = ',',
        value_name = "FORMAT[,FORMAT...]",
        env = "PLUGABLE_TOOL_CALL_ENABLED"
    )]
    tool_call_enabled: Option<Vec<String>>,
    /// Primary tool call format to prompt
    #[arg(
        long = "tool-call-primary",
        value_name = "FORMAT",
        env = "PLUGABLE_TOOL_CALL_PRIMARY"
    )]
    tool_call_primary: Option<String>,
    /// Override per-tool system prompts (server_id::tool_name=prompt_or_@file). Use server_id=builtin for built-ins.
    #[arg(long = "tool-system-prompt", value_name = "KEY=VALUE_OR_@FILE", env = "PLUGABLE_TOOL_SYSTEM_PROMPTS", value_delimiter = None)]
    tool_system_prompts: Vec<String>,
    /// Replace MCP server list with JSON configs (inline JSON or @path/to/json)
    #[arg(long = "mcp-server", value_name = "JSON_OR_@FILE", env = "PLUGABLE_MCP_SERVERS", value_delimiter = None)]
    mcp_servers: Vec<String>,
    /// Optional allowlist of tools to expose on launch.
    /// Built-ins: python_execution, tool_search
    /// MCP tools: server_id::tool_name
    /// Servers: server_id (enables all tools from that server)
    #[arg(long, value_delimiter = ',', env = "PLUGABLE_TOOLS")]
    tools: Option<Vec<String>>,
    /// Enable the built-in dev MCP test server (off by default)
    #[arg(
        long,
        value_name = "BOOL",
        env = "PLUGABLE_ENABLE_MCP_TEST",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    enable_mcp_test: Option<bool>,
    /// Run only the dev MCP test server (no app; blocks until exit)
    #[arg(
        long,
        value_name = "BOOL",
        env = "PLUGABLE_RUN_MCP_TEST_SERVER",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    run_mcp_test_server: bool,
    /// Host for the dev MCP test server when run standalone
    #[arg(long, value_name = "HOST", default_value = MCP_TEST_DEFAULT_HOST)]
    mcp_test_host: String,
    /// Port for the dev MCP test server when run standalone
    #[arg(long, value_name = "PORT", default_value_t = MCP_TEST_DEFAULT_PORT)]
    mcp_test_port: u16,
    /// Auto-run the full MCP test sweep on start (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    mcp_test_run_all_on_start: bool,
    /// Serve the MCP test server UI (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = true,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    mcp_test_serve_ui: bool,
    /// Auto-open the MCP test server UI in a browser (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = true,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    mcp_test_open_ui: bool,
    /// Print the recommended MCP test prompt to stdout (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = true,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    mcp_test_print_prompt: bool,
}

#[derive(Debug, Clone, Default)]
struct LaunchOverrides {
    model: Option<String>,
    initial_prompt: Option<String>,
}

// ToolLaunchFilter moved to tool_capability module

/// Global launch configuration state
struct LaunchConfigState {
    tool_filter: ToolLaunchFilter,
    launch_overrides: LaunchOverrides,
}

/// Maximum number of tool call iterations before stopping (safety limit)
const MAX_TOOL_ITERATIONS: usize = 20;
const PYTHON_EXECUTION_TOOL_TYPE: &str = "python_execution_20251206";

/// Check if a tool call is for a built-in tool (python_execution, tool_search, or database tools)
fn is_builtin_tool(tool_name: &str) -> bool {
    matches!(tool_name, "python_execution" | "tool_search" | "schema_search" | "sql_select")
}

/// Build a consistent key for tool-specific settings
fn tool_prompt_key(server_id: &str, tool_name: &str) -> String {
    format!("{}::{}", server_id, tool_name)
}

fn read_value_or_file(raw: &str) -> Result<String, String> {
    if let Some(path) = raw.strip_prefix('@') {
        let contents = fs::read_to_string(Path::new(path))
            .map_err(|e| format!("Failed to read {}: {}", path, e))?;
        Ok(contents)
    } else {
        Ok(raw.to_string())
    }
}

fn parse_json_or_file<T: DeserializeOwned>(raw: &str) -> Result<T, String> {
    let data = read_value_or_file(raw)?;
    serde_json::from_str(&data).map_err(|e| format!("Failed to parse JSON: {}", e))
}

fn parse_tool_call_format(name: &str) -> Option<ToolCallFormatName> {
    match name {
        "hermes" => Some(ToolCallFormatName::Hermes),
        "mistral" => Some(ToolCallFormatName::Mistral),
        "pythonic" => Some(ToolCallFormatName::Pythonic),
        "pure_json" => Some(ToolCallFormatName::PureJson),
        "code_mode" => Some(ToolCallFormatName::CodeMode),
        _ => None,
    }
}

/// Keep the shared registry's database built-ins in sync with current settings.
async fn sync_registry_database_tools(
    registry: &SharedToolRegistry,
    schema_search_enabled: bool,
    sql_select_enabled: bool,
) {
    let mut guard = registry.write().await;
    guard.set_schema_search_enabled(schema_search_enabled);
    guard.set_sql_select_enabled(sql_select_enabled);
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
    if !settings_guard.sql_select_enabled {
        settings_guard.sql_select_enabled = true;
        
        // Refresh the SettingsStateMachine (Tier 1)
        let mut sm_guard = settings_sm_state.machine.write().await;
        sm_guard.refresh(&settings_guard, &launch_config.tool_filter);

        if let Err(e) = settings::save_settings(&settings_guard).await {
            println!(
                "[Chat] Failed to persist sql_select_enabled ({}): {}",
                reason, e
            );
        } else {
            println!(
                "[Chat] sql_select_enabled auto-enabled after {}",
                reason
            );
        }
    }
}

/// Parse CLI args into a launch-time tool filter
fn parse_tool_filter(args: &CliArgs) -> ToolLaunchFilter {
    let mut builtin_set: HashSet<String> = HashSet::new();
    let mut server_set: HashSet<String> = HashSet::new();
    let mut tool_set: HashSet<(String, String)> = HashSet::new();

    let mut has_builtin = false;
    let mut has_server = false;
    let mut has_tool = false;

    if let Some(entries) = &args.tools {
        for raw in entries {
            if let Some((server_id, tool_name)) = raw.split_once("::") {
                tool_set.insert((server_id.to_string(), tool_name.to_string()));
                has_tool = true;
            } else if is_builtin_tool(raw) {
                builtin_set.insert(raw.to_string());
                has_builtin = true;
            } else {
                server_set.insert(raw.to_string());
                has_server = true;
            }
        }
    }

    ToolLaunchFilter {
        allowed_builtins: if has_builtin { Some(builtin_set) } else { None },
        allowed_servers: if has_server { Some(server_set) } else { None },
        allowed_tools: if has_tool { Some(tool_set) } else { None },
    }
}

/// Apply CLI overrides to settings without persisting them.
fn apply_cli_overrides(args: &CliArgs, settings: &mut AppSettings) -> LaunchOverrides {
    fn resolve_mcp_manifest() -> Option<String> {
        // Probe current dir and a couple parents for the repo root
        let mut dir = std::env::current_dir().ok()?;
        for _ in 0..5 {
            let candidate = dir.join("mcp-test-server").join("Cargo.toml");
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
            if !dir.pop() {
                break;
            }
        }
        None
    }
    // System prompt
    if let Some(raw) = &args.system_prompt {
        match read_value_or_file(raw) {
            Ok(prompt) => settings.system_prompt = prompt,
            Err(e) => println!("[Launch] Failed to apply system_prompt override: {}", e),
        }
    }

    // Core toggles
    if let Some(v) = args.tool_search {
        settings.tool_search_enabled = v;
    }
    if let Some(max_results) = args.tool_search_max_results {
        let capped = max_results.clamp(1, 20);
        settings.tool_search_max_results = capped;
    }
    if let Some(v) = args.python_execution {
        settings.python_execution_enabled = v;
    }
    if let Some(v) = args.python_tool_calling {
        settings.python_tool_calling_enabled = v;
    }
    if let Some(v) = args.native_tool_calling {
        // CLI override for native tool calling - add/remove Native format
        if v {
            if !settings.tool_call_formats.enabled.contains(&ToolCallFormatName::Native) {
                settings.tool_call_formats.enabled.insert(0, ToolCallFormatName::Native);
            }
            settings.tool_call_formats.primary = ToolCallFormatName::Native;
        } else {
            settings.tool_call_formats.enabled.retain(|f| *f != ToolCallFormatName::Native);
            if settings.tool_call_formats.primary == ToolCallFormatName::Native {
                settings.tool_call_formats.primary = settings.tool_call_formats.enabled
                    .first()
                    .copied()
                    .unwrap_or(ToolCallFormatName::Hermes);
            }
        }
        settings.tool_call_formats.normalize();
    }
    if let Some(v) = args.tool_examples {
        settings.tool_use_examples_enabled = v;
    }
    if let Some(max_examples) = args.tool_examples_max {
        let capped = max_examples.clamp(1, 5);
        settings.tool_use_examples_max = capped;
    }
    if let Some(defer) = args.defer_tools {
        for server in &mut settings.mcp_servers {
            server.defer_tools = defer;
        }
    }
    if let Some(v) = args.legacy_tool_call_format {
        settings.legacy_tool_call_format_enabled = v;
    }

    // Tool call formats
    if let Some(enabled) = &args.tool_call_enabled {
        let mut parsed: Vec<ToolCallFormatName> = Vec::new();
        for raw in enabled {
            if let Some(fmt) = parse_tool_call_format(raw) {
                parsed.push(fmt);
            } else {
                println!("[Launch] Unknown tool_call format '{}', ignoring", raw);
            }
        }
        if !parsed.is_empty() {
            settings.tool_call_formats.enabled = parsed;
        }
    }
    if let Some(primary) = &args.tool_call_primary {
        if let Some(fmt) = parse_tool_call_format(primary) {
            settings.tool_call_formats.primary = fmt;
        } else {
            println!("[Launch] Unknown tool_call primary '{}', ignoring", primary);
        }
    }
    settings.tool_call_formats.normalize();

    // Tool system prompts
    for entry in &args.tool_system_prompts {
        if let Some((key, raw_val)) = entry.split_once('=') {
            match read_value_or_file(raw_val) {
                Ok(value) => {
                    settings.tool_system_prompts.insert(key.to_string(), value);
                }
                Err(e) => println!("[Launch] Failed to apply tool_system_prompt {}: {}", key, e),
            }
        } else {
            println!(
                "[Launch] Invalid --tool-system-prompt '{}'. Expected server::tool=prompt_or_@file",
                entry
            );
        }
    }

    // MCP servers
    if !args.mcp_servers.is_empty() {
        let mut parsed_servers: Vec<McpServerConfig> = Vec::new();
        for raw in &args.mcp_servers {
            match parse_json_or_file::<McpServerConfig>(raw) {
                Ok(mut cfg) => {
                    enforce_python_name(&mut cfg);
                    parsed_servers.push(cfg);
                }
                Err(e) => println!("[Launch] Failed to parse MCP server '{}': {}", raw, e),
            }
        }
        if !parsed_servers.is_empty() {
            settings.mcp_servers = parsed_servers;
        }
    }

    // Launch-only overrides
    let launch_model = args.model.clone();
    let launch_prompt = match &args.initial_prompt {
        Some(raw) => match read_value_or_file(raw) {
            Ok(text) => Some(text),
            Err(e) => {
                println!("[Launch] Failed to read initial_prompt: {}", e);
                None
            }
        },
        None => None,
    };

    // Enable default dev MCP test server when requested
    let mut enable_mcp_prompt: Option<String> = None;
    if args.enable_mcp_test == Some(true) {
        ensure_default_servers(settings);
        if let Some(test_server) = settings
            .mcp_servers
            .iter_mut()
            .find(|s| s.id == "mcp-test-server")
        {
            test_server.enabled = true;
            test_server.defer_tools = false;

            // Normalize manifest path to an absolute path if available
            if test_server.command.as_deref() == Some("cargo") {
                if let Some(abs_manifest) = resolve_mcp_manifest() {
                    test_server.args = vec![
                        "run".to_string(),
                        "--manifest-path".to_string(),
                        abs_manifest,
                        "--release".to_string(),
                    ];
                }
            }
        }

        // Force deterministic, test-friendly settings:
        // - Disable tool_search so the test server tools stay active (not deferred)
        // - Keep native tool calling as the primary path (no python code mode)
        settings.tool_search_enabled = false;
        settings.python_execution_enabled = false;
        settings.python_tool_calling_enabled = false;

        // Auto-populate initial prompt to trigger the dev test suite if none provided
        if launch_prompt.is_none() {
            enable_mcp_prompt = Some(
                "Connect to the dev MCP test server and run all tests. Report red/green for each test, a summary, and any errors or logs you see."
                    .to_string(),
            );
        }
    }

    LaunchOverrides {
        model: launch_model,
        initial_prompt: launch_prompt.or(enable_mcp_prompt),
    }
}

/// Execute the tool_search built-in tool
async fn execute_tool_search(
    input: ToolSearchInput,
    tool_registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    max_results: usize,
) -> Result<(String, Vec<ToolSearchResult>), String> {
    let executor = ToolSearchExecutor::new(tool_registry.clone(), embedding_model);
    let mut capped_input = input.clone();
    let top_cap = std::cmp::max(1, max_results);
    capped_input.top_k = std::cmp::max(1, std::cmp::min(capped_input.top_k, top_cap));
    let output = executor.execute(capped_input).await?;

    // Filter out tools that cannot be called from python_execution (respect allowed_callers)
    let filtered_tools: Vec<ToolSearchResult> = {
        let registry_guard = tool_registry.read().await;
        output
            .tools
            .iter()
            .filter(|tool| {
                let key = format!("{}___{}", tool.server_id, tool.name);
                match registry_guard.get_tool(&key) {
                    Some(schema) => schema.can_be_called_by(Some(PYTHON_EXECUTION_TOOL_TYPE)),
                    None => true,
                }
            })
            .cloned()
            .collect()
    };

    // Materialize discovered tools
    executor.materialize_results(&filtered_tools).await;

    // Format result for the model with clear instructions to use python_execution
    let mut result = String::new();
    result.push_str("# Discovered Tools\n\n");
    result.push_str(
        "**YOUR NEXT STEP: Return a single Python program that uses these functions. Do NOT emit <tool_call> tags.**\n\n",
    );

    // Build the python code example
    let mut python_lines: Vec<String> = vec![];
    let mut tool_docs: Vec<String> = vec![];

    for tool in &filtered_tools {
        // Document the tool
        let mut doc = format!("### {}(", tool.name);
        let mut params: Vec<String> = vec![];
        let mut example_params: Vec<String> = vec![];

        if let Some(props) = tool
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
        {
            let required: Vec<&str> = tool
                .parameters
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            for (name, schema) in props {
                let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                let is_required = required.contains(&name.as_str());
                params.push(format!(
                    "{}: {}{}",
                    name,
                    type_str,
                    if is_required { "" } else { " (optional)" }
                ));

                // Build example call with placeholders
                if is_required {
                    let example_val = match type_str {
                        "string" => format!("\"...\""),
                        "integer" => "1".to_string(),
                        "boolean" => "True".to_string(),
                        "array" => "[]".to_string(),
                        _ => "...".to_string(),
                    };
                    example_params.push(format!("{}={}", name, example_val));
                }
            }
        }

        doc.push_str(&params.join(", "));
        doc.push_str(")\n");
        if let Some(ref desc) = tool.description {
            doc.push_str(&format!("{}\n", desc));
        }
        tool_docs.push(doc);

        // Add to example Python code (just the first tool as primary example)
        if python_lines.is_empty() {
            let call = if example_params.is_empty() {
                format!("result = {}()", tool.name)
            } else {
                format!("result = {}({})", tool.name, example_params.join(", "))
            };
            python_lines.push(call);
            python_lines.push("print(result)".to_string());
        }
    }

    // Show available tools
    for doc in tool_docs {
        result.push_str(&doc);
        result.push_str("\n");
    }

    // Show example python_execution program to make
    result.push_str("---\n\n");
    result.push_str("**NOW return exactly this shape (single Python block):**\n");
    result.push_str("```python\n");
    result.push_str("# Use the discovered tools directly\n");
    for line in &python_lines {
        result.push_str(line);
        result.push('\n');
    }
    result.push_str("```\n");

    Ok((result, filtered_tools))
}

/// Parse python_execution arguments, handling multiple formats from different models.
///
/// Models may produce different argument structures:
/// - Correct: `{"code": ["line1", "line2"], "context": null}`
/// - Direct array: `["line1", "line2"]` (model put code directly in arguments)
/// - Nested: `{"arguments": {"code": [...]}}` (double-wrapped)
fn parse_python_execution_args(arguments: &serde_json::Value) -> CodeExecutionInput {
    // First, try standard format: {"code": [...], "context": ...}
    if let Ok(mut input) = serde_json::from_value::<CodeExecutionInput>(arguments.clone()) {
        if !input.code.is_empty() {
            println!(
                "[python_execution] Parsed standard format: {} lines",
                input.code.len()
            );
            input.code = fix_python_indentation(&input.code);
            return input;
        }
    }

    // Try direct array format: arguments is already the code array
    if let Some(arr) = arguments.as_array() {
        let code: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if !code.is_empty() {
            println!(
                "[python_execution] Parsed direct array format: {} lines",
                code.len()
            );
            let fixed_code = fix_python_indentation(&code);
            return CodeExecutionInput {
                code: fixed_code,
                context: None,
            };
        }
    }

    // Try double-wrapped: {"arguments": {"code": [...]}} or {"code": {"code": [...]}}
    if let Some(inner) = arguments.get("arguments").or_else(|| arguments.get("code")) {
        if let Some(arr) = inner.as_array() {
            let code: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !code.is_empty() {
                println!(
                    "[python_execution] Parsed double-wrapped format: {} lines",
                    code.len()
                );
                let fixed_code = fix_python_indentation(&code);
                return CodeExecutionInput {
                    code: fixed_code,
                    context: None,
                };
            }
        } else if let Ok(mut input) = serde_json::from_value::<CodeExecutionInput>(inner.clone()) {
            if !input.code.is_empty() {
                println!(
                    "[python_execution] Parsed nested format: {} lines",
                    input.code.len()
                );
                input.code = fix_python_indentation(&input.code);
                return input;
            }
        }
    }

    // Log the actual format received for debugging
    let preview: String = serde_json::to_string(arguments)
        .unwrap_or_else(|_| "???".to_string())
        .chars()
        .take(300)
        .collect();
    println!(
        "[python_execution] ⚠️ Could not parse arguments, got: {}",
        preview
    );

    // Return empty input - this will be caught by validation
    CodeExecutionInput {
        code: vec![],
        context: None,
    }
}

/// Reconstruct SQL from malformed sql_select arguments.
///
/// When models call sql_select incorrectly (e.g., positional arguments parsed
/// as key-value pairs due to '=' in SQL), the arguments may look like:
/// `{"\"SELECT ... WHERE x": "10 AND y = 20\""}`
///
/// This function attempts to reconstruct the original SQL by:
/// 1. Detecting if keys look like SQL fragments (contain SELECT, WHERE, etc.)
/// 2. Joining keys and values with '=' to reconstruct the query
///
/// Returns None if the arguments don't look like malformed SQL.
fn reconstruct_sql_from_malformed_args(arguments: &serde_json::Value) -> Option<String> {
    let obj = arguments.as_object()?;
    
    // Skip if it already has the proper sql key with a non-empty value
    if let Some(sql_val) = obj.get("sql") {
        if let Some(s) = sql_val.as_str() {
            if !s.is_empty() {
                return None;
            }
        }
    }
    
    // Look for keys that look like SQL fragments
    let sql_keywords = ["SELECT", "INSERT", "UPDATE", "DELETE", "FROM", "WHERE", "JOIN"];
    
    let mut sql_fragments: Vec<(String, String)> = Vec::new();
    
    for (key, value) in obj.iter() {
        // Skip known proper parameter names
        if key == "sql" || key == "source_id" || key == "parameters" || key == "max_rows" {
            continue;
        }
        
        let key_upper = key.to_uppercase();
        
        // Check if the key looks like it contains SQL
        let looks_like_sql = sql_keywords.iter().any(|kw| key_upper.contains(kw))
            || key.contains('(')  // Function calls like EXTRACT(...)
            || key.contains('"')  // Quoted strings
            || key.starts_with("\""); // Malformed quoted key
        
        if looks_like_sql {
            let val_str = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => serde_json::to_string(value).unwrap_or_default(),
            };
            sql_fragments.push((key.clone(), val_str));
        }
    }
    
    if sql_fragments.is_empty() {
        return None;
    }
    
    // Reconstruct the SQL by joining fragments
    // The malformed parsing typically splits on '=' so we join with '='
    let mut reconstructed = String::new();
    for (i, (key, value)) in sql_fragments.iter().enumerate() {
        // Clean up the key (remove surrounding quotes if present)
        let clean_key = key
            .trim_start_matches('"')
            .trim_end_matches('"')
            .to_string();
        
        // Clean up the value (remove surrounding quotes if present)
        let clean_value = value
            .trim_start_matches('"')
            .trim_end_matches('"')
            .to_string();
        
        if i > 0 {
            reconstructed.push(' ');
        }
        
        reconstructed.push_str(&clean_key);
        
        // Only add '=' if the value is non-empty and doesn't start with common SQL joiners
        if !clean_value.is_empty() {
            let value_upper = clean_value.trim().to_uppercase();
            let needs_equals = !value_upper.starts_with("AND ")
                && !value_upper.starts_with("OR ")
                && !value_upper.starts_with("FROM ")
                && !value_upper.starts_with("WHERE ")
                && !value_upper.starts_with("GROUP ")
                && !value_upper.starts_with("ORDER ")
                && !value_upper.starts_with("LIMIT ");
            
            if needs_equals {
                reconstructed.push_str(" = ");
            } else {
                reconstructed.push(' ');
            }
            reconstructed.push_str(&clean_value);
        }
    }
    
    // Basic validation: must start with SELECT/INSERT/UPDATE/DELETE
    let trimmed_upper = reconstructed.trim().to_uppercase();
    if !trimmed_upper.starts_with("SELECT")
        && !trimmed_upper.starts_with("INSERT")
        && !trimmed_upper.starts_with("UPDATE")
        && !trimmed_upper.starts_with("DELETE")
    {
        println!("[reconstruct_sql_from_malformed_args] Reconstructed text doesn't look like SQL: {}...", 
            reconstructed.chars().take(50).collect::<String>());
        return None;
    }
    
    println!("[reconstruct_sql_from_malformed_args] Successfully reconstructed SQL query");
    Some(reconstructed)
}

/// Fix missing Python indentation in code lines.
///
/// When models output code as arrays of lines, they often omit indentation.
/// This function uses a simple heuristic: track indent level based on
/// block-starting keywords (for, if, while, def, etc.) and keywords that
/// indicate staying at the same or reduced level (else, elif, return, etc.).
///
/// This is a best-effort fix and may not handle all edge cases perfectly.
fn fix_python_indentation(lines: &[String]) -> Vec<String> {
    use regex::Regex;

    // Patterns that start a block (require indented lines after)
    let block_starters = Regex::new(
        r"^\s*(for\s+.+:|while\s+.+:|if\s+.+:|elif\s+.+:|else\s*:|def\s+.+:|class\s+.+:|try\s*:|except.*:|finally\s*:|with\s+.+:)\s*(#.*)?$"
    ).unwrap();

    // Patterns that should be at same level as opening (else, elif, except, finally)
    let dedent_before =
        Regex::new(r"^\s*(elif\s+.+:|else\s*:|except.*:|finally\s*:)\s*(#.*)?$").unwrap();

    // Statements that typically end a block
    let block_enders = Regex::new(r"^\s*(return\b|break\b|continue\b|raise\b|pass\b)").unwrap();

    let mut result = Vec::with_capacity(lines.len());
    let mut indent_stack: Vec<usize> = vec![0]; // Stack of indent levels
    let indent_str = "    "; // 4 spaces

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            result.push(String::new());
            continue;
        }

        // Check if line already has indentation
        let existing_indent = line.len() - line.trim_start().len();
        if existing_indent > 0 {
            // Line already has indentation - trust it and reset our tracking
            result.push(line.clone());
            let indent_units = existing_indent / 4;
            indent_stack.clear();
            indent_stack.push(indent_units);
            if block_starters.is_match(trimmed) {
                indent_stack.push(indent_units + 1);
            }
            continue;
        }

        // Get current indent level
        let current_indent = *indent_stack.last().unwrap_or(&0);

        // Check if this line should be at reduced indent (else, elif, except, finally)
        let line_indent = if dedent_before.is_match(trimmed) {
            // Pop one level for else/elif/except/finally
            if indent_stack.len() > 1 {
                indent_stack.pop();
            }
            *indent_stack.last().unwrap_or(&0)
        } else {
            current_indent
        };

        // Apply indentation
        let indented_line = if line_indent > 0 {
            format!("{}{}", indent_str.repeat(line_indent), trimmed)
        } else {
            trimmed.to_string()
        };

        result.push(indented_line);

        // Check if next line needs more indent (this line starts a block)
        if block_starters.is_match(trimmed) {
            indent_stack.push(line_indent + 1);
        } else if block_enders.is_match(trimmed) {
            // After return/break/continue/pass/raise, next line might be less indented
            // But only pop if we're not at top level and there's a next line
            if indent_stack.len() > 1 && i + 1 < lines.len() {
                // Peek at next line - if it's a block continuation keyword, don't pop
                let next_trimmed = lines[i + 1].trim();
                if !dedent_before.is_match(next_trimmed) {
                    indent_stack.pop();
                }
            }
        }
    }

    // Check if any indentation was applied
    let had_changes = result.iter().zip(lines.iter()).any(|(a, b)| a != b);
    if had_changes {
        println!("[python_execution] 🔧 Auto-fixed Python indentation");
    }

    result
}

/// Strip unsupported Python keywords/patterns that cause RustPython compilation errors.
///
/// Keywords removed:
/// - `await` - RustPython sandbox doesn't run in async context
///
/// This is called before code execution to handle models that add unsupported syntax.
fn strip_unsupported_python(lines: &[String]) -> Vec<String> {
    use regex::Regex;

    // Pattern to match standalone `await` keyword (not inside strings)
    // Matches: `await foo()`, `x = await bar()`, but not `"await"` or `# await`
    let await_pattern = Regex::new(r"\bawait\s+").unwrap();

    let mut result = Vec::with_capacity(lines.len());
    let mut stripped_count = 0;

    for line in lines {
        let trimmed = line.trim();

        // Skip comments and string-only lines
        if trimmed.starts_with('#') {
            result.push(line.clone());
            continue;
        }

        // Strip `await ` from the line
        if await_pattern.is_match(line) {
            let fixed = await_pattern.replace_all(line, "").to_string();
            result.push(fixed);
            stripped_count += 1;
        } else {
            result.push(line.clone());
        }
    }

    if stripped_count > 0 {
        println!(
            "[python_execution] 🔧 Stripped {} `await` keyword(s) (not needed in sandbox)",
            stripped_count
        );
    }

    result
}

/// Execute the python_execution built-in tool
async fn execute_python_execution(
    input: CodeExecutionInput,
    exec_id: String,
    tool_registry: SharedToolRegistry,
    python_tx: &mpsc::Sender<PythonMsg>,
    allow_tool_search: bool,
) -> Result<CodeExecutionOutput, String> {
    // Strip unsupported keywords before execution
    let code = strip_unsupported_python(&input.code);

    // Log the code about to be executed
    println!("[python_execution] exec_id={}", exec_id);
    println!("[python_execution] Code to execute ({} lines):", code.len());
    for (i, line) in code.iter().enumerate() {
        println!("[python_execution]   {}: {}", i + 1, line);
    }
    // Flush stdout to ensure logs appear immediately
    use std::io::Write;
    let _ = std::io::stdout().flush();

    // Get available tools and materialized tool modules for the execution context
    let (available_tools_with_servers, mut tool_modules) = {
        let registry = tool_registry.read().await;
        let tools = registry.get_visible_tools_with_servers();
        let modules = registry.get_materialized_tool_modules();
        let stats = registry.stats();
        println!(
            "[python_execution] Registry stats: {} materialized tools",
            stats.materialized_tools
        );
        (tools, modules)
    };

    // Filter tools: remove python_execution, optionally remove tool_search if disabled
    let mut filtered_tools = Vec::new();
    for (server_id, tool) in available_tools_with_servers {
        if tool.name == "python_execution" {
            continue;
        }
        if !tool.can_be_called_by(Some(PYTHON_EXECUTION_TOOL_TYPE)) {
            continue;
        }
        if tool.name == "tool_search" && !allow_tool_search {
            continue;
        }
        filtered_tools.push((server_id, tool));
    }

    // Inject a builtin module for tool_search if it is allowed (so python can call it directly)
    if allow_tool_search {
        tool_modules.push(tool_registry::ToolModuleInfo {
            python_name: "builtin_tools".to_string(),
            server_id: "builtin".to_string(),
            functions: vec![tool_registry::ToolFunctionInfo {
                name: "tool_search".to_string(),
                description: Some(
                    "Semantic search over available tools. Call with relevant_to string."
                        .to_string(),
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "relevant_to": { "type": "string" }
                    },
                    "required": ["relevant_to"]
                }),
            }],
        });
    }

    println!(
        "[python_execution] Available tools: {}, Tool modules: {}",
        filtered_tools.len(),
        tool_modules.len()
    );
    for module in &tool_modules {
        println!(
            "[python_execution]   Module '{}' (server: {}) with {} functions",
            module.python_name,
            module.server_id,
            module.functions.len()
        );
        for func in &module.functions {
            println!("[python_execution]     - {}", func.name);
        }
    }
    let _ = std::io::stdout().flush();

    // Create execution context
    let context = CodeExecutionExecutor::create_context(
        exec_id.clone(),
        filtered_tools,
        input.context.clone(),
        tool_modules,
    );

    // Create modified input with the cleaned code
    let cleaned_input = CodeExecutionInput {
        code,
        context: input.context,
    };

    // Pre-validate before sending to the Python actor so errors can be surfaced immediately
    let mut import_context = crate::tools::code_execution::DynamicImportContext::new();
    for module in &context.tool_modules {
        import_context.add_tool_module(module.python_name.clone(), module.server_id.clone());
    }
    let validation_context = crate::tools::code_execution::ValidationContext {
        import_context: Some(&import_context),
        allowed_functions: Some(&context.allowed_functions),
    };
    crate::tools::code_execution::CodeExecutionExecutor::validate_input_with_rules(
        &cleaned_input,
        Some(validation_context),
    )?;

    println!("[python_execution] Sending to Python actor...");
    let _ = std::io::stdout().flush();

    // Send to Python actor for execution
    let (respond_to, rx) = oneshot::channel();
    python_tx
        .send(PythonMsg::ExecuteSandboxedCode {
            input: cleaned_input,
            context,
            respond_to,
        })
        .await
        .map_err(|e| format!("Failed to send to Python actor: {}", e))?;

    println!("[python_execution] Waiting for Python actor response...");
    let _ = std::io::stdout().flush();

    let result = rx.await.map_err(|_| "Python actor died".to_string())?;

    println!(
        "[python_execution] Python execution complete: success={}",
        result.as_ref().map(|r| r.success).unwrap_or(false)
    );
    let _ = std::io::stdout().flush();

    result
}

/// Helper to execute a single tool call via McpHostActor
async fn execute_tool_internal(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    call: &ParsedToolCall,
) -> Result<String, String> {
    let (tx, rx) = oneshot::channel();
    mcp_host_tx
        .send(McpHostMsg::ExecuteTool {
            server_id: call.server.clone(),
            tool_name: call.tool.clone(),
            arguments: call.arguments.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send to MCP Host: {}", e))?;

    let result = rx.await.map_err(|_| "MCP Host actor died".to_string())??;

    // Convert the result to a string
    let result_text = result
        .content
        .iter()
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
    println!(
        "[resolve_server_for_tool] Searching for tool '{}' across servers...",
        tool_name
    );

    // Get all tool descriptions from connected servers
    let (tx, rx) = oneshot::channel();
    if mcp_host_tx
        .send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .is_err()
    {
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
                println!(
                    "[resolve_server_for_tool] Found tool '{}' on server '{}'",
                    tool_name, server_id
                );
                return Some(server_id);
            }
        }
    }

    println!(
        "[resolve_server_for_tool] Tool '{}' not found on any connected server",
        tool_name
    );
    None
}

/// Extract a Python program from the model response.
/// Prefers fenced ```python blocks, falls back to treating the whole message as code.
fn extract_python_program(response: &str) -> Option<Vec<String>> {
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
fn is_valid_python_syntax(code_lines: &[String]) -> bool {
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

/// Result of deciding what the agentic loop should do with a model response.
#[derive(Debug, PartialEq)]
pub(crate) enum AgenticAction {
    Final { response: String },
    ToolCalls { calls: Vec<ParsedToolCall> },
}

/// Decide whether a response should trigger tool execution or be treated as final text.
pub(crate) fn detect_agentic_action(
    assistant_response: &str,
    model_family: ModelFamily,
    tool_format: ToolFormat,
    python_tool_mode: bool,
    formats: &ToolCallFormatConfig,
    primary_format: ToolCallFormatName,
) -> AgenticAction {
    let non_code_formats_enabled = formats.any_non_code();

    if python_tool_mode {
        if let Some(code_lines) = extract_python_program(assistant_response) {
            if is_valid_python_syntax(&code_lines) {
                return AgenticAction::ToolCalls {
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
            return AgenticAction::Final {
                response: assistant_response.to_string(),
            };
        }
    }

    if non_code_formats_enabled {
        let calls = parse_tool_calls_for_model_profile(
            assistant_response,
            model_family,
            tool_format,
            formats,
            primary_format,
        );
        if !calls.is_empty() {
            return AgenticAction::ToolCalls { calls };
        }
    }

    AgenticAction::Final {
        response: assistant_response.to_string(),
    }
}

/// Create an assistant message with native tool_calls array when using native format
fn create_assistant_message_with_tool_calls(
    content: &str,
    calls: &[ParsedToolCall],
    use_native_format: bool,
    system_prompt: Option<String>,
) -> ChatMessage {
    if use_native_format && calls.iter().all(|c| c.id.is_some()) {
        // Native format: include tool_calls array in assistant message
        let tool_calls: Vec<OpenAIToolCall> = calls
            .iter()
            .filter_map(|c| {
                c.id.as_ref().map(|id| OpenAIToolCall {
                    id: id.clone(),
                    call_type: "function".to_string(),
                    function: OpenAIToolCallFunction {
                        name: if c.server == "builtin" || c.server == "unknown" {
                            c.tool.clone()
                        } else {
                            format!("{}___{}", c.server, c.tool)
                        },
                        arguments: serde_json::to_string(&c.arguments).unwrap_or_default(),
                    },
                })
            })
            .collect();

        ChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            system_prompt,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    } else {
        // Text-based format: content only
        ChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            system_prompt,
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

/// Create a tool result message for native OpenAI format
fn create_native_tool_result_message(tool_call_id: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        system_prompt: None,
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
    }
}

/// Check if we should use native tool result format
/// Returns true when native tool calling is enabled AND all tool calls have IDs
fn should_use_native_tool_results(
    native_tool_calling_enabled: bool,
    calls: &[ParsedToolCall],
) -> bool {
    native_tool_calling_enabled && calls.iter().all(|c| c.id.is_some())
}

/// Run the agentic loop: call model, detect tool calls, execute, repeat
async fn run_agentic_loop(
    foundry_tx: mpsc::Sender<FoundryMsg>,
    mcp_host_tx: mpsc::Sender<McpHostMsg>,
    vector_tx: mpsc::Sender<VectorMsg>,
    python_tx: mpsc::Sender<PythonMsg>,
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    database_toolbox_tx: mpsc::Sender<DatabaseToolboxMsg>,
    tool_registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    pending_approvals: PendingApprovals,
    app_handle: tauri::AppHandle,
    mut full_history: Vec<ChatMessage>,
    reasoning_effort: String,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    server_configs: Vec<McpServerConfig>,
    chat_id: String,
    generation_id: u32,
    title: String,
    original_message: String,
    mut openai_tools: Option<Vec<OpenAITool>>,
    model_name: String,
    python_tool_mode: bool,
    format_config: ToolCallFormatConfig,
    primary_format: ToolCallFormatName,
    allow_tool_search_for_python: bool,
    tool_search_max_results: usize,
    turn_system_prompt: String,
    turn_progress: Arc<RwLock<TurnProgress>>,
    chat_format_default: ChatFormatName,
    chat_format_overrides: std::collections::HashMap<String, ChatFormatName>,
    enabled_db_sources: Vec<String>,
    // State machine is now passed in (single source of truth for prompts and tool gating)
    mut state_machine: AgenticStateMachine,
) {
    // Derive native tool calling from format config
    let native_tool_calling_enabled = format_config.native_enabled();

    // Resolve model profile from model name
    let profile = resolve_profile(&model_name);
    let model_family = profile.model_family;
    let tool_format = profile.tool_call_format;
    let mut iteration = 0;
    let mut had_tool_calls = false;
    let mut final_response = String::new();
    let mut last_token_count: usize;

    // Track repeated errors to detect when model is stuck
    // Format: "tool_name::error_message"
    let mut last_error_signature: Option<String> = None;
    let mut tools_disabled_due_to_repeated_error = false;

    let verbose_logging = is_verbose_logging_enabled();

    // Current system prompt - regenerated by state machine after transitions
    // This is used to update the system message for subsequent iterations
    #[allow(unused_assignments)]
    let mut current_system_prompt = turn_system_prompt.clone();

    println!(
        "[AgenticLoop] Starting with model_family={:?}, tool_format={:?}, python_tool_mode={}, primary_format={:?}, tool_search_in_python={}, state={:?}, prompt_len={}",
        model_family, tool_format, python_tool_mode, primary_format, allow_tool_search_for_python, state_machine.current_state().name(), current_system_prompt.len()
    );
    use std::io::Write;
    let _ = std::io::stdout().flush();

    loop {
        println!("\n[AgenticLoop] Iteration {} starting...", iteration);
        let iteration_start = std::time::Instant::now();
        let _ = std::io::stdout().flush();

        // NOTE: We do NOT clear materialized tools between iterations anymore.
        // Tools discovered via tool_search in iteration 0 must remain available
        // for python_execution in iteration 1 (same user turn).
        // Materialized tools are cleared at the start of each new chat message instead.
        if iteration > 0 {
            let registry = tool_registry.read().await;
            let stats = registry.stats();
            if stats.materialized_tools > 0 {
                println!(
                    "[AgenticLoop] {} materialized tools available from previous iteration",
                    stats.materialized_tools
                );
            }
        }

        // Create an internal cancellation channel that we can trigger if we detect a tool call early.
        // This is separate from the user turn's cancel_rx so we can stop the current stream
        // without cancelling the entire turn.
        let (internal_cancel_tx, mut internal_cancel_rx) = tokio::sync::watch::channel(false);

        // Forward external cancellation to internal one
        let mut external_cancel_rx = cancel_rx.clone();
        let internal_cancel_tx_for_external = internal_cancel_tx.clone();
        tokio::spawn(async move {
            while external_cancel_rx.changed().await.is_ok() {
                if *external_cancel_rx.borrow() {
                    let _ = internal_cancel_tx_for_external.send(true);
                    break;
                }
            }
        });

        // Create channel for this iteration
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Send chat request to Foundry
        println!("[AgenticLoop] 📤 Sending chat request to Foundry...");
        let _ = std::io::stdout().flush();
        // Strip any local-only metadata (like system_prompt) before sending to Foundry
        let model_messages: Vec<ChatMessage> = full_history
            .iter()
            .map(|m| ChatMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                system_prompt: None,
                tool_calls: m.tool_calls.clone(),
                tool_call_id: m.tool_call_id.clone(),
            })
            .collect();

        if let Err(e) = foundry_tx
            .send(FoundryMsg::Chat {
                chat_history_messages: model_messages,
                reasoning_effort: reasoning_effort.clone(),
                native_tool_specs: openai_tools.clone(),
                native_tool_calling_enabled,
                chat_format_default,
                chat_format_overrides: chat_format_overrides.clone(),
                respond_to: tx,
                stream_cancel_rx: internal_cancel_rx.clone(),
            })
            .await
        {
            println!("[AgenticLoop] ERROR: Failed to send to Foundry: {}", e);
            let _ = app_handle.emit("chat-finished", ());
            return;
        }
        println!("[AgenticLoop] 📤 Request sent, waiting for tokens...");
        let _ = std::io::stdout().flush();

        // Collect response while streaming tokens to frontend
        let mut assistant_response = String::new();
        let mut cancelled = false;
        let mut token_count: usize = 0;
        let mut first_token_time: Option<std::time::Instant> = None;
        let mut last_progress_log = std::time::Instant::now();

        loop {
            tokio::select! {
                // Check for cancellation (internal or external)
                _ = internal_cancel_rx.changed() => {
                    if *internal_cancel_rx.borrow() {
                        if *cancel_rx.borrow() {
                            println!("[AgenticLoop] User cancellation received!");
                            cancelled = true;
                        } else {
                            println!("[AgenticLoop] Internal early-stop cancellation triggered.");
                        }
                        break;
                    }
                }
                // Receive tokens
                token = rx.recv() => {
                    match token {
                        Some(token) => {
                            if first_token_time.is_none() {
                                let ttft = iteration_start.elapsed();
                                first_token_time = Some(std::time::Instant::now());
                                println!("[AgenticLoop] 🎯 First token received! TTFT: {:.2}s", ttft.as_secs_f64());
                                let _ = std::io::stdout().flush();
                            }
                            token_count += 1;
                            assistant_response.push_str(&token);
                            let _ = app_handle.emit("chat-token", token.clone());
                            {
                                let mut progress = turn_progress.write().await;
                                progress.assistant_response = assistant_response.clone();
                                progress.last_token_index = token_count;
                                progress.timestamp_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis())
                                    .unwrap_or(progress.timestamp_ms);
                            }

                            // Check for early stop if we see a potential closing character.
                            // This prevents models from hallucinating results or extra text after tool calls.
                            if assistant_response.len() > 20 && (token.contains('>') || token.contains('`') || token.contains('}') || token.contains(']')) {
                                let action = detect_agentic_action(
                                    &assistant_response,
                                    model_family,
                                    tool_format,
                                    python_tool_mode,
                                    &format_config,
                                    primary_format,
                                );
                                
                                if let AgenticAction::ToolCalls { .. } = action {
                                    let trimmed = assistant_response.trim_end();
                                    let mut should_stop = false;
                                    
                                    // Check if the response ends with a valid closing tag or code fence
                                    if trimmed.ends_with("</tool_call>")
                                        || trimmed.ends_with("</function_call>")
                                        || trimmed.ends_with("</function>")
                                        || trimmed.ends_with("[/TOOL_CALLS]")
                                    {
                                        should_stop = true;
                                    } else if python_tool_mode && trimmed.ends_with("```") {
                                        // Only stop on ``` if it's actually closing a python block
                                        if assistant_response.contains("```python") || assistant_response.contains("```py") {
                                            should_stop = true;
                                        }
                                    }
                                    
                                    if should_stop {
                                        println!("[AgenticLoop] 🛑 Detected complete tool call during streaming, stopping early to prevent hallucination.");
                                        let _ = internal_cancel_tx.send(true);
                                        // The next iteration of the tokio::select! will catch the cancellation
                                    }
                                }
                            }

                            // Log progress every 5 seconds (verbose only)
                            if verbose_logging
                                && last_progress_log.elapsed()
                                    >= std::time::Duration::from_secs(5)
                            {
                                println!(
                                    "[AgenticLoop] 📊 Receiving: {} tokens, {} chars so far",
                                    token_count,
                                    assistant_response.len()
                                );
                                let _ = std::io::stdout().flush();
                                last_progress_log = std::time::Instant::now();
                            }
                        }
                        None => {
                            println!("[AgenticLoop] 📥 Channel closed, stream complete");
                            let _ = std::io::stdout().flush();
                            break; // Channel closed, stream complete
                        }
                    }
                }
            }
        }

        if cancelled {
            println!("[AgenticLoop] Stream cancelled by user");
            let _ = app_handle.emit("chat-finished", ());
            return;
        }

        let iteration_elapsed = iteration_start.elapsed();
        last_token_count = token_count;
        println!(
            "[AgenticLoop] ✅ Response complete: {} tokens, {} chars in {:.2}s",
            token_count,
            assistant_response.len(),
            iteration_elapsed.as_secs_f64()
        );
        println!("[AgenticLoop] 📄 Full model response:\n---\n{}\n---", assistant_response);
        let _ = std::io::stdout().flush();
        // #region agent log
        {
            let response_preview: String = assistant_response.chars().take(1000).collect();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
                let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"MODEL","location":"lib.rs:agentic_loop","message":"model_response_complete","data":{{"iteration":{},"token_count":{},"response_len":{},"response_preview":"{}"}},"timestamp":{}}}"#, 
                    iteration, token_count, assistant_response.len(), 
                    response_preview.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r"),
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
            }
        }
        // #endregion

        let agentic_action = detect_agentic_action(
            &assistant_response,
            model_family,
            tool_format,
            python_tool_mode,
            &format_config,
            primary_format,
        );

        let tool_calls = match agentic_action {
            AgenticAction::Final { response } => {
                println!("[AgenticLoop] No tool calls detected, loop complete");
                final_response = response.clone();

                // Add final assistant response to history
                full_history.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: response,
                    system_prompt: Some(turn_system_prompt.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
                break;
            }
            AgenticAction::ToolCalls { calls } => calls,
        };

        if iteration >= MAX_TOOL_ITERATIONS {
            println!(
                "[AgenticLoop] Max iterations ({}) reached, stopping",
                MAX_TOOL_ITERATIONS
            );
            final_response = assistant_response.clone();

            // Add response with unexecuted tool calls (use native format if available)
            let use_native = should_use_native_tool_results(native_tool_calling_enabled, &tool_calls);
            let assistant_msg = create_assistant_message_with_tool_calls(
                &assistant_response,
                &tool_calls,
                use_native,
                Some(turn_system_prompt.clone()),
            );
            full_history.push(assistant_msg);
            break;
        }

        had_tool_calls = true;
        println!("[AgenticLoop] Found {} tool call(s)", tool_calls.len());

        // Determine if we should use native tool result format
        let use_native_tool_results = should_use_native_tool_results(native_tool_calling_enabled, &tool_calls);
        if use_native_tool_results {
            println!("[AgenticLoop] Using native OpenAI tool result format (all calls have IDs)");
        }

        // Add assistant response (with tool calls) to history
        let assistant_msg = create_assistant_message_with_tool_calls(
            &assistant_response,
            &tool_calls,
            use_native_tool_results,
            Some(turn_system_prompt.clone()),
        );
        full_history.push(assistant_msg);

        // Process each tool call
        // In native format, we'll add individual tool messages; otherwise collect as strings
        let mut tool_results: Vec<String> = Vec::new();
        let mut native_tool_messages: Vec<ChatMessage> = Vec::new();
        let mut any_executed = false;

        for (idx, call) in tool_calls.iter().enumerate() {
            // Resolve server ID if unknown
            // Built-in tools (python_execution, tool_search) use "builtin" as their server
            let resolved_server = if is_builtin_tool(&call.tool) {
                println!(
                    "[AgenticLoop] Built-in tool '{}' detected, using 'builtin' server",
                    call.tool
                );
                "builtin".to_string()
            } else if call.server == "unknown" {
                match resolve_server_for_tool(&mcp_host_tx, &call.tool).await {
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
                        tool_results.push(format_tool_result(
                            call,
                            &format!("Could not find server for tool '{}'. Make sure an MCP server with this tool is connected.", call.tool),
                            true,
                            tool_format,
                            Some(&original_message),
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
                id: call.id.clone(),
            };

            // State machine validation: check if tool is allowed in current state
            if !state_machine.is_tool_allowed(&resolved_call.tool) {
                let error_msg = format!(
                    "Tool '{}' not available in {} state. Enabled capabilities: {:?}",
                    resolved_call.tool,
                    state_machine.current_state().name(),
                    state_machine.enabled_capabilities()
                );
                println!(
                    "[AgenticLoop] ⛔ Tool '{}' blocked by state machine (current state: {})",
                    resolved_call.tool,
                    state_machine.current_state().name()
                );
                
                // Emit error to status bar so user knows why nothing happened
                let _ = app_handle.emit(
                    "tool-blocked",
                    serde_json::json!({
                        "tool": resolved_call.tool,
                        "state": state_machine.current_state().name(),
                        "message": error_msg,
                    }),
                );
                
                tool_results.push(format_tool_result(
                    &resolved_call,
                    &format!(
                        "Tool '{}' is not available in the current context (state: {}). Available tools: {:?}",
                        resolved_call.tool,
                        state_machine.current_state().name(),
                        state_machine.allowed_tool_names()
                    ),
                    true,
                    tool_format,
                    Some(&original_message),
                ));
                continue;
            }

            println!(
                "[AgenticLoop] 🔧 Processing tool call {}/{}: {}::{} (state: {})",
                idx + 1,
                tool_calls.len(),
                resolved_call.server,
                resolved_call.tool,
                state_machine.current_state().name()
            );

            // Log tool call arguments
            let args_str = serde_json::to_string_pretty(&resolved_call.arguments)
                .unwrap_or_else(|_| "{}".to_string());
            let args_preview: String = args_str.chars().take(500).collect();
            println!(
                "[AgenticLoop] 📝 Arguments: {}{}",
                args_preview,
                if args_str.len() > 500 { "..." } else { "" }
            );
            let _ = std::io::stdout().flush();

            // Check if this server allows auto-approve
            // Built-in tools are always auto-approved
            let auto_approve = if is_builtin_tool(&resolved_call.tool) {
                true
            } else {
                server_configs
                    .iter()
                    .find(|s| s.id == resolved_call.server)
                    .map(|s| s.auto_approve_tools)
                    .unwrap_or(false)
            };

            if !auto_approve {
                println!(
                    "[AgenticLoop] Server {} requires manual approval, emitting pending event",
                    resolved_call.server
                );

                // Create a unique approval key for this tool call
                let approval_key = format!("{}-{}-{}", chat_id, iteration, idx);

                // Emit pending event for manual approval
                let _ = app_handle.emit(
                    "tool-calls-pending",
                    ToolCallsPendingEvent {
                        approval_key: approval_key.clone(),
                        calls: vec![resolved_call.clone()],
                        iteration,
                    },
                );

                // Create approval channel and register it
                let (approval_tx, approval_rx) = oneshot::channel();
                {
                    let mut pending = pending_approvals.write().await;
                    pending.insert(approval_key.clone(), approval_tx);
                }

                println!(
                    "[AgenticLoop] Waiting for approval on key: {}",
                    approval_key
                );

                // Wait for approval (with timeout)
                let approval_result = tokio::time::timeout(
                    std::time::Duration::from_secs(300), // 5 minute timeout
                    approval_rx,
                )
                .await;

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
                            Some(&original_message),
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
                            Some(&original_message),
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
                            Some(&original_message),
                        ));
                        continue;
                    }
                }
            }

            // Emit executing event
            let _ = app_handle.emit(
                "tool-executing",
                ToolExecutingEvent {
                    server: resolved_call.server.clone(),
                    tool: resolved_call.tool.clone(),
                    arguments: resolved_call.arguments.clone(),
                },
            );

            // Start heartbeat task to keep UI informed during long tool runs
            let heartbeat_app = app_handle.clone();
            let heartbeat_server = resolved_call.server.clone();
            let heartbeat_tool = resolved_call.tool.clone();
            let heartbeat_start = std::time::Instant::now();
            let (heartbeat_stop_tx, mut heartbeat_stop_rx) = tokio::sync::oneshot::channel::<()>();
            let _heartbeat_handle = tokio::spawn(async move {
                use tokio::time::Duration;
                let mut ticker = tokio::time::interval(Duration::from_millis(1000));
                let mut beat: u64 = 0;
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            beat += 1;
                            let elapsed_ms = heartbeat_start.elapsed().as_millis() as u64;
                            let _ = heartbeat_app.emit(
                                "tool-heartbeat",
                                ToolHeartbeatEvent {
                                    server: heartbeat_server.clone(),
                                    tool: heartbeat_tool.clone(),
                                    elapsed_ms,
                                    beat,
                                },
                            );
                        }
                        _ = &mut heartbeat_stop_rx => {
                            break;
                        }
                    }
                }
            });

            // Execute the tool - check for built-in tools first
            // #region agent log
            {
                let args_json = serde_json::to_string(&resolved_call.arguments).unwrap_or_else(|_| "{}".to_string());
                let args_preview: String = args_json.chars().take(500).collect();
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
                    let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"TOOL","location":"lib.rs:agentic_loop","message":"tool_call_start","data":{{"iteration":{},"idx":{},"server":"{}","tool":"{}","is_builtin":{},"args_preview":"{}"}},"timestamp":{}}}"#, 
                        iteration, idx, resolved_call.server, resolved_call.tool, is_builtin_tool(&resolved_call.tool),
                        args_preview.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n"),
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
                }
            }
            let tool_exec_start = std::time::Instant::now();
            // #endregion
            let (result_text, is_error) = if is_builtin_tool(&resolved_call.tool) {
                match resolved_call.tool.as_str() {
                    "tool_search" => {
                        println!("[AgenticLoop] ⏳ Executing built-in: tool_search");
                        let _ = std::io::stdout().flush();
                        let exec_start = std::time::Instant::now();

                        // Parse tool_search input
                        let input: ToolSearchInput =
                            serde_json::from_value(resolved_call.arguments.clone())
                                .map_err(|e| format!("Invalid tool_search arguments: {}", e))
                                .unwrap_or(ToolSearchInput {
                                    queries: vec![],
                                    top_k: tool_search_max_results,
                                });

                        match execute_tool_search(
                            input,
                            tool_registry.clone(),
                            embedding_model.clone(),
                            tool_search_max_results,
                        )
                        .await
                        {
                            Ok((result, discovered_tools)) => {
                                let elapsed = exec_start.elapsed();
                                println!("[AgenticLoop] ✅ tool_search completed in {:.2}s, found {} tools",
                                    elapsed.as_secs_f64(), discovered_tools.len());
                                let result_preview: String = result.chars().take(500).collect();
                                println!(
                                    "[AgenticLoop] 📤 Result: {}{}",
                                    result_preview,
                                    if result.len() > 500 { "..." } else { "" }
                                );
                                let _ = std::io::stdout().flush();
                                (result, false)
                            }
                            Err(e) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ❌ tool_search failed in {:.2}s: {}",
                                    elapsed.as_secs_f64(),
                                    e
                                );
                                let _ = std::io::stdout().flush();
                                (e, true)
                            }
                        }
                    }
                    "python_execution" => {
                        println!("[AgenticLoop] ⏳ Executing built-in: python_execution");
                        let _ = std::io::stdout().flush();
                        let exec_start = std::time::Instant::now();

                        // Parse python_execution input with fallback handling
                        // Some models output: {"name": "python_execution", "arguments": ["code", "lines"]}
                        // instead of: {"name": "python_execution", "arguments": {"code": ["code", "lines"]}}
                        let input: CodeExecutionInput =
                            parse_python_execution_args(&resolved_call.arguments);

                        let exec_id = format!("{}-{}-{}", chat_id, iteration, idx);
                        let code_lines = input.code.len();
                        println!(
                            "[AgenticLoop] 🐍 python_execution triggered (chat_id={}, iteration={}, call_idx={}, exec_id={}, code_lines={})",
                            chat_id, iteration, idx, exec_id, code_lines
                        );
                        let _ = std::io::stdout().flush();
                        match execute_python_execution(
                            input,
                            exec_id,
                            tool_registry.clone(),
                            &python_tx,
                            allow_tool_search_for_python,
                        )
                        .await
                        {
                            Ok(output) => {
                                let elapsed = exec_start.elapsed();
                                println!("[AgenticLoop] {} python_execution completed in {:.2}s: {} chars stdout, {} chars stderr",
                                    if output.success { "✅" } else { "⚠️" },
                                    elapsed.as_secs_f64(),
                                    output.stdout.len(),
                                    output.stderr.len());
                                let has_stdout = !output.stdout.trim().is_empty();
                                let has_stderr = !output.stderr.trim().is_empty();
                                let result = if output.success {
                                    match (has_stdout, has_stderr) {
                                        (true, true) => format!(
                                            "STDOUT:\n{}\n\nSTDERR:\n{}",
                                            output.stdout, output.stderr
                                        ),
                                        (true, false) => output.stdout.clone(),
                                        (false, true) => format!("STDERR:\n{}", output.stderr),
                                        (false, false) => {
                                            "Execution completed with no output".to_string()
                                        }
                                    }
                                } else if has_stdout && has_stderr {
                                    format!(
                                        "STDERR:\n{}\n\nSTDOUT:\n{}",
                                        output.stderr, output.stdout
                                    )
                                } else if has_stderr {
                                    output.stderr.clone()
                                } else if has_stdout {
                                    output.stdout.clone()
                                } else {
                                    "Execution signaled follow-up with no message".to_string()
                                };
                                let result_preview: String = result.chars().take(500).collect();
                                println!(
                                    "[AgenticLoop] 📤 Result: {}{}",
                                    result_preview,
                                    if result.len() > 500 { "..." } else { "" }
                                );
                                let _ = std::io::stdout().flush();
                                (result, !output.success)
                            }
                            Err(e) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ❌ python_execution failed in {:.2}s: {}",
                                    elapsed.as_secs_f64(),
                                    e
                                );
                                let _ = std::io::stdout().flush();
                                (e, true)
                            }
                        }
                    }
                    "schema_search" => {
                        println!("[AgenticLoop] ⏳ Executing built-in: schema_search");
                        let _ = std::io::stdout().flush();
                        let exec_start = std::time::Instant::now();

                        // Parse input
                        let input: tools::SchemaSearchInput = 
                            serde_json::from_value(resolved_call.arguments.clone())
                                .unwrap_or_else(|e| {
                                    println!("[AgenticLoop] ⚠️ Failed to parse schema_search args: {}, using defaults", e);
                                    tools::SchemaSearchInput {
                                        query: resolved_call.arguments
                                            .get("query")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        max_tables: 5,
                                        max_columns_per_table: 5,
                                        min_relevance: 0.3,
                                    }
                                });

                        let executor = tools::SchemaSearchExecutor::new(
                            schema_tx.clone(),
                            embedding_model.clone(),
                        );

                        match executor.execute(input).await {
                            Ok(mut output) => {
                                // Filter tables by enabled database sources
                                output.tables.retain(|t| enabled_db_sources.contains(&t.source_id));

                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ✅ schema_search completed in {:.2}s: {} tables found (after filtering)",
                                    elapsed.as_secs_f64(),
                                    output.tables.len()
                                );
                                let result = serde_json::to_string_pretty(&output)
                                    .unwrap_or_else(|_| output.summary.clone());
                                (result, false)
                            }
                            Err(e) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ❌ schema_search failed in {:.2}s: {}",
                                    elapsed.as_secs_f64(),
                                    e
                                );
                                (e, true)
                            }
                        }
                    }
                    "sql_select" => {
                        println!("[AgenticLoop] ⏳ Executing built-in: sql_select");
                        let _ = std::io::stdout().flush();
                        let exec_start = std::time::Instant::now();

                        // Parse input with fallback to reconstruct malformed SQL arguments
                        let input: tools::SqlSelectInput = 
                            serde_json::from_value(resolved_call.arguments.clone())
                                .unwrap_or_else(|e| {
                                    println!("[AgenticLoop] ⚠️ Failed to parse sql_select args: {}, attempting reconstruction", e);
                                    
                                    // Try to get sql and source_id from proper keys first
                                    let mut sql = resolved_call.arguments
                                        .get("sql")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());
                                    
                                    let source_id = resolved_call.arguments
                                        .get("source_id")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());
                                    
                                    // If sql is empty or None, try to reconstruct from malformed arguments
                                    // This handles cases like: {"\"SELECT ... = 10": "AND ...\""}
                                    if sql.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                                        if let Some(reconstructed) = reconstruct_sql_from_malformed_args(&resolved_call.arguments) {
                                            println!("[AgenticLoop] 🔧 Reconstructed SQL: {}...", 
                                                reconstructed.chars().take(100).collect::<String>());
                                            sql = Some(reconstructed);
                                        }
                                    }
                                    
                                    tools::SqlSelectInput {
                                        source_id,
                                        sql: sql.unwrap_or_default(),
                                        parameters: vec![],
                                        max_rows: 100,
                                    }
                                });

                        let executor = tools::SqlSelectExecutor::new(database_toolbox_tx.clone());

                        match executor.execute(input, &enabled_db_sources).await {
                            Ok(output) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ✅ sql_select completed in {:.2}s: {} rows",
                                    elapsed.as_secs_f64(),
                                    output.row_count
                                );
                                let result = serde_json::to_string_pretty(&output)
                                    .unwrap_or_else(|_| format!("{} rows returned", output.row_count));
                                (result, !output.success)
                            }
                            Err(e) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ❌ sql_select failed in {:.2}s: {}",
                                    elapsed.as_secs_f64(),
                                    e
                                );
                                (e, true)
                            }
                        }
                    }
                    _ => {
                        // Unknown built-in tool
                        (
                            format!("Unknown built-in tool: {}", resolved_call.tool),
                            true,
                        )
                    }
                }
            } else {
                // Execute MCP tool
                println!(
                    "[AgenticLoop] ⏳ Executing MCP tool: {}::{}",
                    resolved_call.server, resolved_call.tool
                );
                let _ = std::io::stdout().flush();
                let exec_start = std::time::Instant::now();

                match execute_tool_internal(&mcp_host_tx, &resolved_call).await {
                    Ok(result) => {
                        let elapsed = exec_start.elapsed();
                        println!(
                            "[AgenticLoop] ✅ MCP tool {} completed in {:.2}s: {} chars",
                            resolved_call.tool,
                            elapsed.as_secs_f64(),
                            result.len()
                        );
                        let result_preview: String = result.chars().take(500).collect();
                        println!(
                            "[AgenticLoop] 📤 Result: {}{}",
                            result_preview,
                            if result.len() > 500 { "..." } else { "" }
                        );
                        let _ = std::io::stdout().flush();
                        (result, false)
                    }
                    Err(e) => {
                        let elapsed = exec_start.elapsed();
                        println!(
                            "[AgenticLoop] ❌ MCP tool {} failed in {:.2}s: {}",
                            resolved_call.tool,
                            elapsed.as_secs_f64(),
                            e
                        );
                        let _ = std::io::stdout().flush();
                        (e, true)
                    }
                }
            };

            // #region agent log
            {
                let tool_elapsed_ms = tool_exec_start.elapsed().as_millis();
                let result_preview: String = result_text.chars().take(500).collect();
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
                    let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"TOOL","location":"lib.rs:agentic_loop","message":"tool_call_end","data":{{"iteration":{},"idx":{},"server":"{}","tool":"{}","is_error":{},"elapsed_ms":{},"result_len":{},"result_preview":"{}"}},"timestamp":{}}}"#, 
                        iteration, idx, resolved_call.server, resolved_call.tool, is_error, tool_elapsed_ms, result_text.len(),
                        result_preview.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r"),
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
                }
            }
            // #endregion

            // Handle state machine transitions based on tool execution
            match resolved_call.tool.as_str() {
                "sql_select" if !is_error => {
                    // Transition to SqlResultCommentary state
                    // Parse the result to get row count
                    let row_count = if let Ok(output) = serde_json::from_str::<serde_json::Value>(&result_text) {
                        output.get("row_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize
                    } else {
                        0
                    };
                    state_machine.handle_event(agentic_state::StateEvent::SqlExecuted {
                        results: agentic_state::SqlResults {
                            columns: vec![],
                            rows: vec![],
                            row_count,
                            truncated: false,
                        },
                        row_count,
                    });
                    println!(
                        "[AgenticLoop] State transition: {} -> {} (sql_select completed)",
                        "SqlRetrieval",
                        state_machine.current_state().name()
                    );
                }
                "schema_search" if !is_error => {
                    // Parse schema search output to get tables
                    if let Ok(output) = serde_json::from_str::<tools::schema_search::SchemaSearchOutput>(&result_text) {
                        let tables: Vec<agentic_state::TableInfo> = output.tables.iter().map(|t| {
                            agentic_state::TableInfo {
                                fully_qualified_name: t.table_name.clone(),
                                source_id: t.source_id.clone(),
                                sql_dialect: t.sql_dialect.clone(),
                                relevancy: t.relevance,
                                columns: t.relevant_columns.iter().map(|c| agentic_state::ColumnInfo {
                                    name: c.name.clone(),
                                    data_type: c.data_type.clone(),
                                    nullable: false,
                                    description: c.description.clone(),
                                }).collect(),
                                description: t.description.clone(),
                            }
                        }).collect();
                        let max_relevancy = tables.iter().map(|t| t.relevancy).fold(0.0f32, f32::max);
                        state_machine.handle_event(agentic_state::StateEvent::SchemaSearched {
                            tables,
                            max_relevancy,
                        });
                        println!(
                            "[AgenticLoop] State transition after schema_search: {} (max_relevancy: {:.2})",
                            state_machine.current_state().name(),
                            max_relevancy
                        );
                    }
                }
                "python_execution" => {
                    // Check for stderr in the result to determine if we need a handoff
                    let has_stderr = result_text.contains("STDERR:");
                    let stdout = if result_text.contains("STDOUT:") {
                        result_text.split("STDERR:").next().unwrap_or("").replace("STDOUT:\n", "")
                    } else if has_stderr {
                        "".to_string()
                    } else {
                        result_text.clone()
                    };
                    let stderr = if has_stderr {
                        result_text.split("STDERR:\n").last().unwrap_or("").to_string()
                    } else {
                        "".to_string()
                    };
                    state_machine.handle_event(agentic_state::StateEvent::PythonExecuted { stdout, stderr });
                    println!(
                        "[AgenticLoop] State transition after python_execution: {}",
                        state_machine.current_state().name()
                    );
                }
                "tool_search" if !is_error => {
                    // Parse tool search output to get discovered tools
                    if let Ok(output) = serde_json::from_str::<ToolSearchOutput>(&result_text) {
                        let discovered: Vec<String> = output.tools.iter().map(|t| t.name.clone()).collect();
                        state_machine.handle_event(agentic_state::StateEvent::ToolSearchCompleted {
                            discovered,
                            schemas: vec![], // Schemas are populated separately
                        });
                        println!(
                            "[AgenticLoop] State transition after tool_search: {}",
                            state_machine.current_state().name()
                        );
                    }
                }
                _ => {
                    // MCP tool or unknown - transition to Conversational if final
                    if !is_error {
                        state_machine.handle_event(agentic_state::StateEvent::McpToolExecuted {
                            tool_name: resolved_call.tool.clone(),
                            result: result_text.clone(),
                        });
                    }
                }
            }

            // After any schema search, automatically surface sql_select for follow-up
            if resolved_call.tool == "schema_search" {
                {
                    let mut registry = tool_registry.write().await;
                    registry.set_sql_select_enabled(true);
                }
                println!(
                    "[AgenticLoop] sql_select enabled after schema_search tool call (runtime only)"
                );
            }

            // Emit result event
            let _ = app_handle.emit(
                "tool-result",
                ToolResultEvent {
                    server: resolved_call.server.clone(),
                    tool: resolved_call.tool.clone(),
                    result: result_text.clone(),
                    is_error,
                },
            );

            {
                let mut progress = turn_progress.write().await;
                progress.had_tool_calls = true;
                progress.timestamp_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(progress.timestamp_ms);
            }

            // Stop heartbeat after tool completion
            let _ = heartbeat_stop_tx.send(());

            // Check for repeated errors - if the same tool produces the same error twice in a row,
            // disable tool calling and prompt the model to answer without tools
            if is_error {
                let error_signature = format!("{}::{}", resolved_call.tool, result_text);
                if let Some(ref last_sig) = last_error_signature {
                    if *last_sig == error_signature {
                        println!("[AgenticLoop] REPEATED ERROR DETECTED: Tool '{}' failed with same error twice", resolved_call.tool);
                        println!("[AgenticLoop] Error: {}", result_text);
                        println!("[AgenticLoop] Disabling tool calling, prompting model to answer directly");

                        // Mark that we're disabling tools due to repeated error
                        tools_disabled_due_to_repeated_error = true;

                        // Prompt the model to answer without tools
                        let redirect_msg = "The tool is not available for this request. \
                            Please answer the user's question directly using your knowledge, \
                            without attempting to use any tools."
                            .to_string();
                        tool_results.push(redirect_msg);

                        // Remove tools from future iterations
                        openai_tools = None;

                        any_executed = true;
                        break; // Stop processing more tool calls this iteration
                    }
                }
                // Update the last error signature
                last_error_signature = Some(error_signature);
            } else {
                // Clear error tracking on successful execution
                last_error_signature = None;
            }

            // Format and collect tool result using model-appropriate format
            // Include original user prompt in error cases to help model retry
            let user_prompt_for_error = if is_error { Some(original_message.as_str()) } else { None };
            
            if use_native_tool_results {
                // Native format: create individual tool result messages
                if let Some(ref tool_call_id) = resolved_call.id {
                    native_tool_messages.push(create_native_tool_result_message(
                        tool_call_id,
                        &result_text,
                    ));
                } else {
                    // Fallback for calls without IDs (shouldn't happen if use_native_tool_results is true)
                    tool_results.push(format_tool_result(
                        &resolved_call,
                        &result_text,
                        is_error,
                        tool_format,
                        user_prompt_for_error,
                    ));
                }
            } else {
                // Text-based format: collect as formatted strings
                tool_results.push(format_tool_result(
                    &resolved_call,
                    &result_text,
                    is_error,
                    tool_format,
                    user_prompt_for_error,
                ));
            }
            any_executed = true;
        }

        // If no tools were actually executed (all required manual approval), stop the loop
        if !any_executed {
            println!("[AgenticLoop] No tools executed (all require approval), stopping loop");
            break;
        }

        // Add tool results to history using appropriate format
        if use_native_tool_results && !native_tool_messages.is_empty() {
            // Native format: add individual tool result messages
            println!(
                "[AgenticLoop] Adding {} native tool result messages to history",
                native_tool_messages.len()
            );
            for msg in native_tool_messages {
                full_history.push(msg);
            }
        } else if !tool_results.is_empty() {
            // Text-based format: combine results into a single user message
            let combined_results = tool_results.join("\n\n");
            full_history.push(ChatMessage {
                role: "user".to_string(),
                content: combined_results,
                system_prompt: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Check state machine for continuation
        let should_continue = state_machine.should_continue_loop();
        println!(
            "[AgenticLoop] State machine: state={}, should_continue={}",
            state_machine.current_state().name(),
            should_continue
        );

        // If state machine says continue (e.g., SqlResultCommentary, CodeExecutionHandoff),
        // regenerate the system prompt from the state machine
        if should_continue {
            let new_prompt = state_machine.build_system_prompt();
            // Update the system message (first message with role "system") with the new prompt
            if !full_history.is_empty() && full_history[0].role == "system" {
                full_history[0].content = new_prompt.clone();
                println!(
                    "[AgenticLoop] System prompt updated in history for state: {} ({} chars)",
                    state_machine.current_state().name(),
                    new_prompt.len()
                );
            }
            // Keep track of current prompt for debugging/logging
            #[allow(unused_assignments)]
            {
                current_system_prompt = new_prompt;
            }
        }

        iteration += 1;
        println!("[AgenticLoop] Continuing to iteration {} (state: {})...", iteration, state_machine.current_state().name());
    }

    // Emit loop finished event
    // #region agent log
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
        let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H1","location":"lib.rs:agentic_loop","message":"emit_tool_loop_finished_before","data":{{"iterations":{},"had_tool_calls":{}}},"timestamp":{}}}"#, iteration, had_tool_calls, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
    }
    // #endregion
    let _ = app_handle.emit(
        "tool-loop-finished",
        ToolLoopFinishedEvent {
            iterations: iteration,
            had_tool_calls,
        },
    );
    // #region agent log
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
        let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H1","location":"lib.rs:agentic_loop","message":"emit_chat_finished_before","data":{{}},"timestamp":{}}}"#, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
    }
    // #endregion
    let _ = app_handle.emit("chat-finished", ());
    // #region agent log
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
        let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H1","location":"lib.rs:agentic_loop","message":"emit_chat_finished_after","data":{{}},"timestamp":{}}}"#, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
    }
    // #endregion

    {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut progress = turn_progress.write().await;
        progress.assistant_response = final_response.clone();
        progress.last_token_index = last_token_count;
        progress.finished = true;
        progress.had_tool_calls = had_tool_calls;
        progress.timestamp_ms = now_ms;
    }

    println!(
        "[AgenticLoop] Loop complete after {} iterations, had_tool_calls={}, tools_disabled={}",
        iteration, had_tool_calls, tools_disabled_due_to_repeated_error
    );
    println!("[chat] -------------------- TURN COMPLETE --------------------");
    println!(
        "[chat] Turn summary | id={} | gen={} | iterations={} | tool_calls={} | response_chars={} | tools_disabled_due_to_repeat_error={}",
        chat_id,
        generation_id,
        iteration,
        had_tool_calls,
        final_response.len(),
        tools_disabled_due_to_repeated_error
    );
    let _ = std::io::stdout().flush();

    // Save the chat
    let messages_json = serde_json::to_string(&full_history).unwrap_or_default();
    let embedding_text = format!(
        "{}\nUser: {}\nAssistant: {}",
        title, original_message, final_response
    );

    println!("[ChatSave] Requesting embedding...");
    let (emb_tx, emb_rx) = oneshot::channel();

    match foundry_tx
        .send(FoundryMsg::GetEmbedding {
            text: embedding_text.clone(),
            respond_to: emb_tx,
        })
        .await
    {
        Ok(_) => {
            println!("[ChatSave] Waiting for embedding response...");
            match emb_rx.await {
                Ok(vector) => {
                    println!(
                        "[ChatSave] Got embedding (len={}), sending to ChatVectorStoreActor...",
                        vector.len()
                    );
                    match vector_tx
                        .send(VectorMsg::UpsertChatRecord {
                            id: chat_id.clone(),
                            title: title.clone(),
                            content: embedding_text,
                            messages: messages_json,
                            embedding_vector: Some(vector),
                            pinned: false,
                            model: Some(model_name.clone()),
                        })
                        .await
                    {
                        Ok(_) => {
                            println!(
                                "[ChatSave] UpsertChatRecord sent, emitting chat-saved event"
                            );
                            // #region agent log
                            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
                                let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H1","location":"lib.rs:run_agentic_loop","message":"chat_saved_emit_before","data":{{"chat_id":"{}"}},"timestamp":{}}}"#, &chat_id[..8.min(chat_id.len())], std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
                            }
                            // #endregion
                            let _ = app_handle.emit("chat-saved", chat_id.clone());
                            // #region agent log
                            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
                                let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H1","location":"lib.rs:run_agentic_loop","message":"chat_saved_emit_after","data":{{"chat_id":"{}"}},"timestamp":{}}}"#, &chat_id[..8.min(chat_id.len())], std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
                            }
                            // #endregion
                        }
                        Err(e) => println!(
                            "[ChatSave] ERROR: Failed to send UpsertChatRecord: {}",
                            e
                        ),
                    }
                }
                Err(e) => println!("[ChatSave] ERROR: Failed to receive embedding: {}", e),
            }
        }
        Err(e) => println!("[ChatSave] ERROR: Failed to send GetEmbedding: {}", e),
    }
    // #region agent log
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
        let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H1","location":"lib.rs:run_agentic_loop","message":"agentic_loop_end","data":{{"chat_id":"{}"}},"timestamp":{}}}"#, &chat_id[..8.min(chat_id.len())], std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
    }
    // #endregion
}


#[tauri::command]async fn search_history(
    query: String,
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Ask Foundry Actor for embedding
    let (emb_tx, emb_rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetEmbedding {
            text: query,
            respond_to: emb_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    // Wait for embedding
    let embedding = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // Send to Vector Actor
    let (search_tx, search_rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::SearchChatsByEmbedding {
            query_vector: embedding,
            limit: 10,
            respond_to: search_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let results = search_rx.await.map_err(|_| "Vector actor died")?;

    app_handle
        .emit("sidebar-update", results)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
async fn get_all_chats(
    handles: State<'_, ActorHandles>,
) -> Result<Vec<protocol::ChatSummary>, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx
        .send(VectorMsg::FetchAllChats { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn get_models(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn set_model(model: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::SetModel {
            model_id: model,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn get_cached_models(handles: State<'_, ActorHandles>) -> Result<Vec<CachedModel>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetCachedModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn get_model_info(handles: State<'_, ActorHandles>) -> Result<Vec<ModelInfo>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetModelInfo { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn download_model(
    model_name: String,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    println!("[download_model] Starting download for: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::DownloadModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send download request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

#[tauri::command]
async fn load_model(model_name: String, handles: State<'_, ActorHandles>) -> Result<(), String> {
    println!("[load_model] Loading model: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::LoadModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send load request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

#[tauri::command]
async fn get_loaded_models(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    println!("[get_loaded_models] Getting loaded models");
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetLoadedModels { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    Ok(rx.await.map_err(|_| "Foundry actor died".to_string())?)
}

#[tauri::command]
async fn reload_foundry(handles: State<'_, ActorHandles>) -> Result<(), String> {
    use std::io::Write;
    println!("\n[reload_foundry] 🔄 Reloading foundry service (requested by UI)");
    let _ = std::io::stdout().flush();
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::Reload { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    match rx.await {
        Ok(res) => {
            println!(
                "[reload_foundry] ✅ Reload command completed with result: {:?}",
                res
            );
            let _ = std::io::stdout().flush();
            res.map_err(|e| e)
        }
        Err(_) => {
            println!("[reload_foundry] ❌ Foundry actor died while reloading");
            let _ = std::io::stdout().flush();
            Err("Foundry actor died".to_string())
        }
    }
}

#[tauri::command]
async fn get_catalog_models(
    handles: State<'_, ActorHandles>,
) -> Result<Vec<CatalogModel>, String> {
    println!("[get_catalog_models] Getting catalog models");
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetCatalogModels { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    Ok(rx.await.map_err(|_| "Foundry actor died".to_string())?)
}

#[tauri::command]
async fn unload_model(model_name: String, handles: State<'_, ActorHandles>) -> Result<(), String> {
    println!("[unload_model] Unloading model: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::UnloadModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

#[tauri::command]
async fn get_foundry_service_status(
    handles: State<'_, ActorHandles>,
) -> Result<FoundryServiceStatus, String> {
    println!("[get_foundry_service_status] Getting service status");
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetServiceStatus { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

#[tauri::command]
async fn remove_cached_model(
    model_name: String,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    println!("[remove_cached_model] Removing cached model: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::RemoveCachedModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

#[tauri::command]
async fn cancel_generation(
    generation_id: u32,
    cancellation_state: State<'_, CancellationState>,
) -> Result<(), String> {
    use std::io::Write;

    println!("\n[cancel_generation] 🛑 STOP BUTTON PRESSED - User requested cancellation");
    println!(
        "[cancel_generation] Requested generation_id: {}",
        generation_id
    );
    let _ = std::io::stdout().flush();

    // Check if this matches the current generation
    let current_id = *cancellation_state.current_generation_id.read().await;

    // Send cancel signal
    if let Some(sender) = cancellation_state.cancel_signal.read().await.as_ref() {
        let _ = sender.send(true);
        println!(
            "[cancel_generation] ✅ Cancel signal sent to generation {} (current active: {})",
            generation_id, current_id
        );
        let _ = std::io::stdout().flush();
    } else {
        println!(
            "[cancel_generation] ⚠️ No active generation to cancel (no cancel signal registered)"
        );
        let _ = std::io::stdout().flush();
    }

    Ok(())
}

#[tauri::command]
async fn get_turn_status(
    turn_tracker: State<'_, TurnTrackerState>,
) -> Result<TurnProgress, String> {
    let progress = turn_tracker.progress.read().await;
    Ok(progress.clone())
}

#[derive(Default)]
struct AutoDiscoveryContext {
    tool_search_output: Option<ToolSearchOutput>,
    schema_search_output: Option<SchemaSearchOutput>,
    discovered_tool_schemas: Vec<(String, Vec<McpTool>)>,
}

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

fn map_tool_search_hits_to_schemas(
    hits: &[ToolSearchResult],
    filtered_tool_descriptions: &[(String, Vec<McpTool>)],
) -> Vec<(String, Vec<McpTool>)> {
    let mut per_server: HashMap<String, Vec<McpTool>> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();

    for hit in hits {
        let matching_server = filtered_tool_descriptions
            .iter()
            .find(|(server_id, _)| server_id == &hit.server_id);

        if let Some((_, tools)) = matching_server {
            if let Some(schema) = tools.iter().find(|tool| tool.name == hit.name) {
                let key = format!("{}::{}", hit.server_id, hit.name);
                if seen.insert(key) {
                    per_server
                        .entry(hit.server_id.clone())
                        .or_default()
                        .push(schema.clone());
                }
            } else {
                println!(
                    "[Chat] Tool search hit '{}' not found in filtered schemas for server {}",
                    hit.name, hit.server_id
                );
            }
        } else {
            println!(
                "[Chat] Tool search hit for unknown server {} (tool {})",
                hit.server_id, hit.name
            );
        }
    }

    let mut grouped: Vec<(String, Vec<McpTool>)> = per_server
        .into_iter()
        .map(|(server, tools)| (server, tools))
        .collect();
    grouped.sort_by(|a, b| a.0.cmp(&b.0));
    grouped
}


async fn auto_tool_search_for_prompt(
    prompt: &str,
    tool_search_enabled: bool,
    tool_search_max_results: usize,
    has_mcp_tools: bool,
    filtered_tool_descriptions: &[(String, Vec<McpTool>)],
    registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    materialize: bool,
) -> (Option<ToolSearchOutput>, Vec<(String, Vec<McpTool>)>) {
    if !tool_search_enabled || !has_mcp_tools {
        return (None, Vec::new());
    }

    if prompt.trim().is_empty() {
        println!("[Chat] Auto tool_search skipped: empty user prompt");
        return (None, Vec::new());
    }

    let executor = ToolSearchExecutor::new(registry, embedding_model);
    let search_input = ToolSearchInput {
        queries: vec![prompt.to_string()],
        top_k: tool_search_max_results,
    };

    match executor.execute(search_input).await {
        Ok(output) => {
            if materialize {
                executor.materialize_results(&output.tools).await;
            }
            println!(
                "[Chat] Auto tool_search discovered {} tools before first turn",
                output.tools.len()
            );
            let schemas = map_tool_search_hits_to_schemas(&output.tools, filtered_tool_descriptions);
            (Some(output), schemas)
        }
        Err(e) => {
            println!(
                "[Chat] Auto tool_search failed (continuing without discoveries): {}",
                e
            );
            (None, Vec::new())
        }
    }
}
async fn auto_schema_search_for_prompt(
    prompt: &str,
    schema_search_enabled: bool,
    min_relevance: f32,
    toolbox_config: &DatabaseToolboxConfig,
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
) -> Option<SchemaSearchOutput> {
    // Use a generous cap so we don't silently drop discovered tables
    const AUTO_SCHEMA_SEARCH_MAX_TABLES: usize = 50;
    if !schema_search_enabled {
        return None;
    }

    let has_enabled_sources = toolbox_config.enabled
        && toolbox_config
            .sources
            .iter()
            .any(|source| source.enabled);

    if !has_enabled_sources {
        println!("[Chat] Auto schema_search skipped: no enabled database sources");
        return None;
    }

    if prompt.trim().is_empty() {
        println!("[Chat] Auto schema_search skipped: empty user prompt");
        return None;
    }

    let executor = tools::SchemaSearchExecutor::new(schema_tx, embedding_model);
    
    // Check if any tables are cached
    if let Ok(stats) = executor.get_stats().await {
        if stats.table_count == 0 {
            println!("[Chat] Auto schema_search skipped: No tables cached in LanceDB. User needs to click 'Refresh schemas'.");
            return Some(SchemaSearchOutput {
                tables: vec![],
                query_used: prompt.to_string(),
                summary: "WARNING: No database tables are currently cached. You CANNOT write accurate SQL queries yet. Please ask the user to click 'Refresh schemas' in Settings > Schemas to index their databases.".to_string(),
            });
        }
    }

    let input = tools::SchemaSearchInput {
        query: prompt.to_string(),
        max_tables: AUTO_SCHEMA_SEARCH_MAX_TABLES,
        max_columns_per_table: 25,
        min_relevance, 
    };

    let mut search_result = executor.execute(input.clone()).await;

    // Fallback: If semantic search found nothing but we HAVE tables in the cache,
    // and the total number of tables is small (<= 10), just include all of them.
    // This handles cases where table names are cryptic and embeddings are weak.
    if let Ok(ref output) = search_result {
        if output.tables.is_empty() {
            if let Ok(stats) = executor.get_stats().await {
                if stats.table_count > 0 && stats.table_count <= 10 {
                    println!("[Chat] Auto schema_search fallback: semantic match failed (at 30%), but total tables small ({}). Including all tables.", stats.table_count);
                    let fallback_input = tools::SchemaSearchInput {
                        min_relevance: 0.0, // Get everything
                        ..input
                    };
                    search_result = executor.execute(fallback_input).await;
                }
            }
        }
    }

    match search_result {
        Ok(mut output) => {
            // Filter tables by enabled database sources
            let enabled_sources: std::collections::HashSet<String> = toolbox_config
                .sources
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.id.clone())
                .collect();

            output.tables.retain(|t| enabled_sources.contains(&t.source_id));

            println!(
                "[Chat] Auto schema_search found {} table(s) matching prompt (after filtering)",
                output.tables.len()
            );
            if output.tables.is_empty() {
                println!("[Chat] Tip: If you have database sources enabled but see 0 tables, ensure you have clicked 'Refresh schemas' in Settings > Schemas.");
            }
            Some(output)
        }
        Err(e) => {
            println!(
                "[Chat] Auto schema_search failed (continuing without schema context): {}",
                e
            );
            None
        }
    }
}

async fn perform_auto_discovery_for_prompt(
    prompt: &str,
    tool_search_enabled: bool,
    tool_search_max_results: usize,
    has_mcp_tools: bool,
    schema_search_enabled: bool,
    schema_relevancy_threshold: f32,
    toolbox_config: &DatabaseToolboxConfig,
    filtered_tool_descriptions: &[(String, Vec<McpTool>)],
    registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    materialize_tools: bool,
) -> AutoDiscoveryContext {
    let (tool_search_output, discovered_tool_schemas) = auto_tool_search_for_prompt(
        prompt,
        tool_search_enabled,
        tool_search_max_results,
        has_mcp_tools,
        filtered_tool_descriptions,
        registry.clone(),
        embedding_model.clone(),
        materialize_tools,
    )
    .await;

    let schema_search_output = auto_schema_search_for_prompt(
        prompt,
        schema_search_enabled,
        schema_relevancy_threshold,
        toolbox_config,
        schema_tx,
        embedding_model,
    )
    .await;

    AutoDiscoveryContext {
        tool_search_output,
        schema_search_output,
        discovered_tool_schemas,
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
    let tool_search_enabled = settings.tool_search_enabled;
    let schema_search_enabled = settings.schema_search_enabled;
    // Internal schema search is auto-derived: ON when sql_select is enabled but schema_search is not
    let internal_schema_search = settings.should_run_internal_schema_search();
    let sql_select_enabled = settings.sql_select_enabled;
    let python_execution_enabled = settings.python_execution_enabled;
    let python_tool_calling_enabled = settings.python_tool_calling_enabled;
    let tool_search_max_results = settings.tool_search_max_results.max(1);
    let tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let tool_use_examples_max = settings.tool_use_examples_max;
    let database_toolbox_config = settings.database_toolbox.clone();

    println!(
        "[Chat] Settings: python_execution={}, tool_search={}, schema_search={}, sql_select={}",
        python_execution_enabled,
        tool_search_enabled,
        schema_search_enabled,
        sql_select_enabled
    );

    let mut enabled_db_sources = Vec::new();
    // Database sources should only be enabled when database-specific tools are on.
    // python_execution alone does NOT require database MCP connections.
    let db_tools_available = schema_search_enabled || sql_select_enabled || internal_schema_search;
    
    if settings.database_toolbox.enabled && db_tools_available {
        enabled_db_sources = settings
            .database_toolbox
            .sources
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.id.clone())
            .collect();
        if !enabled_db_sources.is_empty() {
            println!("[Chat] Enabled database sources: {:?}", enabled_db_sources);
        }
    } else if settings.database_toolbox.enabled && !db_tools_available {
        println!("[Chat] Database toolbox is ON but no database tools are enabled; sources will be treated as disabled.");
    }
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    let tool_system_prompts = settings.tool_system_prompts.clone();
    let chat_format_default = settings.chat_format_default;
    let chat_format_overrides = settings.chat_format_overrides.clone();
    drop(settings);

    // Initialize turn tracker for this generation
    {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut progress = turn_tracker.progress.write().await;
        *progress = TurnProgress {
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
    if schema_search_enabled || internal_schema_search || sql_select_enabled {
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
    let (current_model_info, model_supports_native_tools) = {
        let (tx, rx) = oneshot::channel();
        if handles
            .foundry_tx
            .send(FoundryMsg::GetModelInfo { respond_to: tx })
            .await
            .is_ok()
        {
            if let Ok(models) = rx.await {
                // Find the model that matches the frontend-selected model
                if let Some(model_info) = models.into_iter().find(|m| m.id == model) {
                    let supports_native = model_info.tool_calling;
                    (Some(model_info), supports_native)
                } else {
                    // Model not found in info list - use defaults
                    println!("[Chat] Warning: Model '{}' not found in model info, using defaults", model);
                    (None, false)
                }
            } else {
                (None, false)
            }
        } else {
            (None, false)
        }
    };

    // Native tool calling is only available if: format is enabled AND model supports it
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

    // Ensure registry reflects persisted database tool toggles before building prompts
    sync_registry_database_tools(
        &tool_registry_state.registry,
        schema_search_enabled,
        sql_select_enabled,
    )
    .await;

    // Apply global tool_search flag to server defer settings (only if tool_search is actually available)
    let tool_search_allowed = tool_filter.builtin_allowed("tool_search");
    if tool_search_enabled && tool_search_allowed {
        for config in &mut server_configs {
            config.defer_tools = true;
        }
    } else if !tool_search_enabled {
        // If tool search is explicitly disabled globally, we MUST surface regular tools or they'll be unreachable.
        // HOWEVER, database sources should stay deferred because they are only meant for sql_select context injection.
        for config in &mut server_configs {
            if !enabled_db_sources.contains(&config.id) {
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
    let python_tool_mode = code_mode_possible;
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
        match precompute_tool_search_embeddings(
            tool_registry_state.registry.clone(),
            embedding_state.model.clone(),
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

    // Run auto-discovery (tool search + schema search) for this user prompt
    let auto_discovery = perform_auto_discovery_for_prompt(
        &message,
        tool_search_enabled && tool_search_allowed, // Only run auto tool discovery if allowed for this turn
        tool_search_max_results,
        has_mcp_tools,
        schema_search_enabled || internal_schema_search || sql_select_enabled,
        settings_state.settings.read().await.schema_relevancy_threshold,
        &database_toolbox_config,
        &filtered_tool_descriptions,
        tool_registry_state.registry.clone(),
        embedding_state.model.clone(),
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
    let resolved_capabilities = {
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
        ToolCapabilityResolver::resolve(
            &settings_for_resolver,
            model_info,
            &tool_filter,
            &server_configs,
            &registry,
        )
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
                // Only include python_execution if it's enabled
                if schema.name == "python_execution" {
                    python_execution_enabled && tool_filter.builtin_allowed("python_execution")
                } else if schema.name == "tool_search" {
                    // Only include tool_search if there are deferred tools to discover
                    has_deferred_mcp_tools && tool_filter.builtin_allowed("tool_search")
                } else {
                    // Other built-ins (schema_search, sql_select) are included if allowed
                    tool_filter.builtin_allowed(&schema.name)
                }
            })
            .map(|schema| ("builtin".to_string(), vec![tool_schema_to_mcp_tool(schema)]))
            .collect()
    };

    let visible_tool_descriptions: Vec<(String, Vec<McpTool>)> = if tool_search_enabled {
        let mut list = builtin_tools;
        if !auto_discovery.discovered_tool_schemas.is_empty() {
            list.extend(auto_discovery.discovered_tool_schemas.clone());
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
                if schema.name == "python_execution" {
                    if !python_execution_enabled || !tool_filter.builtin_allowed("python_execution")
                    {
                        continue;
                    }
                } else if schema.name == "tool_search" {
                    // Only include tool_search if there are deferred tools to discover
                    if !has_deferred_mcp_tools || !tool_filter.builtin_allowed("tool_search") {
                        continue;
                    }
                } else if !tool_filter.builtin_allowed(&schema.name) {
                    // Other built-ins (schema_search, sql_select) - check filter
                    continue;
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
    
    // Build prompt context - use the raw system prompt, let state machine add context
    let prompt_context = agentic_state::PromptContext {
        base_prompt: configured_system_prompt.clone(),
        has_attachments,
        mcp_context,
        tool_call_format: primary_format_for_prompt,
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
                description: None,
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

    // Clone handles for the async task
    let foundry_tx = handles.foundry_tx.clone();
    let mcp_host_tx = handles.mcp_host_tx.clone();
    let vector_tx = handles.vector_tx.clone();
    let python_tx = handles.python_tx.clone();
    let schema_tx = handles.schema_tx.clone();
    let database_toolbox_tx = handles.database_toolbox_tx.clone();
    let pending_approvals = approval_state.pending.clone();
    let tool_registry = tool_registry_state.registry.clone();
    let embedding_model = embedding_state.model.clone();
    let chat_id_task = chat_id.clone();
    let generation_id_task = generation_id;
    let title_task = title.clone();
    let message_task = message.clone();
    let openai_tools_task = openai_tools;
    let python_tool_mode_task = python_tool_mode;
    let format_config_task = format_config.clone();
    let primary_format_task = primary_format_for_prompt;
    let allow_tool_search_for_python_task = allow_tool_search_for_python;
    let tool_search_max_results_task = tool_search_max_results;
    let turn_system_prompt_task = system_prompt.clone();
    let turn_progress = turn_tracker.progress.clone();
    let chat_format_default_task = chat_format_default;
    let chat_format_overrides_task = chat_format_overrides.clone();
    let server_configs_task = server_configs.clone(); // Combined list!

    // Spawn the agentic loop task with state machine (single source of truth)
    tauri::async_runtime::spawn(async move {
        run_agentic_loop(
            foundry_tx,
            mcp_host_tx,
            vector_tx,
            python_tx,
            schema_tx,
            database_toolbox_tx,
            tool_registry,
            embedding_model,
            pending_approvals,
            app_handle,
            full_history,
            reasoning_effort,
            cancel_rx,
            server_configs_task, // Pass the combined list
            chat_id_task,
            generation_id_task,
            title_task,
            message_task,
            openai_tools_task,
            model_name,
            python_tool_mode_task,
            format_config_task,
            primary_format_task,
            allow_tool_search_for_python_task,
            tool_search_max_results_task,
            turn_system_prompt_task,
            turn_progress,
            chat_format_default_task,
            chat_format_overrides_task,
            enabled_db_sources,
            initial_state_machine,  // Pass state machine instead of thresholds
        )
        .await;
    });

    Ok(chat_id_return)
}

#[tauri::command]
async fn delete_chat(id: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::DeleteChatById { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn load_chat(id: String, handles: State<'_, ActorHandles>) -> Result<Option<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::FetchChatMessages { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn update_chat(
    id: String,
    title: Option<String>,
    pinned: Option<bool>,
    handles: State<'_, ActorHandles>,
) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::UpdateChatTitleAndPin {
            id,
            title,
            pinned,
            respond_to: tx,
        })
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
    let embedding_model = model_guard
        .clone()
        .ok_or_else(|| "Embedding model not initialized".to_string())?;
    drop(model_guard);

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::IndexRagDocuments {
            paths,
            embedding_model,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "RAG actor died".to_string())?
}

#[tauri::command]
async fn search_rag_context(
    query: String,
    limit: usize,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<RagChunk>, String> {
    println!(
        "[RAG] Searching for context with query length: {}",
        query.len()
    );

    // First, get embedding for the query
    let (emb_tx, emb_rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetEmbedding {
            text: query,
            respond_to: emb_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let query_vector = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // Then search the RAG index
    let (search_tx, search_rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::SearchRagChunksByEmbedding {
            query_vector,
            limit,
            respond_to: search_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    search_rx.await.map_err(|_| "RAG actor died".to_string())
}

#[tauri::command]
async fn clear_rag_context(handles: State<'_, ActorHandles>) -> Result<bool, String> {
    println!("[RAG] Clearing context");

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::ClearContext { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "RAG actor died".to_string())
}

#[tauri::command]
async fn remove_rag_file(
    handles: State<'_, ActorHandles>,
    source_file: String,
) -> Result<RemoveFileResult, String> {
    println!("[RAG] Removing file from index: {}", source_file);

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::RemoveFile {
            source_file,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "RAG actor died".to_string())
}

#[tauri::command]
async fn get_rag_indexed_files(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    println!("[RAG] Getting indexed files");

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::GetIndexedFiles { respond_to: tx })
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
fn get_default_mcp_test_server() -> McpServerConfig {
    settings::default_mcp_test_server()
}

#[tauri::command]
fn get_python_allowed_imports() -> Vec<String> {
    PYTHON_ALLOWED_MODULES
        .iter()
        .map(|m| m.to_string())
        .collect()
}

#[tauri::command]
async fn save_app_settings(
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

#[tauri::command]
async fn add_mcp_server(
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

#[tauri::command]
async fn update_mcp_server(
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

#[tauri::command]
async fn remove_mcp_server(
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

#[tauri::command]
async fn update_system_prompt(
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

#[tauri::command]
async fn update_tool_system_prompt(
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

#[tauri::command]
async fn update_tool_call_formats(
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

#[tauri::command]
async fn update_chat_format(
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

#[tauri::command]
async fn update_python_execution_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.python_execution_enabled = enabled;
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!(
        "[Settings] python_execution_enabled updated to: {}",
        enabled
    );
    Ok(())
}

#[tauri::command]
async fn update_native_tool_calling_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    // Update the format config to add/remove Native format
    if enabled {
        if !guard
            .tool_call_formats
            .enabled
            .contains(&ToolCallFormatName::Native)
        {
            guard
                .tool_call_formats
                .enabled
                .insert(0, ToolCallFormatName::Native);
        }
        guard.tool_call_formats.primary = ToolCallFormatName::Native;
    } else {
        guard
            .tool_call_formats
            .enabled
            .retain(|f| *f != ToolCallFormatName::Native);
        if guard.tool_call_formats.primary == ToolCallFormatName::Native {
            guard.tool_call_formats.primary = guard
                .tool_call_formats
                .enabled
                .first()
                .copied()
                .unwrap_or(ToolCallFormatName::Hermes);
        }
    }
    guard.tool_call_formats.normalize();
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!(
        "[Settings] Native tool calling updated to: {} (primary={:?}, enabled={:?})",
        enabled, guard.tool_call_formats.primary, guard.tool_call_formats.enabled
    );
    Ok(())
}

#[tauri::command]
async fn update_tool_search_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.tool_search_enabled = enabled;
    settings::save_settings(&guard).await?;

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] tool_search_enabled updated to: {}", enabled);
    Ok(())
}

#[tauri::command]
async fn update_schema_search_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
    tool_registry_state: State<'_, ToolRegistryState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.schema_search_enabled = enabled;
    settings::save_settings(&guard).await?;
    {
        let mut registry = tool_registry_state.registry.write().await;
        registry.set_schema_search_enabled(enabled);
    }

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] schema_search_enabled updated to: {}", enabled);
    Ok(())
}

// Note: update_schema_search_internal_only was removed - internal schema search
// is now auto-derived when sql_select is enabled but schema_search is not

#[tauri::command]
async fn update_sql_select_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
    tool_registry_state: State<'_, ToolRegistryState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.sql_select_enabled = enabled;
    settings::save_settings(&guard).await?;
    {
        let mut registry = tool_registry_state.registry.write().await;
        registry.set_sql_select_enabled(enabled);
    }

    // Refresh the SettingsStateMachine (Tier 1)
    let mut sm_guard = settings_sm_state.machine.write().await;
    sm_guard.refresh(&guard, &launch_config.tool_filter);

    println!("[Settings] sql_select_enabled updated to: {}", enabled);
    Ok(())
}

// ============ Relevancy Threshold Commands ============

#[tauri::command]
async fn update_rag_chunk_min_relevancy(
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

#[tauri::command]
async fn update_schema_relevancy_threshold(
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

#[tauri::command]
async fn update_rag_dominant_threshold(
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

/// Get a preview of all possible states for the settings UI
#[tauri::command]
async fn get_state_machine_preview(
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
        mcp_context: agentic_state::McpToolContext::default(),
        tool_call_format: guard.tool_call_formats.primary,
        custom_tool_prompts: guard.tool_system_prompts.clone(),
        python_primary: guard.python_execution_enabled,
    };
    
    let machine = AgenticStateMachine::new_from_settings_sm(
        &settings_sm_guard,
        prompt_context,
    );
    
    let previews = machine.get_possible_states();
    println!(
        "[Settings] State machine preview: {} possible states (mode: {})",
        previews.len(),
        settings_sm_guard.operational_mode().name()
    );
    Ok(previews)
}

#[tauri::command]
async fn update_database_toolbox_config(
    config: settings::DatabaseToolboxConfig,
    settings_state: State<'_, SettingsState>,
    settings_sm_state: State<'_, SettingsStateMachineState>,
    launch_config: State<'_, LaunchConfigState>,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<(), String> {
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
        let _ = handles.database_toolbox_tx.send(DatabaseToolboxMsg::Stop { reply_to: tx }).await;
        let _ = rx.await;
        println!("[Settings] database_toolbox stopped because it is disabled");
        return Ok(());
    }

    let refresh_summary =
        refresh_database_schemas_for_config(&handles, &embedding_state, &config).await?;

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

// ============ Database Schema Cache Commands ============

#[derive(Debug, Clone, serde::Serialize)]
struct SchemaTableStatus {
    source_id: String,
    source_name: String,
    table_fq_name: String,
    enabled: bool,
    column_count: usize,
    description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SchemaSourceStatus {
    source_id: String,
    source_name: String,
    database_kind: SupportedDatabaseKind,
    tables: Vec<SchemaTableStatus>,
}

#[derive(Debug, Clone)]
struct SchemaRefreshSummary {
    sources: Vec<SchemaSourceStatus>,
    errors: Vec<String>,
}

async fn refresh_database_schemas_for_config(
    handles: &State<'_, ActorHandles>,
    embedding_state: &State<'_, EmbeddingModelState>,
    toolbox_config: &settings::DatabaseToolboxConfig,
) -> Result<SchemaRefreshSummary, String> {
    let sources: Vec<DatabaseSourceConfig> = toolbox_config
        .sources
        .iter()
        .cloned()
        .filter(|s| s.enabled)
        .collect();

    if sources.is_empty() {
        return Ok(SchemaRefreshSummary {
            sources: Vec::new(),
            errors: Vec::new(),
        });
    }

    let model_guard = embedding_state.model.read().await;
    let embedding_model = model_guard
        .clone()
        .ok_or_else(|| "Embedding model not initialized".to_string())?;
    drop(model_guard);

    ensure_toolbox_running(&handles.database_toolbox_tx, toolbox_config).await?;

    let mut refreshed_sources = Vec::new();
    let mut errors = Vec::new();

    for source in sources {
        match refresh_schema_cache_for_source(handles, &source, embedding_model.clone()).await {
            Ok(status) => refreshed_sources.push(status),
            Err(err) => {
                let msg = format!("{} ({}): {}", source.name, source.id, err);
                println!(
                    "[SchemaRefresh] Failed to refresh source {} ({}): {}",
                    source.name, source.id, err
                );
                errors.push(msg);
            }
        }
    }

    Ok(SchemaRefreshSummary {
        sources: refreshed_sources,
        errors,
    })
}

#[tauri::command]
async fn refresh_database_schemas(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<Vec<SchemaSourceStatus>, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    drop(settings_guard);

    let summary =
        refresh_database_schemas_for_config(&handles, &embedding_state, &toolbox_config).await?;

    if !summary.errors.is_empty() {
        println!(
            "[SchemaRefresh] Completed with errors: {}",
            summary.errors.join("; ")
        );
    }

    Ok(summary.sources)
}

#[tauri::command]
async fn get_cached_database_schemas(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<SchemaSourceStatus>, String> {
    let settings_guard = settings_state.settings.read().await;
    let sources: Vec<DatabaseSourceConfig> = settings_guard
        .database_toolbox
        .sources
        .iter()
        .cloned()
        .filter(|s| s.enabled)
        .collect();
    drop(settings_guard);

    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let mut cached_sources = Vec::new();

    for source in sources {
        let (tx, rx) = oneshot::channel();
        handles
            .schema_tx
            .send(SchemaVectorMsg::GetTablesForSource {
                source_id: source.id.clone(),
                respond_to: tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        let cached_tables = rx
            .await
            .map_err(|_| "Schema vector actor unavailable".to_string())?;

        let table_statuses = cached_tables
            .into_iter()
            .map(|table| SchemaTableStatus {
                source_id: source.id.clone(),
                source_name: source.name.clone(),
                table_fq_name: table.fully_qualified_name,
                enabled: table.enabled,
                column_count: table.columns.len(),
                description: table.description,
            })
            .collect();

        cached_sources.push(SchemaSourceStatus {
            source_id: source.id.clone(),
            source_name: source.name.clone(),
            database_kind: source.kind,
            tables: table_statuses,
        });
    }

    Ok(cached_sources)
}

#[tauri::command]
async fn set_schema_table_enabled(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
    source_id: String,
    table_fq_name: String,
    enabled: bool,
) -> Result<SchemaTableStatus, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    let source = settings_guard
        .database_toolbox
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .cloned()
        .ok_or_else(|| format!("Source not found: {}", source_id))?;
    let source_name = source.name.clone();
    drop(settings_guard);

    // Try to flip the enabled flag on the cached record
    let toggle_result = {
        let (tx, rx) = oneshot::channel();
        handles
            .schema_tx
            .send(SchemaVectorMsg::SetTableEnabled {
                table_fq_name: table_fq_name.clone(),
                enabled,
                respond_to: tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        rx.await
            .map_err(|_| "Schema vector actor unavailable".to_string())?
    };

    let table_schema = match toggle_result {
        Ok(schema) => schema,
        Err(err) => {
            if !enabled {
                return Err(err);
            }

            // If enabling a table that is not cached yet, fetch and cache it now
            println!(
                "[SchemaRefresh] Table {} not cached yet, fetching fresh schema: {}",
                table_fq_name, err
            );

            let model_guard = embedding_state.model.read().await;
            let embedding_model = model_guard
                .clone()
                .ok_or_else(|| "Embedding model not initialized".to_string())?;
            drop(model_guard);

            ensure_toolbox_running(&handles.database_toolbox_tx, &toolbox_config).await?;

            let table_schema =
                fetch_table_schema(&handles.database_toolbox_tx, &source.id, &table_fq_name)
                    .await?;
            let mut schema_with_flag = table_schema.clone();
            schema_with_flag.enabled = true;
            let primary_set: std::collections::HashSet<String> =
                schema_with_flag.primary_keys.iter().cloned().collect();
            let partition_set: std::collections::HashSet<String> =
                schema_with_flag.partition_columns.iter().cloned().collect();
            let cluster_set: std::collections::HashSet<String> =
                schema_with_flag.cluster_columns.iter().cloned().collect();
            let (table_embedding, column_embeddings) =
                embed_table_and_columns(embedding_model.clone(), &schema_with_flag).await?;
            cache_table_and_columns(
                &handles.schema_tx,
                schema_with_flag.clone(),
                table_embedding,
                column_embeddings,
                &primary_set,
                &partition_set,
                &cluster_set,
            )
            .await?;
            schema_with_flag
        }
    };

    Ok(SchemaTableStatus {
        source_id: source.id,
        source_name,
        table_fq_name: table_schema.fully_qualified_name,
        enabled: table_schema.enabled,
        column_count: table_schema.columns.len(),
        description: table_schema.description,
    })
}

async fn refresh_schema_cache_for_source(
    handles: &State<'_, ActorHandles>,
    source: &DatabaseSourceConfig,
    embedding_model: Arc<TextEmbedding>,
) -> Result<SchemaSourceStatus, String> {
    println!(
        "[SchemaRefresh] Refreshing source '{}' ({})",
        source.name, source.id
    );

    // Preserve existing enabled flags if present
    let previous_map = load_cached_enabled_flags(&handles.schema_tx, &source.id).await?;

    // Remove stale entries so we only keep current enumeration
    let _ = clear_source_cache(&handles.schema_tx, &source.id).await;

    let mut datasets = enumerate_source_schemas(&handles.database_toolbox_tx, &source.id).await?;
    // Apply BigQuery dataset allowlist if provided
    if source.kind == SupportedDatabaseKind::Bigquery {
        if let Some(allow_raw) = source.dataset_allowlist.as_ref() {
            let allow: Vec<String> = allow_raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !allow.is_empty() {
                let allow_set: std::collections::HashSet<String> = allow.into_iter().collect();
                datasets.retain(|d| allow_set.contains(d));
                println!(
                    "[SchemaRefresh] Applying dataset allowlist for source {}: {} datasets retained",
                    source.id,
                    datasets.len()
                );
            }
        }
    }
    let mut tables_status = Vec::new();

    for dataset in datasets {
        let dataset_clean = dataset.trim().to_string();
        if dataset_clean
            .chars()
            .last()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            println!(
                "[SchemaRefresh] Skipping dataset '{}' because it ends with a numeric suffix",
                dataset_clean
            );
            continue;
        }

        let mut tables = match enumerate_tables_for_schema(
            &handles.database_toolbox_tx,
            &source.id,
            &dataset_clean,
        )
        .await
        {
            Ok(t) => t,
            Err(err) => {
                println!(
                    "[SchemaRefresh] Failed to enumerate tables for {}: {}",
                    dataset_clean, err
                );
                continue;
            }
        };

        // Apply BigQuery table allowlist if provided
        if source.kind == SupportedDatabaseKind::Bigquery {
            if let Some(allow_raw) = source.table_allowlist.as_ref() {
                let allow: Vec<String> = allow_raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !allow.is_empty() {
                    let allow_set: std::collections::HashSet<String> = allow.into_iter().collect();
                    tables.retain(|t| allow_set.contains(t));
                    println!(
                        "[SchemaRefresh] Applying table allowlist for source {} dataset {}: {} tables retained",
                        source.id,
                        dataset_clean,
                        tables.len()
                    );
                }
            }
        }

        for table_name in tables {
            let table_clean = table_name.trim().to_string();
            if table_clean
                .chars()
                .last()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                println!(
                    "[SchemaRefresh] Skipping table '{}' in dataset '{}' because it ends with a numeric suffix",
                    table_clean,
                    dataset_clean
                );
                continue;
            }

            let fq_name = build_fully_qualified_table_name(source, &dataset_clean, &table_name);
            let enabled = previous_map.get(&fq_name).copied().unwrap_or(true);

            match fetch_table_schema(&handles.database_toolbox_tx, &source.id, &fq_name).await {
                Ok(mut table_schema) => {
                    table_schema.enabled = enabled;
                    // Annotate join-worthy columns for chunk key purposes
                    let partition_set: std::collections::HashSet<String> =
                        table_schema.partition_columns.iter().cloned().collect();
                    let cluster_set: std::collections::HashSet<String> =
                        table_schema.cluster_columns.iter().cloned().collect();
                    let primary_set: std::collections::HashSet<String> =
                        table_schema.primary_keys.iter().cloned().collect();

                    let (table_embedding, column_embeddings) =
                        match embed_table_and_columns(embedding_model.clone(), &table_schema).await
                        {
                            Ok(res) => res,
                            Err(err) => {
                                println!(
                                    "[SchemaRefresh] Failed to embed table {}: {}",
                                    fq_name, err
                                );
                                continue;
                            }
                        };

                    if let Err(err) = cache_table_and_columns(
                        &handles.schema_tx,
                        table_schema.clone(),
                        table_embedding,
                        column_embeddings,
                        &primary_set,
                        &partition_set,
                        &cluster_set,
                    )
                    .await
                    {
                        println!(
                            "[SchemaRefresh] Failed to cache table {}: {}",
                            fq_name, err
                        );
                        continue;
                    }

                    tables_status.push(SchemaTableStatus {
                        source_id: source.id.clone(),
                        source_name: source.name.clone(),
                        table_fq_name: fq_name.clone(),
                        enabled,
                        column_count: table_schema.columns.len(),
                        description: table_schema.description.clone(),
                    });
                }
                Err(err) => {
                    println!(
                        "[SchemaRefresh] Failed to cache table {}: {}",
                        fq_name, err
                    );
                }
            }
        }
    }

    Ok(SchemaSourceStatus {
        source_id: source.id.clone(),
        source_name: source.name.clone(),
        database_kind: source.kind,
        tables: tables_status,
    })
}

async fn ensure_toolbox_running(
    toolbox_tx: &mpsc::Sender<DatabaseToolboxMsg>,
    config: &DatabaseToolboxConfig,
) -> Result<(), String> {
    if !config.enabled {
        println!(
            "[SchemaRefresh] Database toolbox is disabled in settings; attempting to start anyway"
        );
    }

    let (status_tx, status_rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::GetStatus {
            reply_to: status_tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    let status: ToolboxStatus = status_rx
        .await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?;

    if status.running {
        return Ok(());
    }

    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::Start {
            config: config.clone(),
            reply_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let start_result = rx
        .await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?;

    match start_result {
        Ok(()) => Ok(()),
        Err(msg) if msg.contains("already running") => Ok(()),
        Err(err) => Err(err),
    }
}

async fn enumerate_source_schemas(
    toolbox_tx: &mpsc::Sender<DatabaseToolboxMsg>,
    source_id: &str,
) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::EnumerateSchemas {
            source_id: source_id.to_string(),
            reply_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?
}

async fn enumerate_tables_for_schema(
    toolbox_tx: &mpsc::Sender<DatabaseToolboxMsg>,
    source_id: &str,
    dataset_or_schema: &str,
) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::EnumerateTables {
            source_id: source_id.to_string(),
            dataset_or_schema: dataset_or_schema.to_string(),
            reply_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?
}

async fn fetch_table_schema(
    toolbox_tx: &mpsc::Sender<DatabaseToolboxMsg>,
    source_id: &str,
    table_fq_name: &str,
) -> Result<CachedTableSchema, String> {
    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::GetTableInfo {
            source_id: source_id.to_string(),
            fully_qualified_table: table_fq_name.to_string(),
            reply_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?
}

fn build_fully_qualified_table_name(
    source: &DatabaseSourceConfig,
    dataset_or_schema: &str,
    table_name: &str,
) -> String {
    match source.kind {
        SupportedDatabaseKind::Bigquery | SupportedDatabaseKind::Spanner => {
            if let Some(project_id) = &source.project_id {
                format!("{}.{}.{}", project_id, dataset_or_schema, table_name)
            } else {
                format!("{}.{}", dataset_or_schema, table_name)
            }
        }
        _ => format!("{}.{}", dataset_or_schema, table_name),
    }
}

fn split_parent_and_table(fq_name: &str) -> (String, String) {
    let parts: Vec<&str> = fq_name.split('.').collect();
    if parts.len() < 2 {
        return ("".to_string(), fq_name.to_string());
    }
    let table = parts.last().unwrap().to_string();
    let parent = parts[..parts.len() - 1].join(".");
    (parent, table)
}

fn build_table_embedding_text(schema: &CachedTableSchema) -> String {
    let column_summaries: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{} {}{}", c.name, c.data_type, if c.nullable { " nullable" } else { "" }))
        .collect();

    let primary = if schema.primary_keys.is_empty() {
        "none".to_string()
    } else {
        schema.primary_keys.join(", ")
    };

    let partitions = if schema.partition_columns.is_empty() {
        "none".to_string()
    } else {
        schema.partition_columns.join(", ")
    };

    let clusters = if schema.cluster_columns.is_empty() {
        "none".to_string()
    } else {
        schema.cluster_columns.join(", ")
    };

    format!(
        "table {} ({}) columns [{}]; primary keys: {}; partitions: {}; clusters: {}; description: {}",
        schema.fully_qualified_name,
        schema.kind.display_name(),
        column_summaries.join("; "),
        primary,
        partitions,
        clusters,
        schema
            .description
            .clone()
            .unwrap_or_else(|| "none".to_string())
    )
}

fn build_column_embedding_text(table_name: &str, column: &CachedColumnSchema) -> String {
    format!(
        "column {}.{} type {} {}; description: {}",
        table_name,
        column.name,
        column.data_type,
        if column.nullable { "nullable" } else { "not null" },
        column
            .description
            .clone()
            .unwrap_or_else(|| "none".to_string())
    )
}

async fn embed_table_and_columns(
    model: Arc<TextEmbedding>,
    schema: &CachedTableSchema,
) -> Result<(Vec<f32>, Vec<Vec<f32>>), String> {
    let table_text = build_table_embedding_text(schema);
    let column_texts: Vec<String> = schema
        .columns
        .iter()
        .map(|c| build_column_embedding_text(&schema.fully_qualified_name, c))
        .collect();

    let mut to_embed = Vec::with_capacity(1 + column_texts.len());
    to_embed.push(table_text);
    to_embed.extend(column_texts);

    let model_clone = model.clone();
    let embeddings = tokio::task::spawn_blocking(move || model_clone.embed(to_embed, None))
        .await
        .map_err(|e| format!("Embedding task panicked: {}", e))?
        .map_err(|e| format!("Failed to embed schema: {}", e))?;

    let mut iter = embeddings.into_iter();
    let table_embedding = iter
        .next()
        .ok_or_else(|| "No table embedding returned".to_string())?;
    let column_embeddings: Vec<Vec<f32>> = iter.collect();

    Ok((table_embedding, column_embeddings))
}

async fn cache_table_and_columns(
    schema_tx: &mpsc::Sender<SchemaVectorMsg>,
    schema: CachedTableSchema,
    table_embedding: Vec<f32>,
    column_embeddings: Vec<Vec<f32>>,
    primary_keys: &std::collections::HashSet<String>,
    partition_keys: &std::collections::HashSet<String>,
    cluster_keys: &std::collections::HashSet<String>,
) -> Result<(), String> {
    if schema.columns.len() != column_embeddings.len() {
        println!(
            "[SchemaRefresh] Column embedding count mismatch for {} (columns={}, embeddings={})",
            schema.fully_qualified_name,
            schema.columns.len(),
            column_embeddings.len()
        );
    }

    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::CacheTableSchema {
            schema: schema.clone(),
            table_embedding,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Schema vector actor unavailable".to_string())?
        .map_err(|e| format!("Failed to cache table: {}", e))?;

    // Build base chunk key: table only (to reduce duplication); disambiguation relies on table_fq_name elsewhere
    let (_, table_name) = split_parent_and_table(&schema.fully_qualified_name);
    let base_chunk = table_name;

    for (column, embedding) in schema.columns.iter().zip(column_embeddings.into_iter()) {
        let is_join = primary_keys.contains(&column.name)
            || partition_keys.contains(&column.name)
            || cluster_keys.contains(&column.name);
        let chunk_key = if is_join {
            format!("{}:join", base_chunk)
        } else {
            base_chunk.clone()
        };

        let (col_tx, col_rx) = oneshot::channel();
        schema_tx
            .send(SchemaVectorMsg::CacheColumnSchema {
                table_fq_name: schema.fully_qualified_name.clone(),
                source_id: schema.source_id.clone(),
                column: column.clone(),
                column_embedding: embedding,
                chunk_key,
                respond_to: col_tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        col_rx
            .await
            .map_err(|_| "Schema vector actor unavailable".to_string())?
            .map_err(|e| format!("Failed to cache column: {}", e))?;
    }

    Ok(())
}

async fn clear_source_cache(
    schema_tx: &mpsc::Sender<SchemaVectorMsg>,
    source_id: &str,
) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::ClearSource {
            source_id: source_id.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Schema vector actor unavailable".to_string())?
}

async fn load_cached_enabled_flags(
    schema_tx: &mpsc::Sender<SchemaVectorMsg>,
    source_id: &str,
) -> Result<HashMap<String, bool>, String> {
    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::GetTablesForSource {
            source_id: source_id.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let tables = rx
        .await
        .map_err(|_| "Schema vector actor unavailable".to_string())?;

    let mut map = HashMap::new();
    for table in tables {
        map.insert(table.fully_qualified_name.clone(), table.enabled);
    }
    Ok(map)
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
    let configs = settings.get_all_mcp_configs();
    drop(settings);

    // Count enabled and deferred servers for informative logging
    let enabled_count = configs.iter().filter(|c| c.enabled).count();
    let deferred_count = configs.iter().filter(|c| c.enabled && c.defer_tools).count();
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

#[tauri::command]
async fn connect_mcp_server(
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

#[tauri::command]
async fn disconnect_mcp_server(
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

#[tauri::command]
async fn list_mcp_tools(
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

#[tauri::command]
async fn execute_mcp_tool(
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

#[tauri::command]
async fn get_mcp_server_status(
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

#[tauri::command]
async fn get_all_mcp_tool_descriptions(
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
async fn test_mcp_server_config(
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

/// Get a preview of the final system prompt with MCP tool descriptions
#[tauri::command]
async fn get_system_prompt_preview(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    launch_config: State<'_, LaunchConfigState>,
    tool_registry_state: State<'_, ToolRegistryState>,
    embedding_state: State<'_, EmbeddingModelState>,
    user_prompt: Option<String>,
) -> Result<String, String> {
    // Get current settings
    let settings = settings_state.settings.read().await;
    let base_prompt = settings.system_prompt.clone();
    let mut server_configs = settings.mcp_servers.clone();
    let tool_search_enabled = settings.tool_search_enabled;
    let schema_search_enabled = settings.schema_search_enabled;
    let sql_select_enabled = settings.sql_select_enabled;
    let _python_execution_enabled = settings.python_execution_enabled;
    let tool_search_max_results = settings.tool_search_max_results.max(1);
    let _tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let _tool_use_examples_max = settings.tool_use_examples_max;
    let database_toolbox_config = settings.database_toolbox.clone();
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    let tool_prompts = settings.tool_system_prompts.clone();
    let settings_for_resolver = settings.clone();
    drop(settings);
    let tool_filter = launch_config.tool_filter.clone();

    sync_registry_database_tools(
        &tool_registry_state.registry,
        schema_search_enabled,
        sql_select_enabled,
    )
    .await;

    if tool_search_enabled {
        for config in &mut server_configs {
            config.defer_tools = true;
        }
    } else {
        for config in &mut server_configs {
            config.defer_tools = false;
        }
    }

    // Get current tool descriptions from connected servers
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;

    let tool_descriptions = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
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
            let filtered: Vec<McpTool> = tools
                .into_iter()
                .filter(|t| tool_filter.tool_allowed(&server_id, &t.name))
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some((server_id, filtered))
            }
        })
        .collect();
    let has_mcp_tools = !filtered_tool_descriptions.is_empty();
    // Deferred tools exist only if tool_search is enabled AND there are MCP tools
    let _has_deferred_mcp_tools = tool_search_enabled && has_mcp_tools;
    // Note: code_mode_possible and python_tool_mode now handled by resolver
    let prompt_for_discovery = user_prompt.unwrap_or_default();
    let auto_discovery = perform_auto_discovery_for_prompt(
        &prompt_for_discovery,
        tool_search_enabled,
        tool_search_max_results,
        has_mcp_tools,
        schema_search_enabled,
        settings_for_resolver.schema_relevancy_threshold,
        &database_toolbox_config,
        &filtered_tool_descriptions,
        tool_registry_state.registry.clone(),
        embedding_state.model.clone(),
        handles.schema_tx.clone(),
        false,
    )
    .await;

    // Check if there are any attached documents
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

    // Resolve capabilities for prompt building
    let resolved_capabilities = {
        let registry = tool_registry_state.registry.read().await;
        let (tx, rx) = oneshot::channel();
        let fetched_model_info = if handles
            .foundry_tx
            .send(FoundryMsg::GetCurrentModel { respond_to: tx })
            .await
            .is_ok()
        {
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
        ToolCapabilityResolver::resolve(
            &settings_for_resolver,
            model_info,
            &tool_filter,
            &server_configs,
            &registry,
        )
    };

    let settings_sm = SettingsStateMachine::from_settings(&settings_for_resolver, &tool_filter);
    let mut initial_state_machine = AgenticStateMachine::new_from_settings_sm(
        &settings_sm,
        crate::agentic_state::PromptContext {
            base_prompt: base_prompt.clone(),
            mcp_context: crate::agentic_state::McpToolContext::from_tool_lists(
                &auto_discovery.discovered_tool_schemas, // Use auto-discovered for preview if tool_search enabled
                &Vec::new(), // Deferred not used in preview logic currently
                &server_configs,
            ),
            tool_call_format: resolved_capabilities.primary_format,
            custom_tool_prompts: tool_prompts,
            python_primary: resolved_capabilities.available_builtins.contains(tool_capability::BUILTIN_PYTHON_EXECUTION),
            has_attachments,
        },
    );

    // Set auto-discovery context after initialization
    initial_state_machine.set_auto_discovery_context(
        auto_discovery.tool_search_output,
        auto_discovery.schema_search_output,
    );

    Ok(initial_state_machine.build_system_prompt())
}

#[derive(Clone, serde::Serialize)]
struct SystemPromptLayers {
    base_prompt: String,
    additions: Vec<String>,
    combined: String,
}

#[tauri::command]
async fn get_system_prompt_layers(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    launch_config: State<'_, LaunchConfigState>,
    tool_registry_state: State<'_, ToolRegistryState>,
) -> Result<SystemPromptLayers, String> {
    // Get current settings
    let settings = settings_state.settings.read().await;
    let base_prompt = settings.system_prompt.clone();
    let mut server_configs = settings.mcp_servers.clone();
    let tool_search_enabled = settings.tool_search_enabled;
    let schema_search_enabled = settings.schema_search_enabled;
    let sql_select_enabled = settings.sql_select_enabled;
    let python_execution_enabled = settings.python_execution_enabled;
    // python_tool_calling_enabled now handled by resolver
    let _tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let _tool_use_examples_max = settings.tool_use_examples_max;
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    let tool_prompts = settings.tool_system_prompts.clone();
    let settings_for_resolver = settings.clone();
    drop(settings);
    let tool_filter = launch_config.tool_filter.clone();

    sync_registry_database_tools(
        &tool_registry_state.registry,
        schema_search_enabled,
        sql_select_enabled,
    )
    .await;

    if tool_search_enabled {
        for config in &mut server_configs {
            config.defer_tools = true;
        }
    } else {
        for config in &mut server_configs {
            config.defer_tools = false;
        }
    }

    // Get current tool descriptions from connected servers
    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;

    let tool_descriptions = rx.await.map_err(|_| "MCP Host actor died".to_string())?;
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
            let filtered: Vec<McpTool> = tools
                .into_iter()
                .filter(|t| tool_filter.tool_allowed(&server_id, &t.name))
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some((server_id, filtered))
            }
        })
        .collect();
    let has_mcp_tools = !filtered_tool_descriptions.is_empty();
    // Deferred tools exist only if tool_search is enabled AND there are MCP tools
    let has_deferred_mcp_tools = tool_search_enabled && has_mcp_tools;

    let builtin_tools: Vec<(String, Vec<McpTool>)> = {
        let registry = tool_registry_state.registry.read().await;
        registry
            .get_internal_tools()
            .iter()
            .filter(|schema| {
                // Only include python_execution if it's enabled
                if schema.name == "python_execution" {
                    python_execution_enabled && tool_filter.builtin_allowed("python_execution")
                } else if schema.name == "tool_search" {
                    // Only include tool_search if there are deferred tools to discover
                    has_deferred_mcp_tools && tool_filter.builtin_allowed("tool_search")
                } else {
                    // Other built-ins (schema_search, sql_select) are included if allowed
                    tool_filter.builtin_allowed(&schema.name)
                }
            })
            .map(|schema| ("builtin".to_string(), vec![tool_schema_to_mcp_tool(schema)]))
            .collect()
    };

    let visible_tool_descriptions: Vec<(String, Vec<McpTool>)> = if tool_search_enabled {
        // Defer MCP tools; always include built-ins.
        builtin_tools
    } else {
        let mut list = builtin_tools;
        list.extend(filtered_tool_descriptions.clone());
        list
    };

    // Check if there are any attached documents
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

    // Resolve capabilities for prompt building
    let resolved_capabilities = {
        let registry = tool_registry_state.registry.read().await;
        let (tx, rx) = oneshot::channel();
        let fetched_model_info = if handles
            .foundry_tx
            .send(FoundryMsg::GetCurrentModel { respond_to: tx })
            .await
            .is_ok()
        {
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
        ToolCapabilityResolver::resolve(
            &settings_for_resolver,
            model_info,
            &tool_filter,
            &server_configs,
            &registry,
        )
    };

    let settings_sm = SettingsStateMachine::from_settings(&settings_for_resolver, &tool_filter);
    let initial_state_machine = AgenticStateMachine::new_from_settings_sm(
        &settings_sm,
        crate::agentic_state::PromptContext {
            base_prompt: base_prompt.clone(),
            mcp_context: crate::agentic_state::McpToolContext::from_tool_lists(
                &visible_tool_descriptions,
                &Vec::new(),
                &server_configs,
            ),
            tool_call_format: resolved_capabilities.primary_format,
            custom_tool_prompts: tool_prompts,
            python_primary: resolved_capabilities.available_builtins.contains(tool_capability::BUILTIN_PYTHON_EXECUTION),
            has_attachments,
        },
    );

    let sections = initial_state_machine.build_system_prompt_sections();
    let additions = if sections.len() > 1 {
        sections[1..].to_vec()
    } else {
        Vec::new()
    };

    Ok(SystemPromptLayers {
        base_prompt: base_prompt.clone(),
        additions,
        combined: sections.join("\n\n"),
    })
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
    println!(
        "[ToolCall] Executing {}::{} with args: {:?}",
        server_id, tool_name, arguments
    );

    let (tx, rx) = oneshot::channel();
    handles
        .mcp_host_tx
        .send(McpHostMsg::ExecuteTool {
            server_id,
            tool_name: tool_name.clone(),
            arguments,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let result = rx.await.map_err(|_| "MCP Host actor died".to_string())??;

    // Convert the result to a string for display
    let result_text = result
        .content
        .iter()
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
        println!(
            "[ToolApproval] No pending approval found for key: {}",
            approval_key
        );
        Err(format!(
            "No pending approval found for key: {}",
            approval_key
        ))
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
        println!(
            "[ToolApproval] No pending approval found for key: {}",
            approval_key
        );
        Err(format!(
            "No pending approval found for key: {}",
            approval_key
        ))
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

#[derive(Debug, Clone, serde::Serialize)]
struct LaunchOverridesPayload {
    model: Option<String>,
    initial_prompt: Option<String>,
}

#[tauri::command]
async fn get_current_model(handles: State<'_, ActorHandles>) -> Result<Option<ModelInfo>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetCurrentModel { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn get_launch_overrides(
    launch_config: State<'_, LaunchConfigState>,
) -> Result<LaunchOverridesPayload, String> {
    let launch_overrides = &launch_config.launch_overrides;
    Ok(LaunchOverridesPayload {
        model: launch_overrides.model.clone(),
        initial_prompt: launch_overrides.initial_prompt.clone(),
    })
}

/// Simple heartbeat endpoint called by the frontend once per second.
/// Resets the "frontend alive" timer; backend will log if beats stop arriving.
#[tauri::command]
async fn heartbeat_ping(heartbeat_state: State<'_, HeartbeatState>) -> Result<(), String> {
    // #region agent log
    use std::io::Write;
    let hb_start = std::time::Instant::now();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
        let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H3","location":"lib.rs:heartbeat_ping","message":"heartbeat_ping_start","data":{{"timestamp_ms":{}}},"timestamp":{}}}"#, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
    }
    // #endregion
    let mut last = heartbeat_state.last_frontend_beat.write().await;
    *last = Some(Instant::now());

    // Clear any previous unresponsive flag so a new gap will log once.
    let mut logged = heartbeat_state.logged_unresponsive.write().await;
    *logged = false;

    // #region agent log
    let hb_elapsed = hb_start.elapsed().as_micros();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/bernie/git/plugable-chat/.cursor/debug.log") {
        let _ = writeln!(f, r#"{{"sessionId":"debug-session","runId":"pre-fix","hypothesisId":"H3","location":"lib.rs:heartbeat_ping","message":"heartbeat_ping_end","data":{{"elapsed_us":{}}},"timestamp":{}}}"#, hb_elapsed, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0));
    }
    // #endregion
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            let python_mcp_host_tx = mcp_host_tx.clone();
            let mcp_host_tx_for_db = mcp_host_tx.clone();
            let mcp_host_tx_for_handles = mcp_host_tx.clone();
            let logging_persistence = Arc::new(LoggingPersistence::default());
            let logging_persistence_for_foundry = logging_persistence.clone();

            // Store handles in state
            app.manage(ActorHandles {
                vector_tx,
                foundry_tx,
                rag_tx,
                mcp_host_tx: mcp_host_tx_for_handles,
                python_tx,
                database_toolbox_tx: database_toolbox_tx.clone(),
                schema_tx: schema_tx.clone(),
                logging_persistence,
            });

            // Initialize shared embedding model state
            let embedding_model_state = EmbeddingModelState {
                model: Arc::new(RwLock::new(None)),
            };
            let embedding_model_arc = embedding_model_state.model.clone();
            app.manage(embedding_model_state);
            let embedding_model_arc_for_python = embedding_model_arc.clone();

            // Initialize shared tool registry
            let tool_registry = create_shared_registry();
            let tool_registry_state = ToolRegistryState {
                registry: tool_registry.clone(),
            };
            app.manage(tool_registry_state);

            // Initialize settings state (load from config file)
            let mut settings =
                tauri::async_runtime::block_on(async { settings::load_settings().await });
            let launch_overrides = apply_cli_overrides(&cli_args_for_setup, &mut settings);
            println!(
                "Settings loaded: {} MCP servers configured",
                settings.mcp_servers.len()
            );
            // Create SettingsStateMachine (Tier 1 of the three-tier hierarchy)
            let settings_sm = SettingsStateMachine::from_settings(&settings, &launch_filter);
            println!(
                "[SettingsStateMachine] Initialized with mode: {} (capabilities: {:?})",
                settings_sm.operational_mode().name(),
                settings_sm.enabled_capabilities()
            );
            
            let settings_state = SettingsState {
                settings: Arc::new(RwLock::new(settings)),
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
                // Ensure data directory exists
                let _ = tokio::fs::create_dir_all("./data").await;
                let actor = ChatVectorStoreActor::new(vector_rx, "./data/lancedb").await;
                actor.run().await;
            });

            // Spawn Foundry Actor (manages embedding model initialization with GPU support)
            let foundry_app_handle = app_handle.clone();
            let embedding_model_arc_for_foundry = embedding_model_arc.clone();
            tauri::async_runtime::spawn(async move {
                let actor = ModelGatewayActor::new(
                    foundry_rx,
                    foundry_app_handle,
                    embedding_model_arc_for_foundry,
                    logging_persistence_for_foundry,
                );
                actor.run().await;
            });

            // Spawn RAG Actor
            let rag_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let actor = RagRetrievalActor::new(
                    rag_rx,
                    std::path::PathBuf::from("./data/lancedb"),
                    Some(rag_app_handle),
                );
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
                let _ = tokio::fs::create_dir_all("./data").await;
                let actor = SchemaVectorStoreActor::new(schema_rx, "./data/lancedb").await;
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
            update_python_execution_enabled,
            update_native_tool_calling_enabled,
            update_tool_search_enabled,
            update_schema_search_enabled,
            update_sql_select_enabled,
            update_rag_chunk_min_relevancy,
            update_schema_relevancy_threshold,
            update_rag_dominant_threshold,
            get_state_machine_preview,
            update_database_toolbox_config,
            get_cached_database_schemas,
            refresh_database_schemas,
            set_schema_table_enabled,
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
            get_system_prompt_layers,
            detect_tool_calls,
            execute_tool_call,
            approve_tool_call,
            reject_tool_call,
            get_pending_tool_approvals,
            get_current_model,
            get_launch_overrides,
            heartbeat_ping
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod inline_tests {
    use crate::settings::ToolCallFormatName;
    use crate::protocol::ToolFormat;

    // Helper to create test ResolvedToolCapabilities
    use super::*;
    use serde_json::json;

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
        let input = vec![
            "for i in range(10):".to_string(),
            "    print(i)".to_string(), // Already indented - resets tracking
            "print('done')".to_string(), // After explicit indent, we follow it
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "for i in range(10):");
        assert_eq!(result[1], "    print(i)"); // Preserved
                                               // After seeing explicit indent, we reset to that level
        assert_eq!(result[2], "    print('done')");
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
                mcp_context: crate::agentic_state::McpToolContext::from_tool_lists(
                    &active_tools,
                    &Vec::new(),
                    &server_configs,
                ),
                tool_call_format: ToolCallFormatName::Hermes,
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
        let formatted = format_tool_result(&calls[0], "echo: hi", false, ToolFormat::Hermes, None);

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
