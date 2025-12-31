//! Command-line argument parsing and launch configuration.
//!
//! This module handles CLI argument parsing using clap, and applies
//! launch-time overrides to application settings.

use crate::app_state::LaunchOverrides;
use crate::settings::{
    enforce_python_name, ensure_default_servers, AlwaysOnTableConfig, AppSettings, McpServerConfig, ToolCallFormatName,
};
use crate::tool_capability::ToolLaunchFilter;
use clap::Parser;
use mcp_test_server::{DEFAULT_HOST as MCP_TEST_DEFAULT_HOST, DEFAULT_PORT as MCP_TEST_DEFAULT_PORT};
use serde::de::DeserializeOwned;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// CLI arguments for plugable-chat
#[derive(Parser, Debug, Clone)]
#[command(name = "plugable-chat", about = "Plugable Chat desktop app")]
pub struct CliArgs {
    /// Optional model to load on launch (non-persistent)
    #[arg(long, value_name = "MODEL", env = "PLUGABLE_MODEL")]
    pub model: Option<String>,
    /// Override global system prompt (string or @path/to/file)
    #[arg(long, value_name = "PROMPT_OR_@FILE", env = "PLUGABLE_SYSTEM_PROMPT")]
    pub system_prompt: Option<String>,
    /// Initial user prompt to send on startup (string or @path/to/file)
    #[arg(long, value_name = "PROMPT_OR_@FILE", env = "PLUGABLE_INITIAL_PROMPT")]
    pub initial_prompt: Option<String>,
    /// Enable/disable tool_search
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_TOOL_SEARCH", value_parser = clap::builder::BoolishValueParser::new())]
    pub tool_search: Option<bool>,
    /// Maximum number of tools returned by tool_search (caps auto and explicit searches)
    #[arg(long, value_name = "INT", env = "PLUGABLE_TOOL_SEARCH_MAX_RESULTS")]
    pub tool_search_max_results: Option<usize>,
    /// Enable/disable python_execution built-in
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_PYTHON_EXECUTION", value_parser = clap::builder::BoolishValueParser::new())]
    pub python_execution: Option<bool>,
    /// Enable/disable python-driven tool calling
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_PYTHON_TOOL_CALLING", value_parser = clap::builder::BoolishValueParser::new())]
    pub python_tool_calling: Option<bool>,
    /// Enable/disable native tool calling (OpenAI-compatible) when model supports it
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_NATIVE_TOOL_CALLING", value_parser = clap::builder::BoolishValueParser::new())]
    pub native_tool_calling: Option<bool>,
    /// Enable/disable inclusion of tool input_examples in prompts
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_TOOL_EXAMPLES", value_parser = clap::builder::BoolishValueParser::new())]
    pub tool_examples: Option<bool>,
    /// Maximum number of examples per tool when tool_examples is enabled
    #[arg(long, value_name = "INT", env = "PLUGABLE_TOOL_EXAMPLES_MAX")]
    pub tool_examples_max: Option<usize>,
    /// Enable compact prompt mode for small models
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_COMPACT_MODE", value_parser = clap::builder::BoolishValueParser::new())]
    pub compact_mode: Option<bool>,
    /// Maximum number of tools to surface in prompts when compact mode is on
    #[arg(long, value_name = "INT", env = "PLUGABLE_COMPACT_MAX_TOOLS")]
    pub compact_max_tools: Option<usize>,
    /// Override per-server defer_tools setting at launch
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_DEFER_TOOLS", value_parser = clap::builder::BoolishValueParser::new())]
    pub defer_tools: Option<bool>,
    /// Enable/disable legacy <tool_call> parsing
    #[arg(long, value_name = "BOOL", env = "PLUGABLE_LEGACY_TOOL_FORMAT", value_parser = clap::builder::BoolishValueParser::new())]
    pub legacy_tool_call_format: Option<bool>,
    /// Comma-separated list of tool call formats to enable (hermes,mistral,pythonic,pure_json,code_mode)
    #[arg(
        long = "tool-call-enabled",
        value_delimiter = ',',
        value_name = "FORMAT[,FORMAT...]",
        env = "PLUGABLE_TOOL_CALL_ENABLED"
    )]
    pub tool_call_enabled: Option<Vec<String>>,
    /// Primary tool call format to prompt
    #[arg(
        long = "tool-call-primary",
        value_name = "FORMAT",
        env = "PLUGABLE_TOOL_CALL_PRIMARY"
    )]
    pub tool_call_primary: Option<String>,
    /// Override per-tool system prompts (server_id::tool_name=prompt_or_@file). Use server_id=builtin for built-ins.
    #[arg(long = "tool-system-prompt", value_name = "KEY=VALUE_OR_@FILE", env = "PLUGABLE_TOOL_SYSTEM_PROMPTS", value_delimiter = None)]
    pub tool_system_prompts: Vec<String>,
    /// Replace MCP server list with JSON configs (inline JSON or @path/to/json)
    #[arg(long = "mcp-server", value_name = "JSON_OR_@FILE", env = "PLUGABLE_MCP_SERVERS", value_delimiter = None)]
    pub mcp_servers: Vec<String>,
    /// Optional allowlist of tools to expose on launch.
    /// Built-ins: python_execution, tool_search
    /// MCP tools: server_id::tool_name
    /// Servers: server_id (enables all tools from that server)
    #[arg(long, value_delimiter = ',', env = "PLUGABLE_TOOLS")]
    pub tools: Option<Vec<String>>,
    
    // ============ Always-On Configuration ============
    
    /// Always-on built-in tools (comma-separated, e.g., python_execution,sql_select)
    /// These tools are always available in every chat without explicit attachment.
    #[arg(long = "always-on-builtins", value_delimiter = ',', env = "PLUGABLE_ALWAYS_ON_BUILTINS")]
    pub always_on_builtins: Option<Vec<String>>,
    
    /// Always-on MCP tools (comma-separated, server_id::tool_name format)
    /// These tools are always available in every chat without explicit attachment.
    #[arg(long = "always-on-mcp-tools", value_delimiter = ',', env = "PLUGABLE_ALWAYS_ON_MCP_TOOLS")]
    pub always_on_mcp_tools: Option<Vec<String>>,
    
    /// Always-on database tables (comma-separated, source_id::table_fq_name format)
    /// These tables' schemas are always included in the system prompt.
    #[arg(long = "always-on-tables", value_delimiter = ',', env = "PLUGABLE_ALWAYS_ON_TABLES")]
    pub always_on_tables: Option<Vec<String>>,
    
    /// Always-on RAG files/folders (comma-separated paths)
    /// These are automatically indexed and searched for every chat.
    #[arg(long = "always-on-rag", value_delimiter = ',', env = "PLUGABLE_ALWAYS_ON_RAG")]
    pub always_on_rag: Option<Vec<String>>,
    
    /// Enable the built-in dev MCP test server (off by default)
    #[arg(
        long,
        value_name = "BOOL",
        env = "PLUGABLE_ENABLE_MCP_TEST",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub enable_mcp_test: Option<bool>,
    /// Run only the dev MCP test server (no app; blocks until exit)
    #[arg(
        long,
        value_name = "BOOL",
        env = "PLUGABLE_RUN_MCP_TEST_SERVER",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    pub run_mcp_test_server: bool,
    /// Host for the dev MCP test server when run standalone
    #[arg(long, value_name = "HOST", default_value = MCP_TEST_DEFAULT_HOST)]
    pub mcp_test_host: String,
    /// Port for the dev MCP test server when run standalone
    #[arg(long, value_name = "PORT", default_value_t = MCP_TEST_DEFAULT_PORT)]
    pub mcp_test_port: u16,
    /// Auto-run the full MCP test sweep on start (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    pub mcp_test_run_all_on_start: bool,
    /// Serve the MCP test server UI (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = true,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    pub mcp_test_serve_ui: bool,
    /// Auto-open the MCP test server UI in a browser (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = true,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    pub mcp_test_open_ui: bool,
    /// Print the recommended MCP test prompt to stdout (standalone mode)
    #[arg(
        long,
        value_name = "BOOL",
        default_value_t = true,
        value_parser = clap::builder::BoolishValueParser::new(),
        action = clap::ArgAction::Set
    )]
    pub mcp_test_print_prompt: bool,
}

