//! Command-line argument parsing and launch configuration.
//!
//! This module handles CLI argument parsing using clap, and applies
//! launch-time overrides to application settings.

use crate::app_state::LaunchOverrides;
use crate::settings::{
    enforce_python_name, ensure_default_servers, ensure_dev_test_server, AlwaysOnTableConfig, AppSettings, McpServerConfig, ToolCallFormatName,
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
#[command(name = "plugable-chat", about = "Plugable Chat desktop app", version = crate::version_info::cli_version())]
pub struct CliArgs {
    /// HTTPS proxy for downloads, e.g. http://proxy.example.com:8080 (non-persistent; Settings > Network saves one)
    #[arg(long, value_name = "URL", env = "PLUGABLE_HTTPS_PROXY")]
    pub https_proxy: Option<String>,
    /// Hugging Face mirror address used if the model has to come from Hugging Face (non-persistent)
    #[arg(long, value_name = "URL", env = "PLUGABLE_HF_ENDPOINT")]
    pub hf_endpoint: Option<String>,
    /// Folder holding the search model's files, used instead of downloading them (non-persistent)
    #[arg(long, value_name = "FOLDER", env = "PLUGABLE_OFFLINE_MODEL_FOLDER")]
    pub offline_model_folder: Option<String>,
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
    /// How long to wait for an MCP server to answer one request, in seconds, for every server
    /// that does not set its own request_timeout_secs (default 30).
    #[arg(long, value_name = "SECONDS", env = "PLUGABLE_MCP_TIMEOUT_SECS")]
    pub mcp_timeout_secs: Option<u64>,
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
    
    /// Tabular data files to attach for Python analysis (comma-separated paths).
    /// Supports CSV, TSV, XLS, XLSX formats. Values are pre-typed (int/float/datetime/None).
    #[arg(long = "table-file", value_delimiter = ',', env = "PLUGABLE_TABLE_FILES")]
    pub table_files: Option<Vec<String>>,
    
    /// Enable the built-in dev MCP test server (off by default)
    #[arg(
        long,
        value_name = "BOOL",
        env = "PLUGABLE_ENABLE_MCP_TEST",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub enable_mcp_test: Option<bool>,
    
    /// Enable the built-in Chicago Crimes demo database (off by default)
    #[arg(
        long,
        value_name = "BOOL",
        env = "PLUGABLE_ENABLE_DEMO_DB",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub enable_demo_db: Option<bool>,
    /// Clean-host smoke test: start normally, wait until the app is ready or has failed, write
    /// the outcome (see PLUGABLE_CHAT_TEST_STATE), and exit 0 on success or 1 on failure.
    /// Gives up after 20 minutes; override with PLUGABLE_CHAT_SMOKE_TIMEOUT_SECS.
    #[arg(long)]
    pub smoke: bool,
    /// Append one JSON line per finished chat turn (question, reply, outcome, tool-call trace) to
    /// this file, for automated tests. See turn_record.rs for the record format and exit codes.
    #[arg(long, value_name = "PATH", env = "PLUGABLE_REPLY_FILE")]
    pub reply_file: Option<String>,
    /// Exit once every launch prompt has a record in --reply-file (0 ok, 1 turn failed,
    /// 2 launch model unusable, 3 startup failed, 4 timed out). Needs --reply-file.
    #[arg(long, env = "PLUGABLE_EXIT_AFTER_REPLY")]
    pub exit_after_reply: bool,
    /// Give up on a --reply-file run after this many seconds (default 1800) and record a timeout.
    #[arg(long, value_name = "SECONDS", env = "PLUGABLE_REPLY_TIMEOUT_SECS")]
    pub reply_timeout_secs: Option<u64>,
    /// Run several prompts in one launch, each in a fresh chat after the previous one finishes.
    /// A JSON array of strings in this file (optionally written @path). They follow --initial-prompt.
    #[arg(long, value_name = "PATH", env = "PLUGABLE_PROMPTS_FILE")]
    pub prompts_file: Option<String>,
    /// Run the --prompts-file prompts as follow-ups in ONE chat (the history grows turn by turn)
    /// instead of a fresh chat each. This is how a long conversation is reproduced unattended.
    #[arg(long, env = "PLUGABLE_SAME_CHAT")]
    pub same_chat: bool,
    /// Send the launch prompts (--initial-prompt, --prompts-file) straight to this MCP tool instead of the model, named
    /// as the Model / Tool drop-down shows it: "<server> - <tool>". Records the usual --reply-file lines and works when no
    /// local model loads. The --model override is ignored so a missing model cannot end the run.
    #[arg(long, value_name = "SERVER - TOOL", env = "PLUGABLE_DIRECT_TOOL")]
    pub direct_tool: Option<String>,
    /// Open Settings on this tab at launch (models, databases, builtins, tools, system-prompt, interfaces, schemas, files,
    /// network), so a screenshot needs no input.
    #[arg(long, value_name = "TAB", env = "PLUGABLE_OPEN_SETTINGS")]
    pub open_settings: Option<String>,
    /// Preselect this tool, "<server> - <tool>", in the Model / Tool drop-down at launch (not saved).
    #[arg(long, value_name = "SERVER - TOOL", env = "PLUGABLE_SELECT_TOOL")]
    pub select_tool: Option<String>,
    /// Repair the model engine before starting: set the downloaded GPU components aside (Core
    /// downloads them again), clear crash history, the GPU-off switch and engine-blamed model
    /// blocks. The same thing Settings > Models > Repair does; for support and unattended tests.
    #[arg(long, env = "PLUGABLE_REPAIR_ENGINE")]
    pub repair_engine: bool,
    /// Write every saved chat (id, title, messages) to this JSON file and exit; exit code 1 when the
    /// chat store cannot be read. Shows what a reload would show, e.g. after the app was killed.
    #[arg(long, value_name = "FILE", env = "PLUGABLE_DUMP_CHATS")]
    pub dump_chats: Option<String>,
    /// Start with the main window hidden (for unattended runs).
    #[arg(long, env = "PLUGABLE_HIDDEN")]
    pub hidden: bool,
    /// Keep config, data (chat history) and cache under this directory instead of the platform
    /// locations, so a test run cannot touch the user's own. Downloaded models are still found.
    #[arg(long, value_name = "DIR", env = "PLUGABLE_CHAT_HOME")]
    pub chat_home: Option<String>,
    /// Never write settings to the saved config file during this launch, so a test run cannot
    /// change the user's configuration.
    #[arg(long, env = "PLUGABLE_EPHEMERAL")]
    pub ephemeral: bool,
    /// Run the GPU registration experiments and write a report with a verdict per hypothesis to
    /// DIR, then exit (support use; `scripts/support/ep-hypotheses.ps1` runs it and gathers host facts)
    #[arg(long, value_name = "DIR")]
    pub ep_probe: Option<String>,
    /// JSON file of host facts gathered by the wrapper script, folded into the probe's verdicts
    #[arg(long, value_name = "FILE")]
    pub ep_probe_facts: Option<String>,
    /// Folder holding another Foundry Local Core build to compare against the bundled one
    #[arg(long, value_name = "DIR")]
    pub ep_probe_other_core: Option<String>,
    /// Internal: one probe configuration in its own process (ours, bootstrap, stock, other-core)
    #[arg(long, value_name = "NAME", hide = true)]
    pub ep_probe_case: Option<String>,
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

/// Windows PowerShell 5.1 writes a byte-order mark with `-Encoding UTF8`. A JSON reader rejects it ("expected value at
/// line 1"), and a harness run then waited out its timer with no prompt (rc15 validation), so every file we read as JSON drops it first.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Parse the contents of a `--prompts-file`: a JSON array of non-empty strings.
pub fn parse_prompts_json(text: &str) -> Result<Vec<String>, String> {
    let prompts: Vec<String> = serde_json::from_str(strip_bom(text)).map_err(|e| format!("expected a JSON array of strings: {e}"))?;
    Ok(prompts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
}

/// The Settings tabs `--open-settings` can name (the ids of `settings-store.ts`'s `activeTab`).
pub const SETTINGS_TAB_NAMES: [&str; 9] = ["models", "databases", "builtins", "tools", "system-prompt", "interfaces", "schemas", "files", "network"];

/// The tab id a `--open-settings` value names: case and `_`/space are forgiven, anything else is `None`.
pub fn normalize_settings_tab(raw: &str) -> Option<&'static str> {
    let wanted = raw.trim().to_lowercase().replace(['_', ' '], "-");
    SETTINGS_TAB_NAMES.iter().copied().find(|t| *t == wanted)
}

/// A tool label as the drop-down shows it, `server - tool`; blank or without the separator is `None`.
pub fn normalize_tool_label(raw: &str) -> Option<String> {
    let label = raw.trim();
    let (server, tool) = label.rsplit_once(" - ")?;
    (!server.trim().is_empty() && !tool.trim().is_empty()).then(|| format!("{} - {}", server.trim(), tool.trim()))
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
    serde_json::from_str(strip_bom(&data)).map_err(|e| format!("Failed to parse JSON: {}", e))
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

    if let Some(timeout) = args.mcp_timeout_secs {
        for server in settings.mcp_servers.iter_mut().filter(|s| s.request_timeout_secs.is_none()) {
            server.request_timeout_secs = Some(timeout);
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

    // Further prompts for the same launch, run one after another in fresh chats
    let queued_prompts = match &args.prompts_file {
        Some(raw) => {
            let path = raw.strip_prefix('@').unwrap_or(raw);
            match fs::read_to_string(Path::new(path)).map_err(|e| e.to_string()).and_then(|t| parse_prompts_json(&t)) {
                Ok(prompts) => prompts,
                Err(e) => {
                    println!("[Launch] Failed to read --prompts-file {}: {}", path, e);
                    Vec::new()
                }
            }
        }
        None => Vec::new(),
    };

    // Enable default dev MCP test server when requested
    let mut enable_mcp_prompt: Option<String> = None;
    if args.enable_mcp_test == Some(true) {
        ensure_dev_test_server(settings);
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

    // Enable built-in demo database when requested
    if args.enable_demo_db == Some(true) {
        ensure_default_servers(settings);
        
        // Enable the database toolbox
        settings.database_toolbox.enabled = true;
        
        // Enable the embedded-demo source
        if let Some(demo_source) = settings
            .database_toolbox
            .sources
            .iter_mut()
            .find(|s| s.id == "embedded-demo")
        {
            demo_source.enabled = true;
        }
        
        // Ensure sql_select is in always-on builtins for querying
        if !settings.always_on_builtin_tools.contains(&"sql_select".to_string()) {
            settings.always_on_builtin_tools.push("sql_select".to_string());
        }
        
        println!("[Launch] Demo database enabled (--enable-demo-db)");
    }

    // Collect table files from CLI
    let table_files = args.table_files.clone().unwrap_or_default();
    if !table_files.is_empty() {
        println!("[Launch] Tabular data files attached via --table-file: {:?}", table_files);
    }

    // With no --initial-prompt, the first prompt of --prompts-file starts the run.
    let mut initial_prompt = launch_prompt.or(enable_mcp_prompt);
    let mut queued_prompts = queued_prompts;
    if initial_prompt.is_none() && !queued_prompts.is_empty() {
        initial_prompt = Some(queued_prompts.remove(0));
    }

    let direct_tool = args.direct_tool.as_deref().and_then(|raw| {
        let label = normalize_tool_label(raw);
        if label.is_none() {
            println!("[Launch] --direct-tool \"{raw}\" ignored: use the drop-down's \"<server> - <tool>\"");
        }
        label
    });
    let select_tool = args.select_tool.as_deref().and_then(|raw| {
        let label = normalize_tool_label(raw);
        if label.is_none() {
            println!("[Launch] --select-tool \"{raw}\" ignored: use the drop-down's \"<server> - <tool>\"");
        }
        label
    });
    let open_settings = args.open_settings.as_deref().and_then(|raw| {
        let tab = normalize_settings_tab(raw);
        if tab.is_none() {
            println!("[Launch] --open-settings \"{raw}\" ignored: the tabs are {}", SETTINGS_TAB_NAMES.join(", "));
        }
        tab.map(str::to_string)
    });
    // A direct-tool run involves no model, so a model that cannot load must not end it (the gateway would record that failure).
    let launch_model = if direct_tool.is_some() && launch_model.is_some() {
        println!("[Launch] --model ignored: --direct-tool sends the launch prompts to a tool, not a model");
        None
    } else {
        launch_model
    };

    LaunchOverrides {
        model: launch_model,
        initial_prompt,
        queued_prompts,
        same_chat: args.same_chat,
        table_files,
        direct_tool,
        open_settings,
        select_tool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> CliArgs {
        let mut full = vec!["plugable-chat"];
        full.extend_from_slice(args);
        CliArgs::try_parse_from(full).expect("arguments parse")
    }

    fn temp_file(name: &str, contents: &str) -> String {
        let dir = std::env::temp_dir().join(format!("plugable-cli-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input.json");
        std::fs::write(&path, contents).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn the_unattended_run_flags_parse() {
        let args = parse(&[
            "--reply-file", "/t/r.jsonl", "--exit-after-reply", "--reply-timeout-secs", "90", "--prompts-file", "/t/p.json",
            "--hidden", "--ephemeral", "--chat-home", "/t/home", "--mcp-timeout-secs", "900",
        ]);
        assert_eq!(args.reply_file.as_deref(), Some("/t/r.jsonl"));
        assert!(args.exit_after_reply && args.hidden && args.ephemeral);
        assert_eq!(args.reply_timeout_secs, Some(90));
        assert_eq!(args.prompts_file.as_deref(), Some("/t/p.json"));
        assert_eq!(args.chat_home.as_deref(), Some("/t/home"));
        assert_eq!(args.mcp_timeout_secs, Some(900));
    }

    #[test]
    fn dump_chats_takes_a_file_and_is_off_by_default() {
        assert_eq!(parse(&["--dump-chats", "/t/chats.json"]).dump_chats.as_deref(), Some("/t/chats.json"));
        assert!(parse(&[]).dump_chats.is_none());
    }

    #[test]
    fn repair_engine_is_a_flag_and_off_by_default() {
        assert!(parse(&["--repair-engine"]).repair_engine);
        assert!(!parse(&[]).repair_engine);
    }

    #[test]
    fn the_unattended_run_flags_are_off_by_default() {
        let args = parse(&[]);
        assert!(args.reply_file.is_none() && !args.exit_after_reply && !args.hidden && !args.ephemeral);
    }

    #[test]
    fn help_is_reported_as_help_so_the_launcher_can_print_it_instead_of_opening_the_app() {
        let err = CliArgs::try_parse_from(["plugable-chat", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(err.to_string().contains("--reply-file"), "the help lists the unattended-run flags");
    }

    #[test]
    fn the_unattended_ui_flags_parse_and_are_off_by_default() {
        let args = parse(&["--direct-tool", "Echo - ask", "--open-settings", "Tools", "--select-tool", "Echo - ask"]);
        assert_eq!(args.direct_tool.as_deref(), Some("Echo - ask"));
        assert_eq!(args.open_settings.as_deref(), Some("Tools"));
        assert_eq!(args.select_tool.as_deref(), Some("Echo - ask"));
        let none = parse(&[]);
        assert!(none.direct_tool.is_none() && none.open_settings.is_none() && none.select_tool.is_none());
    }

    #[test]
    fn a_settings_tab_name_is_matched_forgivingly_and_unknown_names_are_refused() {
        assert_eq!(normalize_settings_tab("models"), Some("models"));
        assert_eq!(normalize_settings_tab(" System_Prompt "), Some("system-prompt"));
        assert_eq!(normalize_settings_tab("system prompt"), Some("system-prompt"));
        assert_eq!(normalize_settings_tab("Network"), Some("network"));
        assert_eq!(normalize_settings_tab("nope"), None);
        assert_eq!(normalize_settings_tab(""), None);
        for tab in SETTINGS_TAB_NAMES {
            assert_eq!(normalize_settings_tab(tab), Some(tab));
        }
    }

    #[test]
    fn a_tool_label_needs_a_server_and_a_tool_split_on_the_last_separator() {
        assert_eq!(normalize_tool_label(" Echo  -  ask "), Some("Echo - ask".to_string()));
        assert_eq!(normalize_tool_label("My - Server - ask"), Some("My - Server - ask".to_string()));
        assert_eq!(normalize_tool_label("ask"), None);
        assert_eq!(normalize_tool_label(" - ask"), None);
        assert_eq!(normalize_tool_label(""), None);
    }

    #[test]
    fn the_unattended_ui_flags_reach_the_overrides_and_a_bad_value_is_dropped() {
        let o = apply_cli_overrides(&parse(&["--direct-tool", "Echo - ask", "--open-settings", "system prompt", "--select-tool", "Echo - ask"]), &mut AppSettings::default());
        assert_eq!(o.direct_tool.as_deref(), Some("Echo - ask"));
        assert_eq!(o.open_settings.as_deref(), Some("system-prompt"));
        assert_eq!(o.select_tool.as_deref(), Some("Echo - ask"));
        let bad = apply_cli_overrides(&parse(&["--direct-tool", "ask", "--open-settings", "zzz", "--select-tool", ""]), &mut AppSettings::default());
        assert!(bad.direct_tool.is_none() && bad.open_settings.is_none() && bad.select_tool.is_none());
    }

    #[test]
    fn a_direct_tool_run_ignores_the_model_override_but_a_model_run_keeps_it() {
        let direct = apply_cli_overrides(&parse(&["--model", "qwen3.5-4b", "--direct-tool", "Echo - ask", "--initial-prompt", "hi"]), &mut AppSettings::default());
        assert!(direct.model.is_none(), "a model that cannot load must not end a run that uses none");
        let model = apply_cli_overrides(&parse(&["--model", "qwen3.5-4b", "--initial-prompt", "hi"]), &mut AppSettings::default());
        assert_eq!(model.model.as_deref(), Some("qwen3.5-4b"));
    }

    #[test]
    fn same_chat_is_off_by_default_and_reaches_the_overrides() {
        assert!(!apply_cli_overrides(&parse(&["--initial-prompt", "a"]), &mut AppSettings::default()).same_chat);
        assert!(apply_cli_overrides(&parse(&["--initial-prompt", "a", "--same-chat"]), &mut AppSettings::default()).same_chat);
    }

    #[test]
    fn an_unknown_flag_does_not_parse_so_the_launcher_can_report_it() {
        assert!(CliArgs::try_parse_from(["plugable-chat", "--no-such-flag"]).is_err());
    }

    #[test]
    fn a_prompts_file_is_a_json_array_of_strings_with_blanks_dropped() {
        assert_eq!(parse_prompts_json(r#"[" a ", "", "b"]"#).unwrap(), vec!["a".to_string(), "b".to_string()]);
        assert!(parse_prompts_json(r#"{"a": 1}"#).is_err());
        assert!(parse_prompts_json("not json").is_err());
    }

    #[test]
    fn further_prompts_follow_the_initial_one_and_a_missing_file_adds_none() {
        let path = temp_file("prompts", r#"["second", "third"]"#);
        let mut settings = AppSettings::default();
        let overrides = apply_cli_overrides(&parse(&["--initial-prompt", "first", "--prompts-file", &path]), &mut settings);
        assert_eq!(overrides.initial_prompt.as_deref(), Some("first"));
        assert_eq!(overrides.queued_prompts, vec!["second".to_string(), "third".to_string()]);
        let with_at = apply_cli_overrides(&parse(&["--prompts-file", &format!("@{path}")]), &mut AppSettings::default());
        assert_eq!(with_at.initial_prompt.as_deref(), Some("second"), "with no --initial-prompt the file's first prompt starts the run");
        assert_eq!(with_at.queued_prompts, vec!["third".to_string()], "a leading @ is accepted like the other file arguments");
        let missing = apply_cli_overrides(&parse(&["--prompts-file", "/no/such/file.json"]), &mut AppSettings::default());
        assert!(missing.queued_prompts.is_empty());
    }

    #[test]
    fn the_mcp_timeout_applies_to_servers_that_do_not_set_their_own() {
        let own = temp_file("own", r#"{"id":"own","name":"own","enabled":true,"command":"x","request_timeout_secs":45}"#);
        let plain = temp_file("plain", r#"{"id":"plain","name":"plain","enabled":true,"command":"x"}"#);
        let mut settings = AppSettings::default();
        apply_cli_overrides(&parse(&["--mcp-server", &format!("@{own}"), "--mcp-server", &format!("@{plain}"), "--mcp-timeout-secs", "900"]), &mut settings);
        let timeout = |id: &str| settings.mcp_servers.iter().find(|s| s.id == id).and_then(|s| s.request_timeout_secs);
        assert_eq!(timeout("own"), Some(45));
        assert_eq!(timeout("plain"), Some(900));
    }

    #[test]
    fn a_byte_order_mark_does_not_stop_a_prompts_file_or_a_server_file_from_loading() {
        assert_eq!(parse_prompts_json("\u{feff}[\"one\", \" two \"]").unwrap(), vec!["one".to_string(), "two".to_string()]);
        assert_eq!(parse_prompts_json("[\"one\"]").unwrap(), vec!["one".to_string()]);
        let dir = std::env::temp_dir().join(format!("plugable-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("server.json");
        std::fs::write(&f, "\u{feff}{\"id\":\"a\",\"name\":\"a\",\"enabled\":true}").unwrap();
        let cfg: crate::settings::McpServerConfig = parse_json_or_file(&format!("@{}", f.display())).unwrap();
        assert_eq!(cfg.id, "a");
        assert!(parse_prompts_json("not json").is_err(), "real errors still surface");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
