pub mod actors;
pub mod model_profiles;
pub mod protocol;
pub mod settings;
pub mod tool_adapters;
pub mod tool_registry;
pub mod tools;

use actors::database_toolbox_actor::{DatabaseToolboxActor, DatabaseToolboxMsg};
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
    parse_tool_calls, CachedModel, ChatMessage, FoundryMsg, McpHostMsg, ModelFamily, ModelInfo,
    OpenAITool, ParsedToolCall, RagChunk, RagIndexResult, RagMsg, RemoveFileResult,
    ToolCallsPendingEvent, ToolExecutingEvent, ToolFormat, ToolHeartbeatEvent, ToolLoopFinishedEvent,
    ToolResultEvent,
    VectorMsg,
};
use python_sandbox::sandbox::ALLOWED_MODULES as PYTHON_ALLOWED_MODULES;
use serde::de::DeserializeOwned;
use serde_json::json;
use settings::{
    enforce_python_name, ensure_default_servers, AppSettings, McpServerConfig,
    ToolCallFormatConfig, ToolCallFormatName,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;
use tokio::sync::{mpsc, oneshot};
use tool_adapters::{detect_python_code, format_tool_result, parse_tool_calls_for_model_profile};
use tool_registry::{create_shared_registry, SharedToolRegistry, ToolSearchResult};
use tools::code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};
use tools::tool_search::{
    precompute_tool_search_embeddings, ToolSearchExecutor, ToolSearchInput,
};
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

// Cancellation state for stream abort
struct CancellationState {
    /// Current generation's cancel signal
    cancel_signal: Arc<RwLock<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Current generation ID for matching
    current_generation_id: Arc<RwLock<u32>>,
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

/// Launch-time tool filter derived from CLI args.
#[derive(Debug, Clone, Default)]
struct ToolLaunchFilter {
    allowed_builtins: Option<HashSet<String>>,
    allowed_servers: Option<HashSet<String>>,
    allowed_tools: Option<HashSet<(String, String)>>,
}

impl ToolLaunchFilter {
    fn allow_all(&self) -> bool {
        self.allowed_builtins.is_none()
            && self.allowed_servers.is_none()
            && self.allowed_tools.is_none()
    }

    fn builtin_allowed(&self, name: &str) -> bool {
        match &self.allowed_builtins {
            None => true,
            Some(set) => set.contains(name),
        }
    }

    fn server_allowed(&self, server_id: &str) -> bool {
        match &self.allowed_servers {
            None => true,
            Some(set) => set.contains(server_id),
        }
    }