/// Read a value that may be either a literal string or a @path reference to a file
pub fn read_value_or_file(raw: &str) -> Result<String, String> {
    if let Some(path) = raw.strip_prefix('@') {
        let contents = fs::read_to_string(Path::new(path))
            .map_err(|e| format!("Failed to read {}: {}", path, e))?;
        Ok(contents)
    } else {
        Ok(raw.to_string())
    }
}

/// Parse a JSON value from either inline JSON or a @path reference
pub fn parse_json_or_file<T: DeserializeOwned>(raw: &str) -> Result<T, String> {
    let data = read_value_or_file(raw)?;
    serde_json::from_str(&data).map_err(|e| format!("Failed to parse JSON: {}", e))
}

/// Parse a tool call format name from string
pub fn parse_tool_call_format(name: &str) -> Option<ToolCallFormatName> {
    match name {
        "hermes" => Some(ToolCallFormatName::Hermes),
        "mistral" => Some(ToolCallFormatName::Mistral),
        "pythonic" => Some(ToolCallFormatName::Pythonic),
        "pure_json" => Some(ToolCallFormatName::PureJson),
        "code_mode" => Some(ToolCallFormatName::CodeMode),
        _ => None,
    }
}

/// Check if a tool call is for a built-in tool (python_execution, tool_search, or database tools)
pub fn is_builtin_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "python_execution" | "tool_search" | "schema_search" | "sql_select"
    )
}

/// Parse CLI args into a launch-time tool filter
pub fn parse_tool_filter(args: &CliArgs) -> ToolLaunchFilter {
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

/// Resolve the MCP manifest path by probing current dir and parents
fn resolve_mcp_manifest() -> Option<String> {
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

/// Apply CLI overrides to settings without persisting them.
pub fn apply_cli_overrides(args: &CliArgs, settings: &mut AppSettings) -> LaunchOverrides {
    // System prompt
    if let Some(raw) = &args.system_prompt {
        match read_value_or_file(raw) {
            Ok(prompt) => settings.system_prompt = prompt,
            Err(e) => println!("[Launch] Failed to apply system_prompt override: {}", e),
        }
    }

    // Core toggles
    if let Some(v) = args.tool_search {
        if v {
            if !settings.always_on_builtin_tools.contains(&"tool_search".to_string()) {
                settings.always_on_builtin_tools.push("tool_search".to_string());
            }
        } else {
            settings.always_on_builtin_tools.retain(|t| t != "tool_search");
        }
    }
    if let Some(max_results) = args.tool_search_max_results {
        let capped = max_results.clamp(1, 20);
        settings.tool_search_max_results = capped;
    }
    if let Some(v) = args.python_execution {
        if v {
            if !settings.always_on_builtin_tools.contains(&"python_execution".to_string()) {
                settings.always_on_builtin_tools.push("python_execution".to_string());
            }
        } else {
            settings.always_on_builtin_tools.retain(|t| t != "python_execution");
        }
    }
    if let Some(v) = args.python_tool_calling {
        settings.python_tool_calling_enabled = v;
    }
    if let Some(v) = args.native_tool_calling {
        // CLI override for native tool calling - add/remove Native format
        if v {
            if !settings
                .tool_call_formats
                .enabled
                .contains(&ToolCallFormatName::Native)
            {
                settings
                    .tool_call_formats
                    .enabled
                    .insert(0, ToolCallFormatName::Native);
            }
            settings.tool_call_formats.primary = ToolCallFormatName::Native;
        } else {
            settings
                .tool_call_formats
                .enabled
                .retain(|f| *f != ToolCallFormatName::Native);
            if settings.tool_call_formats.primary == ToolCallFormatName::Native {
                settings.tool_call_formats.primary = settings
                    .tool_call_formats
                    .enabled
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

    // Always-on configuration overrides
    if let Some(builtins) = &args.always_on_builtins {
        settings.always_on_builtin_tools = builtins.clone();
        println!("[Launch] Always-on built-in tools: {:?}", builtins);
    }
    
    if let Some(mcp_tools) = &args.always_on_mcp_tools {
        settings.always_on_mcp_tools = mcp_tools.clone();
        println!("[Launch] Always-on MCP tools: {:?}", mcp_tools);
    }
    
    if let Some(tables) = &args.always_on_tables {
        settings.always_on_tables = tables
            .iter()
            .filter_map(|entry| {
                if let Some((source_id, table_fq_name)) = entry.split_once("::") {
                    Some(AlwaysOnTableConfig {
                        source_id: source_id.to_string(),
                        table_fq_name: table_fq_name.to_string(),
                    })
                } else {
                    println!("[Launch] Invalid always-on table '{}'. Expected source_id::table_fq_name", entry);
                    None
                }
            })
            .collect();
        println!("[Launch] Always-on tables: {:?}", settings.always_on_tables.len());
    }
    
    if let Some(rag_paths) = &args.always_on_rag {
        settings.always_on_rag_paths = rag_paths.clone();
        println!("[Launch] Always-on RAG paths: {:?}", rag_paths);
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
        settings.always_on_builtin_tools.retain(|t| t != "tool_search" && t != "python_execution");
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