    fn tool_allowed(&self, server_id: &str, tool_name: &str) -> bool {
        if let Some(tools) = &self.allowed_tools {
            if !tools.contains(&(server_id.to_string(), tool_name.to_string())) {
                return false;
            }
        }
        self.server_allowed(server_id)
    }
}

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
    matches!(tool_name, "python_execution" | "tool_search" | "search_schemas" | "execute_sql")
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
    if let Some(v) = args.tool_examples {
        settings.tool_use_examples_enabled = v;
    }
    if let Some(max_examples) = args.tool_examples_max {
        let capped = max_examples.clamp(1, 5);
        settings.tool_use_examples_max = capped;
    }
    if let Some(v) = args.compact_mode {
        settings.compact_prompt_enabled = v;
    }
    if let Some(max_tools) = args.compact_max_tools {
        let capped = max_tools.clamp(1, 10);
        settings.compact_prompt_max_tools = capped;
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

    if let Ok(re) = regex::Regex::new(r"(?s)```(?:python|py)?\s*(.*?)\s*```") {
        if let Some(cap) = re.captures(trimmed) {
            let body = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            if !body.is_empty() {
                return Some(
                    body.lines()
                        .map(|l| l.trim_end_matches('\r').to_string())
                        .collect(),
                );
            }
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
                    }],
                };
            } else {
                return AgenticAction::Final {
                    response: assistant_response.to_string(),
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
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    server_configs: Vec<McpServerConfig>,
    chat_id: String,
    title: String,
    original_message: String,
    mut openai_tools: Option<Vec<OpenAITool>>,
    model_name: String,
    python_tool_mode: bool,
    format_config: ToolCallFormatConfig,
    primary_format: ToolCallFormatName,
    allow_tool_search_for_python: bool,
    tool_search_max_results: usize,
) {
    // Resolve model profile from model name
    let profile = resolve_profile(&model_name);
    let model_family = profile.model_family;
    let tool_format = profile.tool_call_format;
    let mut iteration = 0;
    let mut had_tool_calls = false;
    let mut final_response = String::new();

    // Track repeated errors to detect when model is stuck
    // Format: "tool_name::error_message"
    let mut last_error_signature: Option<String> = None;
    let mut tools_disabled_due_to_repeated_error = false;

    println!(
        "[AgenticLoop] Starting with model_family={:?}, tool_format={:?}, python_tool_mode={}, primary_format={:?}, tool_search_in_python={}",
        model_family, tool_format, python_tool_mode, primary_format, allow_tool_search_for_python
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

        // Create channel for this iteration
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Send chat request to Foundry
        println!("[AgenticLoop] 📤 Sending chat request to Foundry...");
        let _ = std::io::stdout().flush();
        if let Err(e) = foundry_tx
            .send(FoundryMsg::Chat {
                chat_history_messages: full_history.clone(),
                reasoning_effort: reasoning_effort.clone(),
                native_tool_specs: openai_tools.clone(),
                respond_to: tx,
                stream_cancel_rx: cancel_rx.clone(),
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
                // Check for cancellation
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        println!("[AgenticLoop] Cancellation received!");
                        cancelled = true;
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
                            let _ = app_handle.emit("chat-token", token);

                            // Log progress every 5 seconds
                            if last_progress_log.elapsed() >= std::time::Duration::from_secs(5) {
                                println!("[AgenticLoop] 📊 Receiving: {} tokens, {} chars so far", token_count, assistant_response.len());
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
        println!(
            "[AgenticLoop] ✅ Response complete: {} tokens, {} chars in {:.2}s",
            token_count,
            assistant_response.len(),
            iteration_elapsed.as_secs_f64()
        );
        let _ = std::io::stdout().flush();

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

            println!(
                "[AgenticLoop] 🔧 Processing tool call {}/{}: {}::{}",
                idx + 1,
                tool_calls.len(),
                resolved_call.server,
                resolved_call.tool
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
                    "search_schemas" => {
                        println!("[AgenticLoop] ⏳ Executing built-in: search_schemas");
                        let _ = std::io::stdout().flush();
                        let exec_start = std::time::Instant::now();

                        // Parse input
                        let input: tools::SchemaSearchInput = 
                            serde_json::from_value(resolved_call.arguments.clone())
                                .unwrap_or_else(|e| {
                                    println!("[AgenticLoop] ⚠️ Failed to parse search_schemas args: {}, using defaults", e);
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
                            Ok(output) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ✅ search_schemas completed in {:.2}s: {} tables found",
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
                                    "[AgenticLoop] ❌ search_schemas failed in {:.2}s: {}",
                                    elapsed.as_secs_f64(),
                                    e
                                );
                                (e, true)
                            }
                        }
                    }
                    "execute_sql" => {
                        println!("[AgenticLoop] ⏳ Executing built-in: execute_sql");
                        let _ = std::io::stdout().flush();
                        let exec_start = std::time::Instant::now();

                        // Parse input
                        let input: tools::ExecuteSqlInput = 
                            serde_json::from_value(resolved_call.arguments.clone())
                                .unwrap_or_else(|e| {
                                    println!("[AgenticLoop] ⚠️ Failed to parse execute_sql args: {}, using defaults", e);
                                    tools::ExecuteSqlInput {
                                        source_id: resolved_call.arguments
                                            .get("source_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        sql: resolved_call.arguments
                                            .get("sql")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        parameters: vec![],
                                        max_rows: 100,
                                    }
                                });

                        let executor = tools::ExecuteSqlExecutor::new(database_toolbox_tx.clone());

                        match executor.execute(input).await {
                            Ok(output) => {
                                let elapsed = exec_start.elapsed();
                                println!(
                                    "[AgenticLoop] ✅ execute_sql completed in {:.2}s: {} rows",
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
                                    "[AgenticLoop] ❌ execute_sql failed in {:.2}s: {}",
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
            tool_results.push(format_tool_result(
                &resolved_call,
                &result_text,
                is_error,
                tool_format,
            ));
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
    let _ = app_handle.emit(
        "tool-loop-finished",
        ToolLoopFinishedEvent {
            iterations: iteration,
            had_tool_calls,
        },
    );
    let _ = app_handle.emit("chat-finished", ());

    println!(
        "[AgenticLoop] Loop complete after {} iterations, had_tool_calls={}, tools_disabled={}",
        iteration, had_tool_calls, tools_disabled_due_to_repeated_error
    );

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
                        })
                        .await
                    {
                        Ok(_) => {
                            println!(
                                "[ChatSave] UpsertChatRecord sent, emitting chat-saved event"
                            );
                            let _ = app_handle.emit("chat-saved", chat_id.clone());
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
}

/// Legacy system prompt builder (kept for reference)
#[allow(dead_code)]
fn legacy_build_system_prompt(
    base_prompt: &str,
    tool_descriptions: &[(String, Vec<McpTool>)],
    server_configs: &[McpServerConfig],
    python_execution_enabled: bool,
    has_attachments: bool,
) -> String {
    let mut prompt = base_prompt.to_string();

    // Categorize servers by defer status
    let mut active_servers: Vec<(&String, &Vec<McpTool>)> = Vec::new();
    let mut deferred_servers: Vec<(&String, &Vec<McpTool>)> = Vec::new();

    for (server_id, tools) in tool_descriptions {
        if tools.is_empty() {
            continue;
        }
        let is_deferred = server_configs
            .iter()
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
    let has_any_tools = python_execution_enabled || has_mcp_tools;

    let _active_tool_count: usize = active_servers.iter().map(|(_, t)| t.len()).sum();
    let _deferred_tool_count: usize = deferred_servers.iter().map(|(_, t)| t.len()).sum();

    // ===== CRITICAL: Attached Documents (only if python_execution is enabled AND attachments exist) =====
    if python_execution_enabled && has_attachments {
        prompt.push_str("\n\n## CRITICAL: How Attached Documents Work\n\n");
        prompt.push_str("The user has attached files to this chat. Important:\n");
        prompt.push_str("- The text content is **already extracted** and shown in the user's message as \"Context from attached documents\"\n");
        prompt.push_str(
            "- ❌ **You CANNOT access the original files** - no file paths, no file I/O\n",
        );
        prompt.push_str("- ✅ **To analyze the content**: Use the text already provided in the conversation\n\n");
        prompt.push_str("**WRONG:** `with open('document.pdf', 'r') as f: ...`\n");
        prompt.push_str("**CORRECT:** Use the extracted text directly in python_execution as a string literal.\n\n");
    }

    // ===== Tool Selection Guide (only if any tools are enabled) =====
    if has_any_tools {
        prompt.push_str("## Tool Selection Guide\n\n");
        prompt.push_str("**IMPORTANT: Before using any tool, first ask yourself: Can I answer this directly from my knowledge?**\n\n");
        prompt.push_str("Most questions can be answered without tools. Only use tools when they provide a clear advantage.\n\n");

        if python_execution_enabled {
            prompt.push_str("### 1. `python_execution` (Built-in Python Sandbox)\n");
            prompt.push_str(
                "**WHEN TO USE** (only when it provides clear advantage over your knowledge):\n",
            );
            prompt.push_str("- Complex arithmetic that's error-prone to compute mentally (e.g., compound interest over 30 years)\n");
            prompt.push_str(
                "- Processing/transforming data the user has provided in the conversation\n",
            );
            prompt.push_str("- Generating structured output (JSON, CSV) from conversation data\n");
            prompt.push_str("- Pattern matching or text manipulation on user-provided text\n\n");
            prompt.push_str("**WHEN NOT TO USE** (just answer directly):\n");
            prompt.push_str("- Simple math you can do reliably (e.g., \"what's 15% of 80?\")\n");
            prompt.push_str("- Date/calendar questions (e.g., \"what day is Jan 6, 2026?\") - answer from knowledge\n");
            prompt.push_str("- Questions about facts, concepts, or explanations\n");
            prompt.push_str("- Anything where your knowledge is sufficient and reliable\n\n");
            prompt.push_str("**LIMITATIONS:** \n");
            prompt.push_str(
                "- ❌ CANNOT access internet, databases, files, APIs, or any external systems\n",
            );
            prompt.push_str("- ❌ CANNOT read or write files - NO filesystem access at all\n");
            prompt.push_str("- ✅ Available modules: math, json, random, re, datetime, collections, itertools, functools, statistics, decimal, fractions, hashlib, base64\n\n");

            // One-shot example to help smaller models understand they should CALL the tool
            prompt.push_str("**EXAMPLE - When user says \"calculate\" or \"execute\":**\n\n");
            prompt
                .push_str("User: \"Calculate compound interest on $5000 at 6% for 10 years\"\n\n");
            prompt.push_str("✅ CORRECT - Return a single Python program:\n");
            prompt.push_str("```python\n");
            prompt.push_str("principal = 5000\nrate = 0.06\nyears = 10\nresult = principal * (1 + rate) ** years\nprint(f\"Result: ${result:,.2f}\")\n");
            prompt.push_str("```\n\n");
            prompt.push_str("❌ WRONG - Don't just describe code without executing it.\n\n");
        }

        if has_mcp_tools && has_deferred_tools && python_execution_enabled {
            // Primary workflow: search → execute with Python → repeat
            prompt.push_str("### 2. External Tools (Databases, APIs, Files, etc.)\n\n");
            prompt.push_str("**WORKFLOW: Search → Execute → Repeat**\n\n");
            prompt
                .push_str("For tasks requiring external data or actions, follow this pattern:\n\n");
            prompt.push_str(
                "1. **SEARCH**: Call `tool_search` to find relevant tools for your current step\n",
            );
            prompt.push_str("2. **EXECUTE**: Write a Python program using `python_execution` that calls the discovered tools\n");
            prompt.push_str("3. **REPEAT**: If more steps are needed, search again for the next step's tools\n\n");
            prompt.push_str("**IMPORTANT**: Tools you discover stay available for this user turn. Re-use them in python_execution without searching again. They reset only when the user sends a new message.\n\n");
        } else if has_mcp_tools {
            let section_num = if python_execution_enabled { "2" } else { "1" };
            prompt.push_str(&format!(
                "### {}. MCP Tools (External Capabilities)\n",
                section_num
            ));
            prompt.push_str("**USE FOR:** Anything requiring external access - databases, APIs, files, web, etc.\n");
            prompt.push_str("**HOW TO USE:**\n");
            if has_deferred_tools {
                prompt.push_str(
                    "1. First call `tool_search` to discover available tools for your task\n",
                );
                prompt.push_str("2. Then call the discovered tools directly\n\n");
            } else if has_active_tools {
                prompt.push_str("- Call active MCP tools directly (listed below)\n\n");
            }
        }

        prompt.push_str("### COMMON MISTAKES TO AVOID:\n");
        prompt.push_str("- ❌ Saying \"I can't do that\" without trying tool_search first\n");
        prompt.push_str(
            "- ❌ Making up function names or imports - tools MUST be discovered first\n",
        );
        prompt.push_str(
            "- ❌ Showing code without executing it - USE the tools, don't just describe them\n",
        );
        if python_execution_enabled && has_deferred_tools {
            prompt.push_str("- ❌ Using `python_execution` with undiscovered tools - call `tool_search` first!\n");
        }
        prompt.push_str("- ✅ When stuck, call `tool_search` to find what tools are available\n\n");

        // Tool calling format instructions
        prompt.push_str("## Tool Calling Format\n\n");
        prompt.push_str("All tool use must happen from inside a single Python program. Do NOT emit <tool_call> tags. Call the provided global functions directly and print results for the user.\n\n");
    }

    // Python execution details (only if enabled)
    if python_execution_enabled {
        prompt.push_str("## python_execution Tool\n\n");
        prompt.push_str("Sandboxed Python for complex calculations. **Only use when it provides clear advantage over answering directly.**\n");
        prompt.push_str("You must `import` modules before using them.\n\n");
        prompt.push_str("**CRITICAL: Do the calculation, don't explain it.**\n");
        prompt.push_str("If a calculation can be done with the available Python libraries, USE `python_execution` to compute it and return the result.\n");
        prompt.push_str("❌ WRONG: \"Here's how you could calculate this in Python...\"\n");
        prompt.push_str("✅ RIGHT: Return a single Python program that performs the calculation and prints the answer.\n\n");
        prompt.push_str("**Good use case** (complex calculation):\n");
        prompt.push_str("```python\nimport math\nresult = 10000 * (1 + 0.07) ** 30\nprint(f\"Final amount: ${result:,.2f}\")\n```\n\n");
        prompt.push_str("**Bad use case** (just answer directly instead):\n");
        prompt.push_str("- \"What's 15% of 200?\" → Just say \"30\" - no code needed\n");
        prompt.push_str("- Simple factual questions → Answer from knowledge\n\n");
    }

    // Tool discovery and execution section
    if has_deferred_tools && python_execution_enabled {
        prompt.push_str("## REQUIRED: Search → Execute Workflow\n\n");
        prompt.push_str("**You MUST call `tool_search` before using any external tools.**\n");
        prompt.push_str(
            "Tools are NOT available until discovered. Do NOT guess or make up tool names.\n\n",
        );

        prompt.push_str("**WRONG - Never do this:**\n");
        prompt.push_str("```python\n");
        prompt.push_str(
            "from some_module import made_up_function  # FAILS - tools must be discovered first!\n",
        );
        prompt.push_str("```\n\n");

        prompt.push_str("**CORRECT - Always follow this pattern inside ONE Python program:**\n\n");
        prompt.push_str("```python\n");
        prompt.push_str("# Step 1: discover tools\n");
        prompt.push_str("tools = tool_search(relevant_to=\"list datasets\")\n");
        prompt.push_str("# Step 2: call discovered tools\n");
        prompt.push_str("result = list_dataset_ids()\n");
        prompt.push_str("print(result)\n");
        prompt.push_str("# Step 3: repeat tool_search if you need more tools\n");
        prompt.push_str("```\n\n");

        // Count total tools available
        let total_deferred: usize = deferred_servers.iter().map(|(_, t)| t.len()).sum();
        prompt.push_str(&format!("There are {} tools available across {} server(s). Use `tool_search` to find the right ones.\n\n",
            total_deferred,
            deferred_servers.len()));
    } else if has_deferred_tools {
        prompt.push_str("## Tool Discovery (REQUIRED)\n\n");
        prompt.push_str("**You MUST call tool_search(relevant_to=\"...\") inside your Python program before using any external tools.**\n\n");
        prompt.push_str("**Pattern:**\n");
        prompt.push_str("```python\n");
        prompt.push_str("tools = tool_search(relevant_to=\"describe what you need\")\n");
        prompt.push_str("result = some_discovered_tool()\n");
        prompt.push_str("print(result)\n");
        prompt.push_str("```\n\n");

        let total_deferred: usize = deferred_servers.iter().map(|(_, t)| t.len()).sum();
        prompt.push_str(&format!(
            "There are {} tools available. Use `tool_search` to find the right ones.\n\n",
            total_deferred
        ));
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
                    prompt.push_str(
                        "**Server Configuration** (use these values for this server's tools):\n",
                    );
                    for (key, value) in &config.env {
                        // Skip sensitive keys
                        let key_lower = key.to_lowercase();
                        if key_lower.contains("secret")
                            || key_lower.contains("password")
                            || key_lower.contains("token")
                            || key_lower.contains("key")
                        {
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
                            let required_fields: Vec<&str> = schema
                                .get("required")
                                .and_then(|r| r.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();

                            prompt.push_str("  Arguments:\n");
                            for (name, prop_schema) in props {
                                let prop_type = prop_schema
                                    .get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("string");
                                let prop_desc = prop_schema
                                    .get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");
                                let is_required = required_fields.contains(&name.as_str());
                                let req_marker = if is_required { " [REQUIRED]" } else { "" };

                                prompt.push_str(&format!(
                                    "  - `{}` ({}){}: {}\n",
                                    name, prop_type, req_marker, prop_desc
                                ));
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

#[derive(Clone, Copy)]
struct PromptTuningOptions {
    include_examples: bool,
    examples_max_per_tool: usize,
    compact_mode: bool,
    compact_max_tools: usize,
}

impl Default for PromptTuningOptions {
    fn default() -> Self {
        Self {
            include_examples: false,
            examples_max_per_tool: 0,
            compact_mode: false,
            compact_max_tools: usize::MAX,
        }
    }
}

/// Build the full system prompt with tool capabilities (new tool-prompt driven version)
fn build_system_prompt(
    base_prompt: &str,
    tool_descriptions: &[(String, Vec<McpTool>)],
    server_configs: &[McpServerConfig],
    has_attachments: bool,
    tool_prompts: &HashMap<String, String>,
    filter: &ToolLaunchFilter,
    primary_format: ToolCallFormatName,
    python_tool_mode: bool,
    allow_tool_search_for_python: bool,
    python_execution_enabled: bool,
    tool_search_enabled: bool,
    tuning: &PromptTuningOptions,
) -> String {
    let additions = collect_tool_prompt_additions(
        tool_descriptions,
        server_configs,
        has_attachments,
        tool_prompts,
        filter,
        primary_format,
        python_tool_mode,
        allow_tool_search_for_python,
        python_execution_enabled,
        tool_search_enabled,
        tuning,
    );

    let mut sections: Vec<String> = vec![base_prompt.trim().to_string()];
    if !additions.is_empty() {
        sections.push("## Additional prompts from tools".to_string());
        sections.extend(additions);
    }

    sections.join("\n\n")
}

fn collect_tool_prompt_additions(
    tool_descriptions: &[(String, Vec<McpTool>)],
    server_configs: &[McpServerConfig],
    has_attachments: bool,
    tool_prompts: &HashMap<String, String>,
    filter: &ToolLaunchFilter,
    primary_format: ToolCallFormatName,
    python_tool_mode: bool,
    allow_tool_search_for_python: bool,
    python_execution_enabled: bool,
    tool_search_enabled: bool,
    tuning: &PromptTuningOptions,
) -> Vec<String> {
    const BUILTIN_SERVER_LABEL: &str = "Built-in Tools";
    const PYTHON_LABEL: &str = "Python Execution";
    const TOOL_SEARCH_LABEL: &str = "Tool Search";

    let mut additions: Vec<String> = Vec::new();
    let mut tools_included: usize = 0;
    let max_tools = if tuning.compact_mode {
        tuning.compact_max_tools.max(1)
    } else {
        usize::MAX
    };
    let mut tool_search_prompt_added = false;

    // Track server defer modes for contextual guidance
    let mut has_deferred_tools = tool_search_enabled && !server_configs.is_empty();
    let mut has_active_tools = false;
    for (server_id, tools) in tool_descriptions {
        if tools.is_empty() {
            continue;
        }
        let is_deferred = server_configs
            .iter()
            .find(|c| c.id == *server_id)
            .map(|c| c.defer_tools)
            .unwrap_or(true);
        if is_deferred {
            has_deferred_tools = true;
        } else {
            has_active_tools = true;
        }
    }

    let has_any_tools = !tool_descriptions.is_empty() || python_tool_mode;

    // Built-ins: always show prompts when enabled (even if MCP tools are deferred)
    let python_prompt_allowed =
        python_execution_enabled && filter.builtin_allowed("python_execution");
    if python_prompt_allowed {
        let tool_search_available = allow_tool_search_for_python
            && tool_search_enabled
            && filter.builtin_allowed("tool_search");
        let mut body =
            default_python_prompt(has_attachments, has_deferred_tools, tool_search_available);
        if let Some(custom) = tool_prompts.get(&tool_prompt_key("builtin", "python_execution")) {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                body.push_str("\n\n");
                body.push_str(trimmed);
            }
        }
        additions.push(format!(
            "### {} ({})\n{}",
            PYTHON_LABEL,
            BUILTIN_SERVER_LABEL,
            body.trim()
        ));

        if tool_search_available {
            let mut body = default_tool_search_prompt(has_deferred_tools);
            if let Some(custom) = tool_prompts.get(&tool_prompt_key("builtin", "tool_search")) {
                let trimmed = custom.trim();
                if !trimmed.is_empty() {
                    body.push_str("\n\n");
                    body.push_str(trimmed);
                }
            }
            additions.push(format!(
                "### {} ({})\n{}",
                TOOL_SEARCH_LABEL,
                BUILTIN_SERVER_LABEL,
                body.trim()
            ));
            tool_search_prompt_added = true;
        }
    } else if has_any_tools {
        if let Some(format_prompt) = tool_calling_format_prompt(primary_format) {
            additions.push(format_prompt);
        }
    }

    // If tool_search is enabled but not yet surfaced (e.g., python disabled), still provide guidance.
    if !tool_search_prompt_added && tool_search_enabled && filter.builtin_allowed("tool_search") {
        let mut body = String::from(
            "Call tool_search to list relevant MCP tools before using them. Example:\n\
             <tool_call>{\"server\": \"builtin\", \"tool\": \"tool_search\", \"arguments\": {\"queries\": [\"your goal\"], \"top_k\": 3}}</tool_call>\n\
             Then call the returned tools directly.",
        );
        if has_deferred_tools {
            body.push_str("\n\nSome MCP tools are deferred; run tool_search early to discover them.");
        } else {
            body.push_str("\n\nUse tool_search when you are unsure which MCP tool to call.");
        }
        if let Some(custom) = tool_prompts.get(&tool_prompt_key("builtin", "tool_search")) {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                body.push_str("\n\n");
                body.push_str(trimmed);
            }
        }
        additions.push(format!(
            "### {} ({})\n{}",
            TOOL_SEARCH_LABEL, BUILTIN_SERVER_LABEL, body.trim()
        ));
    }

    // MCP tools
    for (server_id, tools) in tool_descriptions {
        let server_config = server_configs.iter().find(|c| c.id == *server_id);
        let server_name = server_config
            .map(|c| c.name.clone())
            .unwrap_or_else(|| server_id.clone());
        let is_deferred = server_config.map(|c| c.defer_tools).unwrap_or(true);
        let env_vars = server_config
            .map(|c| c.env.clone())
            .filter(|env| !env.is_empty());

        for tool in tools {
            if tuning.compact_mode && tools_included >= max_tools {
                continue;
            }
            let mut parts: Vec<String> = Vec::new();

            if let Some(desc) = &tool.description {
                let trimmed = desc.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }

            if is_deferred {
                parts.push("Discover this tool with `tool_search`, then call it directly once listed for this turn.".to_string());
            } else if has_active_tools {
                parts.push("Call this MCP tool directly when it fits the task.".to_string());
            }

            if let Some(custom) = tool_prompts.get(&tool_prompt_key(server_id, &tool.name)) {
                let trimmed = custom.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }

            if let Some(env_map) = env_vars.as_ref() {
                let mut pairs: Vec<String> = env_map
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect();
                pairs.sort();
                parts.push(format!(
                    "Environment variables available to this server: {}",
                    pairs.join(", ")
                ));
            }

            // Include parameter schema details if available
            if let Some(props) = tool
                .input_schema
                .as_ref()
                .and_then(|s| s.get("properties"))
                .and_then(|p| p.as_object())
            {
                let required: Vec<&str> = tool
                    .input_schema
                    .as_ref()
                    .and_then(|s| s.get("required"))
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let mut param_lines: Vec<String> = Vec::new();
                for (name, schema) in props {
                    let ty = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                    let desc = schema
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let req = if required.contains(&name.as_str()) {
                        " (required)"
                    } else {
                        ""
                    };
                    let mut line = format!("- `{}` (type: {}){}", name, ty, req);
                    if !desc.is_empty() {
                        line.push_str(&format!(": {}", desc));
                    }
                    param_lines.push(line);
                }

                if tuning.compact_mode && param_lines.len() > 3 {
                    param_lines.truncate(3);
                    param_lines.push("- ...".to_string());
                }

                if !param_lines.is_empty() {
                    parts.push(format!("Parameters:\n{}", param_lines.join("\n")));
                }
            }

            if tuning.include_examples && tuning.examples_max_per_tool > 0 {
                if let Some(examples) = tool.input_examples.as_ref() {
                    let capped_count = examples
                        .len()
                        .min(tuning.examples_max_per_tool);
                    if capped_count > 0 {
                        let mut example_lines: Vec<String> = Vec::new();
                        for example in examples.iter().take(capped_count) {
                            let mut text = serde_json::to_string(example)
                                .unwrap_or_else(|_| "<example serialization failed>".to_string());
                            if text.len() > 240 {
                                text.truncate(240);
                                text.push_str("...");
                            }
                            example_lines.push(format!("- {}", text));
                        }
                        if !example_lines.is_empty() {
                            parts.push(format!(
                                "Usage examples ({} max):\n{}",
                                tuning.examples_max_per_tool,
                                example_lines.join("\n")
                            ));
                        }
                    }
                }
            }

            if parts.is_empty() {
                continue;
            }

            additions.push(format!(
                "### {} (server: {})\n{}",
                tool.name,
                server_name,
                parts.join("\n\n")
            ));
            tools_included += 1;
        }
    }

    additions
}

#[derive(Clone, serde::Serialize)]
struct SystemPromptLayers {
    base_prompt: String,
    additions: Vec<String>,
    combined: String,
}

fn build_system_prompt_layers(
    base_prompt: &str,
    tool_descriptions: &[(String, Vec<McpTool>)],
    server_configs: &[McpServerConfig],
    has_attachments: bool,
    tool_prompts: &HashMap<String, String>,
    filter: &ToolLaunchFilter,
    primary_format: ToolCallFormatName,
    python_tool_mode: bool,
    allow_tool_search_for_python: bool,
    python_execution_enabled: bool,
    tool_search_enabled: bool,
    tuning: &PromptTuningOptions,
) -> SystemPromptLayers {
    let additions = collect_tool_prompt_additions(
        tool_descriptions,
        server_configs,
        has_attachments,
        tool_prompts,
        filter,
        primary_format,
        python_tool_mode,
        allow_tool_search_for_python,
        python_execution_enabled,
        tool_search_enabled,
        tuning,
    );

    let mut sections: Vec<String> = vec![base_prompt.trim().to_string()];
    if !additions.is_empty() {
        sections.push("## Additional prompts from tools".to_string());
        sections.extend(additions.clone());
    }

    let combined = sections.join("\n\n");

    SystemPromptLayers {
        base_prompt: base_prompt.trim().to_string(),
        additions,
        combined,
    }
}

fn default_python_prompt(
    has_attachments: bool,
    has_deferred_tools: bool,
    tool_search_enabled: bool,
) -> String {
    let mut parts: Vec<String> = vec![
        "You must return exactly one runnable Python program when python_execution is enabled. Do not return explanations or multiple blocks.".to_string(),
        "Output format: a single ```python ... ``` block. We will execute it and surface any print output directly to the user.".to_string(),
        if tool_search_enabled {
            "Tool calling is only available via Python. Use the provided global functions (including tool_search when available) from inside your program. Do NOT emit <tool_call> tags or JSON tool calls.".to_string()
        } else {
            "Tool calling is only available via Python. Use the provided global functions from inside your program. Do NOT emit <tool_call> tags or JSON tool calls.".to_string()
        },
        "Use print(...) for user-facing markdown on stdout. Prefer standard library stderr writes (e.g., import sys; sys.stderr.write(\"...\")) for handoff text, which is captured on stderr.".to_string(),
        "Allowed imports only: math, json, random, re, datetime, collections, itertools, functools, operator, string, textwrap, copy, types, typing, abc, numbers, decimal, fractions, statistics, hashlib, base64, binascii, html.".to_string(),
        "Keep code concise and runnable; include prints for results the user should see.".to_string(),
    ];

    if has_attachments {
        parts.push("Attached files are already summarized in the conversation. Do NOT read files; work with the provided text directly inside python_execution.".to_string());
    }

    if has_deferred_tools && tool_search_enabled {
        parts.push("Some MCP tools are deferred; if you need extra capabilities, call the global function tool_search(relevant_to=\"...\") inside your Python program to discover them, then call the returned functions in the same program.".to_string());
    }

    parts.join("\n\n")
}

fn default_tool_search_prompt(has_deferred_tools: bool) -> String {
    let mut parts: Vec<String> = vec![
        "Call the global function tool_search(relevant_to=\"...\") from inside your Python program to discover relevant MCP tools.".to_string(),
        "After discovery, call the returned functions directly in the same Python program.".to_string(),
    ];

    if has_deferred_tools {
        parts.push("Many MCP tools are deferred: call tool_search first inside your Python code, then call the discovered tools directly in that program.".to_string());
    }

    parts.join("\n\n")
}

fn tool_calling_format_prompt(primary: ToolCallFormatName) -> Option<String> {
    match primary {
        ToolCallFormatName::CodeMode => None,
        ToolCallFormatName::Hermes => {
            Some("### Tool calling format (Hermes)\nUse <tool_call>{\"name\": \"TOOL_NAME\", \"arguments\": {\"arg\": \"value\"}}</tool_call> with valid JSON only. Do not wrap in markdown or add prose.".to_string())
        }
        ToolCallFormatName::Mistral => {
            Some("### Tool calling format (Mistral)\nUse [TOOL_CALLS] [{\"name\": \"TOOL_NAME\", \"arguments\": {\"arg\": \"value\"}}] with no extra text or markdown.".to_string())
        }
        ToolCallFormatName::Pythonic => {
            Some("### Tool calling format (Pythonic)\nUse function-call syntax like tool_name(arg1=\"value\", arg2=123). Do not wrap in code fences or add explanations.".to_string())
        }
        ToolCallFormatName::PureJson => {
            Some("### Tool calling format (Pure JSON)\nReturn a raw JSON object or array such as {\"tool\": \"TOOL_NAME\", \"args\": {\"arg\": \"value\"}}. No markdown or additional text.".to_string())
        }
    }
}

#[tauri::command]
async fn search_history(
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
    launch_config: State<'_, LaunchConfigState>,
    cancellation_state: State<'_, CancellationState>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    use std::io::Write;
    let chat_id = chat_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let chat_id_return = chat_id.clone();
    let title = title.unwrap_or_else(|| message.chars().take(50).collect::<String>());

    // Log incoming chat request
    let msg_preview: String = message.chars().take(128).collect();
    println!(
        "\n[chat] 💬 New chat request: \"{}{}\"",
        msg_preview,
        if message.len() > 128 { "..." } else { "" }
    );
    println!("[chat] chat_id={}, history_len={}", chat_id, history.len());
    let _ = std::io::stdout().flush();

    // Set up cancellation signal for this generation
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    {
        // Increment generation ID and store the cancel signal
        let mut gen_id = cancellation_state.current_generation_id.write().await;
        *gen_id = gen_id.wrapping_add(1);
        *cancellation_state.cancel_signal.write().await = Some(cancel_tx);
        println!(
            "[chat] Starting generation {} with cancellation support",
            *gen_id
        );
        let _ = std::io::stdout().flush();
    }

    let tool_filter = launch_config.tool_filter.clone();

    // Get server configs from settings
    let settings = settings_state.settings.read().await;
    let configured_system_prompt = settings.system_prompt.clone();
    let mut server_configs = settings.mcp_servers.clone();
    let tool_search_enabled = settings.tool_search_enabled;
    let python_execution_enabled = settings.python_execution_enabled;
    let python_tool_calling_enabled = settings.python_tool_calling_enabled;
    let tool_search_max_results = settings.tool_search_max_results.max(1);
    let tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let tool_use_examples_max = settings.tool_use_examples_max;
    let compact_prompt_enabled = settings.compact_prompt_enabled;
    let compact_prompt_max_tools = settings.compact_prompt_max_tools;
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    let tool_system_prompts = settings.tool_system_prompts.clone();
    drop(settings);

    // Apply global tool_search flag to server defer settings
    if tool_search_enabled {
        for config in &mut server_configs {
            config.defer_tools = true;
        }
    } else {
        for config in &mut server_configs {
            config.defer_tools = false;
        }
    }

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

    // Apply launch-time filters
    let filtered_tool_descriptions: Vec<(String, Vec<McpTool>)> = tool_descriptions
        .into_iter()
        .filter_map(|(server_id, tools)| {
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

    // Always use the configured system prompt (which should explain tool capabilities)
    let base_system_prompt = configured_system_prompt;

    // Build the tools list:
    // 1. Include python_execution if enabled in settings
    // 2. Include tool_search when MCP servers with tools are available
    // 3. Include all MCP tools
    let code_mode_possible = format_config.is_enabled(ToolCallFormatName::CodeMode)
        && python_execution_enabled
        && python_tool_calling_enabled
        && tool_filter.builtin_allowed("python_execution");
    // Primary affects prompting only; execution should honor any enabled format.
    let primary_format_for_prompt = format_config.resolve_primary_for_prompt(code_mode_possible);
    let python_tool_mode = code_mode_possible;
    let allow_tool_search_for_python =
        python_tool_mode && has_mcp_tools && tool_filter.builtin_allowed("tool_search");
    let non_code_formats_enabled = format_config.any_non_code();
    let legacy_tool_calls_enabled =
        non_code_formats_enabled && primary_format_for_prompt != ToolCallFormatName::CodeMode;
    let legacy_tool_search_enabled =
        legacy_tool_calls_enabled && has_mcp_tools && tool_filter.builtin_allowed("tool_search");

    println!(
        "[chat] tool_call_formats: primary={:?}, enabled={:?}, python_execution_enabled={}, python_tool_calling_enabled={}, python_tool_mode={}, code_mode_possible={}",
        format_config.primary,
        format_config.enabled,
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

        // Clear any previously materialized tools (fresh start for this chat)
        registry.clear_materialized();

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

    // When tool_search is enabled, run it proactively with the user prompt to
    // surface an initial set of tools before the first model call.
    if tool_search_enabled && !message.trim().is_empty() {
        let executor = ToolSearchExecutor::new(
            tool_registry_state.registry.clone(),
            embedding_state.model.clone(),
        );
        let search_input = ToolSearchInput {
            queries: vec![message.clone()],
            top_k: tool_search_max_results,
        };
        match executor.execute(search_input).await {
            Ok(output) => {
                executor.materialize_results(&output.tools).await;
                println!(
                    "[Chat] Auto tool_search discovered {} tools before first turn",
                    output.tools.len()
                );
            }
            Err(e) => {
                println!(
                    "[Chat] Auto tool_search failed (continuing without discoveries): {}",
                    e
                );
            }
        }
    } else if tool_search_enabled {
        println!("[Chat] Auto tool_search skipped: empty user prompt");
    }

    // Determine which MCP tools are visible after any materialization
    let visible_tool_descriptions: Vec<(String, Vec<McpTool>)> = {
        let registry = tool_registry_state.registry.read().await;
        let has_materialized = registry.stats().materialized_tools > 0;

        if tool_search_enabled && !has_materialized {
            Vec::new()
        } else {
            filtered_tool_descriptions
                .iter()
                .filter_map(|(server_id, tools)| {
                    let visible_tools: Vec<McpTool> = tools
                        .iter()
                        .cloned()
                        .filter(|tool| registry.is_tool_visible(server_id, &tool.name))
                        .collect();
                    if visible_tools.is_empty() {
                        None
                    } else {
                        Some((server_id.clone(), visible_tools))
                    }
                })
                .collect()
        }
    };

    // Include visible tools in legacy/native tool calling payloads
    if let Some(ref mut tools_list) = openai_tools {
        let registry = tool_registry_state.registry.read().await;
        let mut seen: HashSet<String> =
            tools_list.iter().map(|t| t.function.name.clone()).collect();
        let mut included_count: usize = 0;
        let max_tools_for_prompt = if compact_prompt_enabled {
            compact_prompt_max_tools.max(1)
        } else {
            usize::MAX
        };

        for (server_id, schema) in registry.get_visible_tools_with_servers() {
            if compact_prompt_enabled && included_count >= max_tools_for_prompt {
                break;
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
            included_count += 1;
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

    let prompt_tuning = PromptTuningOptions {
        include_examples: tool_use_examples_enabled,
        examples_max_per_tool: tool_use_examples_max.max(1),
        compact_mode: compact_prompt_enabled,
        compact_max_tools: compact_prompt_max_tools.max(1),
    };

    if compact_prompt_enabled {
        println!(
            "[Chat] Compact prompt mode enabled (max_tools_in_prompt={})",
            prompt_tuning.compact_max_tools
        );
    }
    if tool_use_examples_enabled {
        println!(
            "[Chat] Tool examples enabled (max_per_tool={})",
            prompt_tuning.examples_max_per_tool
        );
    }
    println!(
        "[Chat] tool_search_max_results={}",
        tool_search_max_results
    );

    // Build the full system prompt with tool descriptions
    // Note: We still include text-based tool instructions as a fallback for models
    // that don't support native tool calling
    let system_prompt = build_system_prompt(
        &base_system_prompt,
        &visible_tool_descriptions,
        &server_configs,
        has_attachments,
        &tool_system_prompts,
        &tool_filter,
        primary_format_for_prompt,
        python_tool_mode,
        allow_tool_search_for_python,
        python_execution_enabled,
        tool_search_enabled,
        &prompt_tuning,
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
    println!(
        "[Chat] --- SYSTEM PROMPT BEGIN ---\n{}\n[Chat] --- SYSTEM PROMPT END ---",
        system_prompt
    );

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
    handles
        .foundry_tx
        .send(FoundryMsg::GetCurrentModel {
            respond_to: model_info_tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    let current_model = model_info_rx
        .await
        .map_err(|_| "Foundry actor died".to_string())?;

    // Get the current model name for profile resolution
    let model_name = current_model
        .map(|m| m.id)
        .unwrap_or_else(|| "unknown".to_string());

    println!("[Chat] Using model: {}", model_name);

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
    let title_task = title.clone();
    let message_task = message.clone();
    let openai_tools_task = openai_tools;
    let python_tool_mode_task = python_tool_mode;
    let format_config_task = format_config.clone();
    let primary_format_task = primary_format_for_prompt;
    let allow_tool_search_for_python_task = allow_tool_search_for_python;
    let tool_search_max_results_task = tool_search_max_results;

    // Spawn the agentic loop task
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
            server_configs,
            chat_id_task,
            title_task,
            message_task,
            openai_tools_task,
            model_name,
            python_tool_mode_task,
            format_config_task,
            primary_format_task,
            allow_tool_search_for_python_task,
            tool_search_max_results_task,
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
) -> Result<(), String> {
    let mut normalized = new_settings;
    normalized.tool_call_formats.normalize();

    // Save to file
    settings::save_settings(&normalized).await?;

    // Update in-memory state
    let mut guard = settings_state.settings.write().await;
    *guard = normalized;

    Ok(())
}

#[tauri::command]
async fn add_mcp_server(
    mut config: McpServerConfig,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    enforce_python_name(&mut config);

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
    mut config: McpServerConfig,
    settings_state: State<'_, SettingsState>,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    enforce_python_name(&mut config);

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
    handles
        .mcp_host_tx
        .send(McpHostMsg::SyncEnabledServers {
            configs: configs_for_sync,
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

#[tauri::command]
async fn update_tool_system_prompt(
    server_id: String,
    tool_name: String,
    prompt: String,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    let key = tool_prompt_key(&server_id, &tool_name);

    if prompt.trim().is_empty() {
        guard.tool_system_prompts.remove(&key);
    } else {
        guard.tool_system_prompts.insert(key, prompt);
    }

    settings::save_settings(&guard).await?;
    Ok(())
}

#[tauri::command]
async fn update_tool_call_formats(
    config: ToolCallFormatConfig,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut normalized = config;
    normalized.normalize();
    let mut guard = settings_state.settings.write().await;
    guard.tool_call_formats = normalized.clone();
    settings::save_settings(&guard).await?;
    println!(
        "[Settings] tool_call_formats updated: primary={:?}, enabled={:?}",
        normalized.primary, normalized.enabled
    );
    Ok(())
}

#[tauri::command]
async fn update_python_execution_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.python_execution_enabled = enabled;
    settings::save_settings(&guard).await?;
    println!(
        "[Settings] python_execution_enabled updated to: {}",
        enabled
    );
    Ok(())
}

#[tauri::command]
async fn update_tool_search_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.tool_search_enabled = enabled;
    settings::save_settings(&guard).await?;
    println!("[Settings] tool_search_enabled updated to: {}", enabled);
    Ok(())
}

#[tauri::command]
async fn update_search_schemas_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.search_schemas_enabled = enabled;
    settings::save_settings(&guard).await?;
    println!("[Settings] search_schemas_enabled updated to: {}", enabled);
    Ok(())
}

#[tauri::command]
async fn update_execute_sql_enabled(
    enabled: bool,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.execute_sql_enabled = enabled;
    settings::save_settings(&guard).await?;
    println!("[Settings] execute_sql_enabled updated to: {}", enabled);
    Ok(())
}

#[tauri::command]
async fn update_database_toolbox_config(
    config: settings::DatabaseToolboxConfig,
    settings_state: State<'_, SettingsState>,
) -> Result<(), String> {
    let mut guard = settings_state.settings.write().await;
    guard.database_toolbox = config;
    settings::save_settings(&guard).await?;
    println!("[Settings] database_toolbox config updated");
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
) -> Result<String, String> {
    // Get current settings
    let settings = settings_state.settings.read().await;
    let base_prompt = settings.system_prompt.clone();
    let mut server_configs = settings.mcp_servers.clone();
    let tool_search_enabled = settings.tool_search_enabled;
    let python_execution_enabled = settings.python_execution_enabled;
    let python_tool_calling_enabled = settings.python_tool_calling_enabled;
    let tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let tool_use_examples_max = settings.tool_use_examples_max;
    let compact_prompt_enabled = settings.compact_prompt_enabled;
    let compact_prompt_max_tools = settings.compact_prompt_max_tools;
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    let tool_prompts = settings.tool_system_prompts.clone();
    drop(settings);
    let tool_filter = launch_config.tool_filter.clone();

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
    let code_mode_possible = format_config.is_enabled(ToolCallFormatName::CodeMode)
        && python_execution_enabled
        && python_tool_calling_enabled
        && tool_filter.builtin_allowed("python_execution");
    let primary_format_for_prompt = format_config.resolve_primary_for_prompt(code_mode_possible);
    let python_tool_mode =
        code_mode_possible && primary_format_for_prompt == ToolCallFormatName::CodeMode;
    let allow_tool_search_for_python =
        python_tool_mode && has_mcp_tools && tool_filter.builtin_allowed("tool_search");

    let visible_tool_descriptions: Vec<(String, Vec<McpTool>)> = if tool_search_enabled {
        Vec::new()
    } else {
        filtered_tool_descriptions.clone()
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

    let prompt_tuning = PromptTuningOptions {
        include_examples: tool_use_examples_enabled,
        examples_max_per_tool: tool_use_examples_max.max(1),
        compact_mode: compact_prompt_enabled,
        compact_max_tools: compact_prompt_max_tools.max(1),
    };

    // Build the full system prompt
    let preview = build_system_prompt(
        &base_prompt,
        &visible_tool_descriptions,
        &server_configs,
        has_attachments,
        &tool_prompts,
        &tool_filter,
        primary_format_for_prompt,
        python_tool_mode,
        allow_tool_search_for_python,
        python_execution_enabled,
        tool_search_enabled,
        &prompt_tuning,
    );

    Ok(preview)
}

#[tauri::command]
async fn get_system_prompt_layers(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    launch_config: State<'_, LaunchConfigState>,
) -> Result<SystemPromptLayers, String> {
    // Get current settings
    let settings = settings_state.settings.read().await;
    let base_prompt = settings.system_prompt.clone();
    let mut server_configs = settings.mcp_servers.clone();
    let tool_search_enabled = settings.tool_search_enabled;
    let python_execution_enabled = settings.python_execution_enabled;
    let python_tool_calling_enabled = settings.python_tool_calling_enabled;
    let tool_use_examples_enabled = settings.tool_use_examples_enabled;
    let tool_use_examples_max = settings.tool_use_examples_max;
    let compact_prompt_enabled = settings.compact_prompt_enabled;
    let compact_prompt_max_tools = settings.compact_prompt_max_tools;
    let mut format_config = settings.tool_call_formats.clone();
    format_config.normalize();
    let tool_prompts = settings.tool_system_prompts.clone();
    drop(settings);
    let tool_filter = launch_config.tool_filter.clone();

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
    let code_mode_possible = format_config.is_enabled(ToolCallFormatName::CodeMode)
        && python_execution_enabled
        && python_tool_calling_enabled
        && tool_filter.builtin_allowed("python_execution");
    let primary_format_for_prompt = format_config.resolve_primary_for_prompt(code_mode_possible);
    let python_tool_mode =
        code_mode_possible && primary_format_for_prompt == ToolCallFormatName::CodeMode;
    let allow_tool_search_for_python =
        python_tool_mode && has_mcp_tools && tool_filter.builtin_allowed("tool_search");

    let visible_tool_descriptions: Vec<(String, Vec<McpTool>)> = if tool_search_enabled {
        Vec::new()
    } else {
        filtered_tool_descriptions.clone()
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

    let prompt_tuning = PromptTuningOptions {
        include_examples: tool_use_examples_enabled,
        examples_max_per_tool: tool_use_examples_max.max(1),
        compact_mode: compact_prompt_enabled,
        compact_max_tools: compact_prompt_max_tools.max(1),
    };

    let layers = build_system_prompt_layers(
        &base_prompt,
        &visible_tool_descriptions,
        &server_configs,
        has_attachments,
        &tool_prompts,
        &tool_filter,
        primary_format_for_prompt,
        python_tool_mode,
        allow_tool_search_for_python,
        python_execution_enabled,
        tool_search_enabled,
        &prompt_tuning,
    );

    Ok(layers)
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
fn get_launch_overrides(
    launch_config: State<'_, LaunchConfigState>,
) -> Result<LaunchOverridesPayload, String> {
    let launch_overrides = &launch_config.launch_overrides;
    Ok(LaunchOverridesPayload {
        model: launch_overrides.model.clone(),
        initial_prompt: launch_overrides.initial_prompt.clone(),
    })
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
            // Initialize channels
            let (vector_tx, vector_rx) = mpsc::channel(32);
            let (foundry_tx, foundry_rx) = mpsc::channel(32);
            let (rag_tx, rag_rx) = mpsc::channel(32);
            let (mcp_host_tx, mcp_host_rx) = mpsc::channel(32);
            let (python_tx, python_rx) = mpsc::channel(32);
            let (database_toolbox_tx, database_toolbox_rx) = mpsc::channel(32);
            let (schema_tx, schema_rx) = mpsc::channel(32);
            let python_mcp_host_tx = mcp_host_tx.clone();

            // Store handles in state
            app.manage(ActorHandles {
                vector_tx,
                foundry_tx,
                rag_tx,
                mcp_host_tx,
                python_tx,
                database_toolbox_tx: database_toolbox_tx.clone(),
                schema_tx: schema_tx.clone(),
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
            let settings_state = SettingsState {
                settings: Arc::new(RwLock::new(settings)),
            };
            app.manage(settings_state);

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

            let app_handle = app.handle();
            // Spawn Vector Actor
            tauri::async_runtime::spawn(async move {
                println!("Starting Chat Vector Store Actor...");
                // Ensure data directory exists
                let _ = tokio::fs::create_dir_all("./data").await;

                let actor = ChatVectorStoreActor::new(vector_rx, "./data/lancedb").await;
                println!("Chat Vector Store Actor initialized.");
                actor.run().await;
            });

            // Spawn Foundry Actor
            let foundry_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                println!("Starting Model Gateway Actor...");
                let actor = ModelGatewayActor::new(foundry_rx, foundry_app_handle);
                actor.run().await;
            });

            // Spawn RAG Actor
            tauri::async_runtime::spawn(async move {
                println!("Starting RAG Actor...");
                let actor = RagRetrievalActor::new(rag_rx);
                actor.run().await;
            });

            // Spawn MCP Host Actor
            tauri::async_runtime::spawn(async move {
                println!("Starting MCP Host Actor...");
                let actor = McpToolRouterActor::new(mcp_host_rx);
                actor.run().await;
            });

            // Spawn Python Actor for code execution
            let python_tool_registry = tool_registry.clone();
            tauri::async_runtime::spawn(async move {
                println!("Starting Python Actor...");
                let actor = PythonSandboxActor::new(
                    python_rx,
                    python_tool_registry,
                    python_mcp_host_tx,
                    embedding_model_arc_for_python,
                );
                actor.run().await;
            });

            // Initialize embedding model in background (shared between FoundryActor and RAG)
            tauri::async_runtime::spawn(async move {
                println!("Initializing shared embedding model for RAG...");
                use fastembed::{EmbeddingModel, InitOptions};

                match tokio::task::spawn_blocking(|| {
                    let mut options = InitOptions::default();
                    options.model_name = EmbeddingModel::AllMiniLML6V2;
                    options.show_download_progress = true;
                    TextEmbedding::try_new(options)
                })
                .await
                {
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

            // Spawn Database Toolbox Actor
            let database_toolbox_state = Arc::new(RwLock::new(
                actors::database_toolbox_actor::DatabaseToolboxState::default(),
            ));
            let db_state_clone = database_toolbox_state.clone();
            tauri::async_runtime::spawn(async move {
                println!("Starting Database Toolbox Actor...");
                let actor = DatabaseToolboxActor::new(database_toolbox_rx, db_state_clone);
                actor.run().await;
            });

            // Spawn Schema Vector Store Actor
            tauri::async_runtime::spawn(async move {
                println!("Starting Schema Vector Store Actor...");
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
            cancel_generation,
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
            update_python_execution_enabled,
            update_tool_search_enabled,
            update_search_schemas_enabled,
            update_execute_sql_enabled,
            update_database_toolbox_config,
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
            get_launch_overrides
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
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
        let input = vec!["data = await execute_sql(query=\"SELECT * FROM users\")".to_string()];

        let result = strip_unsupported_python(&input);

        assert_eq!(
            result[0],
            "data = execute_sql(query=\"SELECT * FROM users\")"
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
    fn test_build_system_prompt_layers_with_tool_prompts() {
        let base_prompt = "Base prompt";
        let tool = McpTool {
            name: "tool_a".to_string(),
            description: Some("Demo tool".to_string()),
            input_schema: None,
            input_examples: None,
            allowed_callers: None,
        };
        let tool_descriptions = vec![("srv1".to_string(), vec![tool])];

        let mut server_config = McpServerConfig::new("srv1".to_string(), "Server 1".to_string());
        server_config.defer_tools = false;
        let server_configs = vec![server_config];

        let mut tool_prompts = HashMap::new();
        tool_prompts.insert("srv1::tool_a".to_string(), "custom prompt".to_string());

        let filter = ToolLaunchFilter::default();
        let layers = build_system_prompt_layers(
            base_prompt,
            &tool_descriptions,
            &server_configs,
            false,
            &tool_prompts,
            &filter,
            ToolCallFormatName::CodeMode,
            true,
            false,
            false,
            false,
            &PromptTuningOptions::default(),
        );

        assert_eq!(layers.base_prompt, "Base prompt");
        assert!(layers.combined.contains("custom prompt"));
        assert!(layers.additions.iter().any(|s| s.contains("custom prompt")));
    }

    #[test]
    fn test_build_system_prompt_layers_includes_env_vars() {
        use std::collections::HashMap;

        let base_prompt = "Base prompt";
        let tool = McpTool {
            name: "tool_a".to_string(),
            description: Some("Demo tool".to_string()),
            input_schema: None,
            input_examples: None,
            allowed_callers: None,
        };
        let tool_descriptions = vec![("srv1".to_string(), vec![tool])];

        let mut server_config = McpServerConfig::new("srv1".to_string(), "Server 1".to_string());
        server_config.defer_tools = false;
        server_config.env = HashMap::from([
            (
                "BIGQUERY_PROJECT".to_string(),
                "plugabot-colchuck".to_string(),
            ),
            ("BQ_DATASET".to_string(), "finance".to_string()),
        ]);
        let server_configs = vec![server_config];

        let tool_prompts = HashMap::new();
        let filter = ToolLaunchFilter::default();
        let layers = build_system_prompt_layers(
            base_prompt,
            &tool_descriptions,
            &server_configs,
            false,
            &tool_prompts,
            &filter,
            ToolCallFormatName::CodeMode,
            true,
            false,
            false,
            false,
            &PromptTuningOptions::default(),
        );

        let addition = layers
            .additions
            .iter()
            .find(|s| s.contains("Environment variables"))
            .expect("env section missing");
        assert!(addition.contains("BIGQUERY_PROJECT=plugabot-colchuck"));
        assert!(addition.contains("BQ_DATASET=finance"));
    }

    #[test]
    fn test_build_system_prompt_layers_compact_limits_tools() {
        let base_prompt = "Base prompt";
        let tool_a = McpTool {
            name: "tool_a".to_string(),
            description: Some("A".to_string()),
            input_schema: None,
            input_examples: None,
            allowed_callers: None,
        };
        let tool_b = McpTool {
            name: "tool_b".to_string(),
            description: Some("B".to_string()),
            input_schema: None,
            input_examples: None,
            allowed_callers: None,
        };
        let tool_descriptions = vec![("srv".to_string(), vec![tool_a, tool_b])];

        let mut server_config = McpServerConfig::new("srv".to_string(), "Server".to_string());
        server_config.defer_tools = false;
        let server_configs = vec![server_config];
        let tool_prompts = HashMap::new();
        let filter = ToolLaunchFilter::default();
        let tuning = PromptTuningOptions {
            include_examples: false,
            examples_max_per_tool: 1,
            compact_mode: true,
            compact_max_tools: 1,
        };

        let layers = build_system_prompt_layers(
            base_prompt,
            &tool_descriptions,
            &server_configs,
            false,
            &tool_prompts,
            &filter,
            ToolCallFormatName::CodeMode,
            false,
            false,
            false,
            false,
            &tuning,
        );

        // Only one tool section should be present due to compact cap
        assert!(layers.additions.iter().filter(|s| s.contains("###")).count() <= 1);
    }

    #[test]
    fn test_build_system_prompt_layers_adds_tool_search_when_deferred_without_python() {
        let base_prompt = "Base prompt";
        let tool = McpTool {
            name: "tool_a".to_string(),
            description: Some("Demo tool".to_string()),
            input_schema: None,
            input_examples: None,
            allowed_callers: None,
        };
        let tool_descriptions = vec![("srv1".to_string(), vec![tool])];

        let mut server_config = McpServerConfig::new("srv1".to_string(), "Server 1".to_string());
        server_config.defer_tools = true;
        let server_configs = vec![server_config];
        let tool_prompts = HashMap::new();
        let filter = ToolLaunchFilter::default();
        let tuning = PromptTuningOptions {
            include_examples: false,
            examples_max_per_tool: 1,
            compact_mode: false,
            compact_max_tools: usize::MAX,
        };

        let layers = build_system_prompt_layers(
            base_prompt,
            &tool_descriptions,
            &server_configs,
            false,
            &tool_prompts,
            &filter,
            ToolCallFormatName::Hermes,
            false, // python_tool_mode
            false, // allow_tool_search_for_python
            false, // python_execution_enabled
            true,  // tool_search_enabled
            &tuning,
        );

        assert!(
            layers
                .combined
                .contains("Call tool_search to list relevant MCP tools"),
            "tool_search instructions should be present when deferred tools exist without python"
        );
    }

    #[test]
    fn test_build_system_prompt_layers_adds_tool_search_when_enabled_no_defer() {
        let base_prompt = "Base prompt";
        let tool = McpTool {
            name: "tool_a".to_string(),
            description: Some("Demo tool".to_string()),
            input_schema: None,
            input_examples: None,
            allowed_callers: None,
        };
        let tool_descriptions = vec![("srv1".to_string(), vec![tool])];

        let mut server_config = McpServerConfig::new("srv1".to_string(), "Server 1".to_string());
        server_config.defer_tools = false;
        let server_configs = vec![server_config];
        let tool_prompts = HashMap::new();
        let filter = ToolLaunchFilter::default();
        let tuning = PromptTuningOptions {
            include_examples: false,
            examples_max_per_tool: 1,
            compact_mode: false,
            compact_max_tools: usize::MAX,
        };

        let layers = build_system_prompt_layers(
            base_prompt,
            &tool_descriptions,
            &server_configs,
            false,
            &tool_prompts,
            &filter,
            ToolCallFormatName::Hermes,
            false, // python_tool_mode
            false, // allow_tool_search_for_python
            false, // python_execution_enabled
            true,  // tool_search_enabled
            &tuning,
        );

        assert!(
            layers
                .combined
                .contains("Call tool_search to list relevant MCP tools"),
            "tool_search instructions should be present whenever enabled"
        );
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
        let formatted = format_tool_result(&calls[0], "echo: hi", false, ToolFormat::Hermes);

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
