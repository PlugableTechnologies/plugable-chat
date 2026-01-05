use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::fs;

use crate::agentic_state::RelevancyThresholds;
use crate::paths;

// ============ Tool Calling Formats ============

/// Canonical names for tool calling formats shared across backend, frontend, and tests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallFormatName {
    /// Native OpenAI-style tool calling via the `tools` API parameter.
    /// Model must support native tool calling for this to work.
    Native,
    /// Text-based: `<tool_call>{"name": "...", "arguments": {...}}</tool_call>`
    Hermes,
    /// Text-based: `[TOOL_CALLS] [{"name": "...", "arguments": {...}}]`
    Mistral,
    /// Text-based: `tool_name(arg1="value", arg2=123)`
    Pythonic,
    /// Text-based: Raw JSON object
    PureJson,
    /// Python execution mode: tools called via python_execution sandbox
    CodeMode,
}

impl ToolCallFormatName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolCallFormatName::Native => "native",
            ToolCallFormatName::Hermes => "hermes",
            ToolCallFormatName::Mistral => "mistral",
            ToolCallFormatName::Pythonic => "pythonic",
            ToolCallFormatName::PureJson => "pure_json",
            ToolCallFormatName::CodeMode => "code_mode",
        }
    }

    /// Returns true if this format uses text-based prompting (not API-level or code-based)
    pub fn is_text_based(&self) -> bool {
        matches!(
            self,
            ToolCallFormatName::Hermes
                | ToolCallFormatName::Mistral
                | ToolCallFormatName::Pythonic
                | ToolCallFormatName::PureJson
        )
    }
}

// ============ Chat Formats ============

/// Chat API format selection per model/profile.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChatFormatName {
    OpenaiCompletions,
    OpenaiResponses,
}

impl ChatFormatName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChatFormatName::OpenaiCompletions => "openai_completions",
            ChatFormatName::OpenaiResponses => "openai_responses",
        }
    }
}

fn default_chat_format() -> ChatFormatName {
    ChatFormatName::OpenaiCompletions
}

/// Configuration for which formats are enabled and which one is primary (prompted).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallFormatConfig {
    #[serde(default = "default_enabled_formats")]
    pub enabled: Vec<ToolCallFormatName>,
    #[serde(default = "default_primary_format")]
    pub primary: ToolCallFormatName,
}

fn default_enabled_formats() -> Vec<ToolCallFormatName> {
    vec![
        ToolCallFormatName::Native,
        ToolCallFormatName::Hermes,
        ToolCallFormatName::CodeMode,
    ]
}

fn default_primary_format() -> ToolCallFormatName {
    ToolCallFormatName::Native
}

impl Default for ToolCallFormatConfig {
    fn default() -> Self {
        let mut cfg = Self {
            enabled: default_enabled_formats(),
            primary: default_primary_format(),
        };
        cfg.normalize();
        cfg
    }
}

impl ToolCallFormatConfig {
    /// Ensure the config is well-formed: at least one enabled format and primary is enabled.
    pub fn normalize(&mut self) {
        if self.enabled.is_empty() {
            self.enabled = default_enabled_formats();
        }

        // Deduplicate while preserving order
        let mut seen = HashSet::new();
        self.enabled.retain(|f| seen.insert(*f));

        if !self.enabled.contains(&self.primary) {
            self.primary = *self.enabled.first().unwrap_or(&default_primary_format());
        }
    }

    pub fn is_enabled(&self, format: ToolCallFormatName) -> bool {
        self.enabled.contains(&format)
    }

    /// Returns true if any text-based format (Hermes, Mistral, Pythonic, PureJson) is enabled
    pub fn any_text_based(&self) -> bool {
        self.enabled.iter().any(|f| f.is_text_based())
    }

    /// Returns true if any format other than CodeMode is enabled (Native or text-based)
    pub fn any_non_code(&self) -> bool {
        self.enabled
            .iter()
            .any(|f| *f != ToolCallFormatName::CodeMode)
    }

    /// Returns true if native tool calling is enabled
    pub fn native_enabled(&self) -> bool {
        self.enabled.contains(&ToolCallFormatName::Native)
    }

    /// Returns true if native is the primary format
    pub fn native_is_primary(&self) -> bool {
        self.primary == ToolCallFormatName::Native
    }

    /// Choose a primary that is actually usable.
    /// - If code mode is primary but not available, fall back
    /// - If native is primary but model doesn't support it, fall back
    /// The `code_mode_available` and `native_available` flags indicate runtime availability.
    pub fn resolve_primary_for_prompt(
        &self,
        code_mode_available: bool,
        native_available: bool,
    ) -> ToolCallFormatName {
        let primary = self.primary;

        // Check if primary is available
        let primary_available = match primary {
            ToolCallFormatName::CodeMode => code_mode_available,
            ToolCallFormatName::Native => native_available,
            _ => true, // Text-based formats are always available
        };

        if primary_available {
            return primary;
        }

        // Primary not available, find first available enabled format
        for format in &self.enabled {
            let available = match format {
                ToolCallFormatName::CodeMode => code_mode_available,
                ToolCallFormatName::Native => native_available,
                _ => true,
            };
            if available && *format != primary {
                return *format;
            }
        }

        // Nothing available, return primary anyway (will likely fail gracefully)
        primary
    }
}

// ============ Python Identifier Validation ============

/// Python reserved keywords that cannot be used as identifiers
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// Validate that a string is a valid Python identifier (module name).
///
/// Rules:
/// - Only lowercase letters, digits, and underscores
/// - Cannot start with a digit
/// - Cannot be a Python keyword
/// - Cannot be empty
pub fn validate_python_identifier(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Python identifier cannot be empty".to_string());
    }

    // Check first character (must be letter or underscore)
    let first_char = name.chars().next().unwrap();
    if !first_char.is_ascii_lowercase() && first_char != '_' {
        return Err(format!(
            "Python identifier must start with a lowercase letter or underscore, got '{}'",
            first_char
        ));
    }

    // Check all characters (must be lowercase letters, digits, or underscores)
    for (i, c) in name.chars().enumerate() {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' {
            return Err(format!(
                "Python identifier can only contain lowercase letters, digits, and underscores. \
                Invalid character '{}' at position {}",
                c, i
            ));
        }
    }

    // Check for Python keywords
    if PYTHON_KEYWORDS.contains(&name) {
        return Err(format!("'{}' is a Python reserved keyword", name));
    }

    Ok(())
}

/// Convert an arbitrary string to a valid Python identifier (snake_case).
///
/// Transformations:
/// - Convert to lowercase
/// - Replace spaces, hyphens, and other separators with underscores
/// - Remove invalid characters
/// - Prepend underscore if starts with digit
/// - Handle empty result
pub fn to_python_identifier(name: &str) -> String {
    let mut result = String::new();
    let mut last_was_underscore = false;

    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            // Convert to lowercase
            for lc in c.to_lowercase() {
                result.push(lc);
            }
            last_was_underscore = false;
        } else if c == ' ' || c == '-' || c == '_' || c == '.' {
            // Replace separators with underscore (avoiding duplicates)
            if !last_was_underscore && !result.is_empty() {
                result.push('_');
                last_was_underscore = true;
            }
        }
        // Skip other characters
    }

    // Remove trailing underscores
    while result.ends_with('_') {
        result.pop();
    }

    // Handle empty result
    if result.is_empty() {
        return "module".to_string();
    }

    // Prepend underscore if starts with digit
    if result.chars().next().unwrap().is_ascii_digit() {
        result = format!("_{}", result);
    }

    // Handle Python keywords by appending underscore
    if PYTHON_KEYWORDS.contains(&result.as_str()) {
        result.push('_');
    }

    result
}

// ============ Transport Types ============

/// MCP Server transport type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Sse { url: String },
}

impl Default for Transport {
    fn default() -> Self {
        Transport::Stdio
    }
}

// ============ Database Toolbox Configuration ============

/// Supported database kinds for schema discovery and SQL execution.
/// Each kind maps to specific Google MCP Database Toolbox tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SupportedDatabaseKind {
    /// BigQuery: uses bigquery-list-dataset-ids, bigquery-list-table-ids, bigquery-execute-sql
    Bigquery,
    /// PostgreSQL: uses postgres-sql, INFORMATION_SCHEMA queries
    Postgres,
    /// MySQL: uses mysql-sql, INFORMATION_SCHEMA queries
    Mysql,
    /// SQLite: uses sqlite-sql, sqlite_master queries
    Sqlite,
    /// Spanner: uses spanner-list-databases, spanner-sql
    Spanner,
}

impl SupportedDatabaseKind {
    /// Get the MCP Toolbox execute tool name for this database kind
    pub fn execute_tool_name(&self) -> &'static str {
        match self {
            SupportedDatabaseKind::Bigquery => "execute_sql",
            SupportedDatabaseKind::Postgres => "postgres-sql",
            SupportedDatabaseKind::Mysql => "mysql-sql",
            SupportedDatabaseKind::Sqlite => "sqlite-sql",
            SupportedDatabaseKind::Spanner => "spanner-sql",
        }
    }

    /// Get the SQL dialect name for this database kind
    pub fn sql_dialect(&self) -> &'static str {
        match self {
            SupportedDatabaseKind::Bigquery => "GoogleSQL",
            SupportedDatabaseKind::Postgres => "PostgreSQL",
            SupportedDatabaseKind::Mysql => "MySQL",
            SupportedDatabaseKind::Sqlite => "SQLite",
            SupportedDatabaseKind::Spanner => "GoogleSQL",
        }
    }

    /// Get the display name for this database kind
    pub fn display_name(&self) -> &'static str {
        match self {
            SupportedDatabaseKind::Bigquery => "BigQuery",
            SupportedDatabaseKind::Postgres => "PostgreSQL",
            SupportedDatabaseKind::Mysql => "MySQL",
            SupportedDatabaseKind::Sqlite => "SQLite",
            SupportedDatabaseKind::Spanner => "Spanner",
        }
    }
}

/// Configuration for a single database MCP server (one per database connection).
/// Auth/credentials live in the MCP toolbox config; we only store launch params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSourceConfig {
    /// Unique ID for this database server
    pub id: String,
    /// Human-readable display name
    pub name: String,
    /// Database kind (determines enumeration and query tools)
    pub kind: SupportedDatabaseKind,
    /// Whether this source is enabled for schema discovery
    pub enabled: bool,
    /// Transport for MCP (stdio or SSE)
    #[serde(default)]
    pub transport: Transport,
    /// Command/binary to launch the MCP toolbox
    #[serde(default)]
    pub command: Option<String>,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Auto-approve tool calls from this server
    #[serde(default)]
    pub auto_approve_tools: bool,
    /// Whether to defer tool exposure (align with MCP servers)
    #[serde(default = "default_defer_tools")]
    pub defer_tools: bool,
    /// Optional project id for BigQuery and similar
    #[serde(default)]
    pub project_id: Option<String>,
    /// Optional SQL dialect override (e.g., "GoogleSQL", "PostgreSQL").
    /// If not provided, the default for the database kind is used.
    #[serde(default)]
    pub sql_dialect: Option<String>,
    /// Optional comma-separated allowlist of datasets (BigQuery only). Empty => all datasets.
    #[serde(default)]
    pub dataset_allowlist: Option<String>,
    /// Optional comma-separated allowlist of tables (BigQuery only). Empty => all tables.
    #[serde(default)]
    pub table_allowlist: Option<String>,
}

impl DatabaseSourceConfig {
    pub fn new(id: String, name: String, kind: SupportedDatabaseKind) -> Self {
        Self {
            id,
            name,
            kind,
            enabled: false,
            transport: Transport::Stdio,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            auto_approve_tools: true,
            defer_tools: true,
            project_id: None,
            sql_dialect: None,
            dataset_allowlist: None,
            table_allowlist: None,
        }
    }

    /// Get the SQL dialect for this source, respecting overrides
    pub fn get_sql_dialect(&self) -> &str {
        if let Some(dialect) = self.sql_dialect.as_ref() {
            if !dialect.trim().is_empty() {
                return dialect.trim();
            }
        }
        self.kind.sql_dialect()
    }
}

/// Configuration for Google MCP Database Toolbox integration.
/// Manages MCP server configs for databases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseToolboxConfig {
    /// Whether the Database Toolbox integration is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Configured database MCP servers
    #[serde(default)]
    pub sources: Vec<DatabaseSourceConfig>,
}

impl Default for DatabaseToolboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sources: Vec::new(),
        }
    }
}

/// Schema for a cached table, used for embedding and search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTableSchema {
    /// Fully-qualified table name (e.g., project.dataset.table for BigQuery)
    pub fully_qualified_name: String,
    /// Source ID this table belongs to
    pub source_id: String,
    /// Database kind (for SQL dialect)
    pub kind: SupportedDatabaseKind,
    /// SQL dialect for this table (resolved from source config)
    #[serde(default)]
    pub sql_dialect: String,
    /// Whether this table should be used for search/SQL
    #[serde(default = "default_schema_enabled")]
    pub enabled: bool,
    /// Column schemas
    pub columns: Vec<CachedColumnSchema>,
    /// Primary key column names
    #[serde(default)]
    pub primary_keys: Vec<String>,
    /// Partition column names (BigQuery, Spanner)
    #[serde(default)]
    pub partition_columns: Vec<String>,
    /// Cluster column names (BigQuery)
    #[serde(default)]
    pub cluster_columns: Vec<String>,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
}

fn default_schema_enabled() -> bool {
    true
}

/// Schema for a cached column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedColumnSchema {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub description: Option<String>,
    /// Special attributes: "primary_key", "foreign_key", "partition", "cluster"
    #[serde(default)]
    pub special_attributes: Vec<String>,
    /// Top 3 most common values with percentage (e.g., "THEFT (23.5%)")
    #[serde(default)]
    pub top_values: Vec<String>,
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub transport: Transport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub auto_approve_tools: bool,
    /// If true (default), tools from this server are deferred (hidden initially, discovered via tool_search)
    /// If false, tools are active (immediately visible to the model)
    #[serde(default = "default_defer_tools")]
    pub defer_tools: bool,
    /// Python module name for this server's tools (must be valid Python identifier).
    /// If not set, defaults to a sanitized version of the server id.
    /// Used for Python imports: `from {python_name} import tool_function`
    #[serde(default)]
    pub python_name: Option<String>,
    /// If true, this server is managed by the Database Toolbox and its tools
    /// should NOT be exposed directly as MCP tools in the system prompt.
    #[serde(default)]
    pub is_database_source: bool,
}

fn default_defer_tools() -> bool {
    true
}

impl McpServerConfig {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            enabled: false,
            transport: Transport::Stdio,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            auto_approve_tools: false,
            defer_tools: true,
            python_name: None,
            is_database_source: false,
        }
    }

    /// Get the Python module name for this server.
    /// Returns the configured python_name, or derives one from the server id.
    pub fn get_python_name(&self) -> String {
        self.python_name
            .clone()
            .unwrap_or_else(|| to_python_identifier(&self.id))
    }

    /// Validate and set the Python module name.
    /// Returns an error if the name is not a valid Python identifier.
    pub fn set_python_name(&mut self, name: &str) -> Result<(), String> {
        validate_python_identifier(name)?;
        self.python_name = Some(name.to_string());
        Ok(())
    }
}

/// Ensure python_name is populated and sanitized from the display name.
pub fn enforce_python_name(config: &mut McpServerConfig) {
    let candidate = config
        .python_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            if config.name.is_empty() {
                config.id.as_str()
            } else {
                config.name.as_str()
            }
        });
    let sanitized = to_python_identifier(candidate);
    config.python_name = Some(sanitized);
}

// ============ Always-On Configuration ============

/// Configuration for an always-on database table
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlwaysOnTableConfig {
    /// Database source ID
    pub source_id: String,
    /// Fully qualified table name (e.g., "project.dataset.table")
    pub table_fq_name: String,
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    /// Persisted model selection - applied on app startup
    #[serde(default)]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Default chat format when no per-model override is present
    #[serde(default = "default_chat_format")]
    pub chat_format_default: ChatFormatName,
    /// Optional per-model chat format overrides keyed by model id
    #[serde(default)]
    pub chat_format_overrides: HashMap<String, ChatFormatName>,
    /// Tool calling format configuration (enabled formats + primary)
    #[serde(default)]
    pub tool_call_formats: ToolCallFormatConfig,
    /// Optional system prompt snippets keyed by "{server_id}::{tool_name}".
    /// Use "builtin" as server_id for built-in tools.
    #[serde(default)]
    pub tool_system_prompts: HashMap<String, String>,
    /// Maximum number of tools returned by tool_search (defaults to 3 for token control)
    #[serde(default = "default_tool_search_max_results")]
    pub tool_search_max_results: usize,
    /// Whether python-driven tool calling is allowed. If false, we will not
    /// execute tool calls even if python_execution is enabled.
    #[serde(default = "default_python_tool_calling_enabled")]
    pub python_tool_calling_enabled: bool,
    /// Whether to allow legacy <tool_call> parsing. Disabled by default.
    #[serde(default)]
    pub legacy_tool_call_format_enabled: bool,
    /// Whether to include tool input_examples in prompts (capped for small models)
    #[serde(default)]
    pub tool_use_examples_enabled: bool,
    /// Maximum number of examples per tool when enabled
    #[serde(default = "default_tool_use_examples_max")]
    pub tool_use_examples_max: usize,
    /// Configuration for Google MCP Database Toolbox integration
    #[serde(default)]
    pub database_toolbox: DatabaseToolboxConfig,
    
    // ============ Relevancy Thresholds for State Machine ============
    
    /// Minimum RAG chunk relevancy to inject into context (default: 0.3)
    #[serde(default = "default_rag_chunk_min_relevancy")]
    pub rag_chunk_min_relevancy: f32,
    /// Minimum schema relevancy to enable sql_select and inject into context (default: 0.4)
    #[serde(default = "default_schema_relevancy_threshold", alias = "sql_enable_min_relevancy")]
    pub schema_relevancy_threshold: f32,
    /// RAG relevancy above which SQL context is suppressed (default: 0.6)
    #[serde(default = "default_rag_dominant_threshold")]
    pub rag_dominant_threshold: f32,

    // ============ Always-On Configuration ============
    // These items are automatically included in every chat without explicit attachment.

    /// Always-on built-in tools (e.g., ["python_execution", "sql_select"])
    /// These appear as locked pills in the UI and are always available.
    #[serde(default)]
    pub always_on_builtin_tools: Vec<String>,

    /// Always-on MCP tools in "server_id::tool_name" format
    /// These appear as locked pills in the UI and are always available.
    #[serde(default)]
    pub always_on_mcp_tools: Vec<String>,

    /// Always-on database tables for SQL context
    /// These tables' schemas are always included in the system prompt.
    #[serde(default)]
    pub always_on_tables: Vec<AlwaysOnTableConfig>,

    /// Always-on RAG file/folder paths
    /// These are automatically indexed and searched for every chat.
    #[serde(default)]
    pub always_on_rag_paths: Vec<String>,

    // NOTE: native_tool_calling_enabled has been removed.
    // Native tool calling is now controlled via tool_call_formats (Native format).
    // Old configs with this field will be migrated on load.
}

fn default_system_prompt() -> String {
    r#"You are a helpful AI assistant. Be direct and concise in your responses. When you don't know something, say so rather than guessing."#.to_string()
}

fn default_tool_search_max_results() -> usize {
    3
}

fn default_python_tool_calling_enabled() -> bool {
    true
}

fn default_tool_use_examples_max() -> usize {
    2
}

fn default_rag_chunk_min_relevancy() -> f32 {
    0.3
}

fn default_schema_relevancy_threshold() -> f32 {
    0.4
}

fn default_rag_dominant_threshold() -> f32 {
    0.6
}

impl AppSettings {
    /// Check if a built-in tool is marked as Always On.
    pub fn is_builtin_always_on(&self, name: &str) -> bool {
        self.always_on_builtin_tools.contains(&name.to_string())
    }

    /// Determine if schema search should run internally (not exposed as a tool).
    /// 
    /// This is automatically derived for globally enabled tools:
    /// - If sql_select is always on but schema_search is not, internal search is ON
    /// - This provides table context for SQL queries without exposing schema_search as a tool
    pub fn should_run_internal_schema_search(&self) -> bool {
        self.is_builtin_always_on("sql_select") && !self.is_builtin_always_on("schema_search")
    }
    
    /// Check if any schema search functionality is active globally (as tool or internal).
    pub fn has_schema_search_active(&self) -> bool {
        self.is_builtin_always_on("schema_search") || self.should_run_internal_schema_search()
    }
    
    /// Get all enabled MCP server configurations, including database sources.
    pub fn get_all_mcp_configs(&self) -> Vec<McpServerConfig> {
        let mut configs = self.mcp_servers.clone();

        // Database sources are only active if the toolbox is enabled AND at least one
        // database-specific tool (sql_select, schema_search) is enabled globally.
        // Note: per-chat attachments will override this at runtime in lib.rs.
        let db_tools_available = self.is_builtin_always_on("schema_search") 
            || self.is_builtin_always_on("sql_select") 
            || self.should_run_internal_schema_search();

        // Always include database sources so they can be disconnected if toolbox is disabled
        for source in &self.database_toolbox.sources {
            let mut config = self.source_to_mcp_config(source);
            if !self.database_toolbox.enabled || !db_tools_available {
                config.enabled = false;
            }
            configs.push(config);
        }

        configs
    }

    fn source_to_mcp_config(&self, source: &DatabaseSourceConfig) -> McpServerConfig {
        let mut env = source.env.clone();

        // BigQuery requires BIGQUERY_PROJECT; derive it from project_id when not provided
        if source.kind == SupportedDatabaseKind::Bigquery {
            if let Some(project_id) = source.project_id.as_ref() {
                if !project_id.trim().is_empty() && !env.contains_key("BIGQUERY_PROJECT") {
                    env.insert("BIGQUERY_PROJECT".to_string(), project_id.trim().to_string());
                }
            }
        }

        McpServerConfig {
            id: source.id.clone(),
            name: source.name.clone(),
            enabled: source.enabled,
            transport: source.transport.clone(),
            command: source.command.clone(),
            args: source.args.clone(),
            env,
            auto_approve_tools: true, // Always true for database sources
            defer_tools: source.defer_tools,
            python_name: None,
            is_database_source: true,
        }
    }

    /// Get the relevancy thresholds from settings.
    pub fn get_relevancy_thresholds(&self) -> RelevancyThresholds {
        RelevancyThresholds {
            rag_chunk_min: self.rag_chunk_min_relevancy,
            schema_relevancy: self.schema_relevancy_threshold,
            rag_dominant_threshold: self.rag_dominant_threshold,
        }
    }
}

fn find_workspace_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..5 {
        if dir.join("mcp-test-server").join("Cargo.toml").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Create the default MCP test server configuration
pub fn default_mcp_test_server() -> McpServerConfig {
    let workspace_root = find_workspace_root();
    let manifest_path = workspace_root
        .as_ref()
        .map(|root| root.join("mcp-test-server").join("Cargo.toml"));

    // Try to find the pre-built binary in common locations
    // Priority: target/release > cargo run
    let binary_path = workspace_root.as_ref().and_then(|root| {
        let release_path = root.join("target/release/mcp-test-server");
        if release_path.exists() {
            Some(release_path.to_string_lossy().to_string())
        } else {
            let alt_path = root.join("mcp-test-server/target/release/mcp-test-server");
            if alt_path.exists() {
                Some(alt_path.to_string_lossy().to_string())
            } else {
                None
            }
        }
    });

    let mut base = if let Some(path) = binary_path {
        McpServerConfig {
            id: "mcp-test-server".to_string(),
            name: "mcp_test_server".to_string(),
            enabled: false, // Disabled by default
            transport: Transport::Stdio,
            command: Some(path),
            args: vec![],
            env: HashMap::new(),
            auto_approve_tools: true, // Auto-approve for dev testing
            defer_tools: false,       // Expose tools immediately for quick testing
            python_name: None,
            is_database_source: false,
        }
    } else {
        // Fall back to cargo run if binary not found
        McpServerConfig {
            id: "mcp-test-server".to_string(),
            name: "mcp_test_server".to_string(),
            enabled: false, // Disabled by default
            transport: Transport::Stdio,
            command: Some("cargo".to_string()),
            args: vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                manifest_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "mcp-test-server/Cargo.toml".to_string()),
                "--release".to_string(),
            ],
            env: HashMap::new(),
            auto_approve_tools: true, // Auto-approve for dev testing
            defer_tools: false,       // Expose tools immediately for quick testing
            python_name: None,
            is_database_source: false,
        }
    };
    enforce_python_name(&mut base);
    base
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            system_prompt: default_system_prompt(),
            selected_model: None,
            mcp_servers: vec![default_mcp_test_server()],
            chat_format_default: default_chat_format(),
            chat_format_overrides: HashMap::new(),
            tool_call_formats: ToolCallFormatConfig::default(),
            tool_system_prompts: HashMap::new(),
            tool_search_max_results: default_tool_search_max_results(),
            python_tool_calling_enabled: default_python_tool_calling_enabled(),
            legacy_tool_call_format_enabled: false,
            tool_use_examples_enabled: false,
            tool_use_examples_max: default_tool_use_examples_max(),
            database_toolbox: DatabaseToolboxConfig::default(),
            // Relevancy thresholds
            rag_chunk_min_relevancy: default_rag_chunk_min_relevancy(),
            schema_relevancy_threshold: default_schema_relevancy_threshold(),
            rag_dominant_threshold: default_rag_dominant_threshold(),
            // Always-on configuration (empty by default)
            always_on_builtin_tools: Vec::new(),
            always_on_mcp_tools: Vec::new(),
            always_on_tables: Vec::new(),
            always_on_rag_paths: Vec::new(),
        }
    }
}

/// Source ID for the embedded demo database
pub const EMBEDDED_DEMO_SOURCE_ID: &str = "embedded-demo";

/// Find the test-data directory by searching from current dir and parents
pub fn find_test_data_dir() -> Option<std::path::PathBuf> {
    // Try current directory first
    let mut dir = std::env::current_dir().ok()?;

    for _ in 0..5 {
        let test_data = dir.join("test-data");
        if test_data.exists() && test_data.is_dir() {
            // Return canonical path to avoid relative path issues
            return test_data.canonicalize().ok().or(Some(test_data));
        }
        if !dir.pop() {
            break;
        }
    }

    // Also check relative to executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // Check exe_dir/../../test-data (typical for development)
            let dev_path = exe_dir.join("../../test-data");
            if dev_path.exists() {
                return dev_path.canonicalize().ok();
            }
            // Check macOS bundle Resources folder: Contents/MacOS/../Resources/test-data
            #[cfg(target_os = "macos")]
            {
                let resources_path = exe_dir.join("../Resources/test-data");
                if resources_path.exists() {
                    return resources_path.canonicalize().ok().or(Some(resources_path));
                }
            }
            // Check exe_dir/test-data (for bundled apps on Windows/Linux)
            let bundled_path = exe_dir.join("test-data");
            if bundled_path.exists() {
                return bundled_path.canonicalize().ok().or(Some(bundled_path));
            }
        }
    }

    None
}

/// Find the MCP Database Toolbox binary by searching PATH and common installation locations.
/// Returns the path to the toolbox binary if found, None otherwise.
pub fn find_toolbox_binary() -> Option<String> {
    // First, try to find it in PATH using which/where
    #[cfg(windows)]
    let which_result = std::process::Command::new("where.exe")
        .arg("toolbox")
        .output();
    
    #[cfg(not(windows))]
    let which_result = std::process::Command::new("which")
        .arg("toolbox")
        .output();
    
    if let Ok(output) = which_result {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(|s| s.trim().to_string());
            if let Some(p) = path {
                if !p.is_empty() && std::path::Path::new(&p).exists() {
                    println!("[Settings] Found toolbox in PATH: {}", p);
                    return Some(p);
                }
            }
        }
    }
    
    // Fall back to common installation locations
    let common_paths: &[&str] = &[
        #[cfg(target_os = "macos")]
        "/opt/homebrew/bin/toolbox",
        #[cfg(target_os = "macos")]
        "/usr/local/bin/toolbox",
        #[cfg(target_os = "linux")]
        "/usr/local/bin/toolbox",
        #[cfg(target_os = "linux")]
        "/usr/bin/toolbox",
        #[cfg(windows)]
        "C:\\Program Files\\toolbox\\toolbox.exe",
        #[cfg(windows)]
        "C:\\toolbox\\toolbox.exe",
    ];
    
    for path in common_paths {
        if std::path::Path::new(path).exists() {
            println!("[Settings] Found toolbox at common location: {}", path);
            return Some(path.to_string());
        }
    }
    
    // Also check if Go binaries directory contains it (common for `go install`)
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        let go_bin_path = std::path::Path::new(&home).join("go").join("bin").join(
            #[cfg(windows)]
            "toolbox.exe",
            #[cfg(not(windows))]
            "toolbox",
        );
        if go_bin_path.exists() {
            let path_str = go_bin_path.to_string_lossy().to_string();
            println!("[Settings] Found toolbox in Go bin: {}", path_str);
            return Some(path_str);
        }
    }
    
    println!("[Settings] Toolbox binary not found - user must configure path manually");
    None
}

/// Normalize a path for use in YAML configuration files.
/// On Windows, this strips the extended path prefix (\\?\) and converts backslashes to forward slashes.
/// SQLite and other tools don't understand the \\?\ prefix that canonicalize() produces on Windows.
fn normalize_path_for_yaml(path: &std::path::Path) -> String {
    let path_str = path.to_string_lossy();
    
    // On Windows, canonicalize() returns paths like \\?\C:\Users\...
    // Strip the \\?\ prefix as SQLite and most tools don't understand it
    #[cfg(windows)]
    let path_str = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);
    
    // Convert backslashes to forward slashes for cross-platform YAML compatibility
    path_str.replace('\\', "/")
}

/// Ensure the demo-tools.yaml file exists with correct absolute paths.
/// 
/// The YAML is written to a writable cache directory because the bundled
/// test-data Resources folder is read-only on macOS. The YAML points to
/// the bundled demo.db in the read-only Resources folder.
pub fn ensure_demo_tools_yaml() -> Option<std::path::PathBuf> {
    let test_data_dir = find_test_data_dir()?;
    let demo_db_path = test_data_dir.join("demo.db");
    
    // Verify demo.db exists
    if !demo_db_path.exists() {
        eprintln!("[DemoDatabase] demo.db not found at {:?}", demo_db_path);
        return None;
    }
    
    // Write to a writable cache directory (since Resources may be read-only on macOS)
    let cache_dir = paths::get_cache_dir().join("demo-toolbox");
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        eprintln!("[DemoDatabase] Failed to create cache dir {:?}: {}", cache_dir, e);
        return None;
    }
    let tools_yaml_path = cache_dir.join("demo-tools.yaml");
    
    // Generate the tools.yaml content with absolute path to demo.db
    // Tool name 'sqlite-sql' matches what DatabaseToolboxActor expects
    let db_path_str = normalize_path_for_yaml(&demo_db_path);
    let content = format!(
        r#"# MCP Database Toolbox configuration for Chicago Crimes demo database
# Auto-generated by Plugable Chat
# Tool names match what Plugable Chat's DatabaseToolboxActor expects

sources:
  demo-sqlite:
    kind: sqlite
    database: "{db_path}"

tools:
  # Generic SQL execution tool - used by DatabaseToolboxActor for all queries
  sqlite-sql:
    kind: sqlite-sql
    source: demo-sqlite
    description: Execute a SQL query against the Chicago Crimes SQLite database
    statement: "{{{{.sql}}}}"
    templateParameters:
      - name: sql
        type: string
        description: The SQL query to execute
"#,
        db_path = db_path_str
    );
    
    // Write the file (overwrite to ensure paths are current)
    if let Err(e) = std::fs::write(&tools_yaml_path, content) {
        eprintln!("[DemoDatabase] Failed to write demo-tools.yaml: {}", e);
        return None;
    }
    
    println!("[DemoDatabase] Generated demo-tools.yaml at {:?} (pointing to {:?})", tools_yaml_path, demo_db_path);
    Some(tools_yaml_path)
}

/// Create the default embedded demo database source configuration.
/// This is a built-in SQLite database with Chicago Crimes data for testing.
/// Requires the Google MCP Database Toolbox binary to be installed.
pub fn default_demo_database_source() -> DatabaseSourceConfig {
    // Ensure the tools.yaml exists and get its absolute path
    let tools_file_path = ensure_demo_tools_yaml()
        .map(|p| normalize_path_for_yaml(&p))
        .unwrap_or_else(|| {
            // Fallback to relative path if we can't generate
            find_test_data_dir()
                .map(|p| normalize_path_for_yaml(&p.join("demo-tools.yaml")))
                .unwrap_or_else(|| "test-data/demo-tools.yaml".to_string())
        });

    // Auto-detect the toolbox binary path
    let toolbox_command = find_toolbox_binary();

    DatabaseSourceConfig {
        id: EMBEDDED_DEMO_SOURCE_ID.to_string(),
        name: "Chicago Crimes Demo".to_string(),
        kind: SupportedDatabaseKind::Sqlite,
        enabled: false, // Disabled by default, user must opt-in
        transport: Transport::Stdio,
        command: toolbox_command, // Auto-detected or None if not found
        args: vec![
            "--tools-file".to_string(),
            tools_file_path,
            "--stdio".to_string(),
        ],
        env: std::collections::HashMap::new(),
        auto_approve_tools: true,
        defer_tools: false, // Demo tables should be immediately visible
        project_id: None,
        sql_dialect: Some("SQLite".to_string()),
        dataset_allowlist: None,
        table_allowlist: None,
    }
}

/// Check if a database source is the embedded demo database
pub fn is_embedded_demo_source(source_id: &str) -> bool {
    source_id == EMBEDDED_DEMO_SOURCE_ID
}

/// Regenerate the args for the embedded demo database source with a fresh tools.yaml path.
/// This should be called before starting the demo database to ensure the path is valid.
/// Returns None if the demo.db cannot be found (not bundled/available).
pub fn regenerate_demo_source_args() -> Option<Vec<String>> {
    let tools_file_path = ensure_demo_tools_yaml()?;
    let path_str = normalize_path_for_yaml(&tools_file_path);
    Some(vec![
        "--tools-file".to_string(),
        path_str,
        "--stdio".to_string(),
    ])
}

/// Ensure the default MCP test server and demo database exist in settings (for migration)
pub fn ensure_default_servers(settings: &mut AppSettings) {
    // Check if mcp-test-server already exists
    let has_test_server = settings
        .mcp_servers
        .iter()
        .any(|s| s.id == "mcp-test-server");

    if !has_test_server {
        println!("Adding default MCP test server to settings");
        settings.mcp_servers.insert(0, default_mcp_test_server());
    }

    // Check if embedded-demo database source already exists
    let demo_source_idx = settings
        .database_toolbox
        .sources
        .iter()
        .position(|s| s.id == EMBEDDED_DEMO_SOURCE_ID);

    match demo_source_idx {
        Some(idx) => {
            // ALWAYS regenerate the demo source args to ensure the tools.yaml path is valid
            // This handles cases where the app was moved, or the cache was cleared, or
            // the working directory changed (common on macOS bundles).
            let source = &mut settings.database_toolbox.sources[idx];
            
            if let Some(fresh_args) = regenerate_demo_source_args() {
                if source.args != fresh_args {
                    println!("[DemoDatabase] Regenerating demo-tools.yaml path for embedded demo source");
                    source.args = fresh_args;
                }
            } else {
                // demo.db not found - leave args as-is but warn
                println!("[DemoDatabase] Warning: Could not regenerate demo-tools.yaml (demo.db not found)");
            }
            
            // Auto-detect toolbox binary if not set or if set to an invalid path
            if source.command.is_none() 
                || source.command.as_ref().is_some_and(|cmd| !std::path::Path::new(cmd).exists()) 
            {
                if let Some(detected_path) = find_toolbox_binary() {
                    println!("Auto-detected toolbox binary for demo source: {}", detected_path);
                    source.command = Some(detected_path);
                }
            }
        }
        None => {
            println!("Adding default embedded demo database source to settings");
            settings
                .database_toolbox
                .sources
                .insert(0, default_demo_database_source());
        }
    }
}

/// Get the path to the config file.
///
/// Uses platform-standard config directories:
/// - macOS: `~/Library/Application Support/plugable-chat/config.json`
/// - Windows: `%APPDATA%\plugable-chat\config.json`
pub fn get_settings_path() -> PathBuf {
    crate::paths::get_config_dir().join("config.json")
}

/// Get the path to the application data directory (for LanceDB, caches, etc.)
///
/// Deprecated: Use `crate::paths::get_data_dir()` directly for new code.
/// This is kept for backward compatibility.
pub fn get_app_data_dir() -> PathBuf {
    crate::paths::get_data_dir()
}

/// Load settings from the config file
pub async fn load_settings() -> AppSettings {
    let config_path = get_settings_path();

    let (mut settings, raw_json) = match fs::read_to_string(&config_path).await {
        Ok(contents) => {
            let s = match serde_json::from_str::<AppSettings>(&contents) {
                Ok(settings) => {
                    println!("Settings loaded from {:?}", config_path);
                    settings
                }
                Err(e) => {
                    println!("Failed to parse settings: {}, using defaults", e);
                    AppSettings::default()
                }
            };
            let v = serde_json::from_str::<serde_json::Value>(&contents).ok();
            (s, v)
        }
        Err(e) => {
            println!(
                "No config file found at {:?}: {}, using defaults",
                config_path, e
            );
            (AppSettings::default(), None)
        }
    };

    // Perform migration from legacy boolean flags if they exist in the raw JSON
    if let Some(obj) = raw_json.and_then(|v| v.as_object().cloned()) {
        let mut always_on = settings.always_on_builtin_tools.clone();
        
        // Map legacy field names/aliases to tool names
        let legacy_map = [
            ("tool_search_enabled", "tool_search"),
            ("python_execution_enabled", "python_execution"),
            ("code_execution_enabled", "python_execution"),
            ("schema_search_enabled", "schema_search"),
            ("search_schemas_enabled", "schema_search"),
            ("sql_select_enabled", "sql_select"),
            ("execute_sql_enabled", "sql_select"),
        ];

        let mut migrated = false;
        for (field, tool) in legacy_map {
            if obj.get(field).and_then(|v| v.as_bool()).unwrap_or(false) {
                if !always_on.contains(&tool.to_string()) {
                    always_on.push(tool.to_string());
                    migrated = true;
                    println!("Migrated legacy tool flag: {} -> always_on_builtin_tools", field);
                }
            }
        }
        
        if migrated {
            settings.always_on_builtin_tools = always_on;
        }
    }

    // Normalize tool format config after load
    settings.tool_call_formats.normalize();

    // Ensure default servers exist (migration)
    ensure_default_servers(&mut settings);

    settings
}

/// Save settings to the config file
pub async fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let config_path = get_settings_path();

    // Ensure the directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let contents = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    fs::write(&config_path, contents)
        .await
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    println!("Settings saved to {:?}", config_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = AppSettings::default();
        assert!(!settings.system_prompt.is_empty());
        // Default settings include the mcp-test-server (disabled by default)
        assert!(settings
            .mcp_servers
            .iter()
            .any(|s| s.id == "mcp-test-server"));
        assert!(
            !settings
                .mcp_servers
                .iter()
                .find(|s| s.id == "mcp-test-server")
                .unwrap()
                .enabled
        );
        assert!(settings.tool_system_prompts.is_empty());
        // python tool calling defaults
        assert!(settings.python_tool_calling_enabled);
        assert!(!settings.legacy_tool_call_format_enabled);
        assert_eq!(
            settings.tool_search_max_results,
            default_tool_search_max_results()
        );
        assert!(!settings.tool_use_examples_enabled);
        assert_eq!(
            settings.tool_use_examples_max,
            default_tool_use_examples_max()
        );
        assert_eq!(settings.tool_call_formats, ToolCallFormatConfig::default());
        assert_eq!(settings.chat_format_default, default_chat_format());
        assert!(settings.chat_format_overrides.is_empty());
        assert!(settings.always_on_builtin_tools.is_empty());
    }

    #[test]
    fn test_serialize_deserialize() {
        let mut settings = AppSettings::default();
        settings.mcp_servers.push(McpServerConfig {
            id: "test-1".to_string(),
            name: "Test Server".to_string(),
            enabled: true,
            transport: Transport::Stdio,
            command: Some("node".to_string()),
            args: vec!["server.js".to_string()],
            env: HashMap::from([("DEBUG".to_string(), "true".to_string())]),
            auto_approve_tools: false,
            defer_tools: true,
            python_name: Some("test_server".to_string()),
            is_database_source: false,
        });

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(settings.system_prompt, parsed.system_prompt);
        assert_eq!(settings.mcp_servers.len(), parsed.mcp_servers.len());
        assert_eq!(settings.mcp_servers[0].id, parsed.mcp_servers[0].id);
        assert_eq!(settings.tool_call_formats, parsed.tool_call_formats);
        assert_eq!(settings.chat_format_default, parsed.chat_format_default);
        assert_eq!(settings.chat_format_overrides, parsed.chat_format_overrides);
        assert_eq!(settings.always_on_builtin_tools, parsed.always_on_builtin_tools);
    }

    #[test]
    fn test_app_settings_includes_database_toolbox() {
        let settings = AppSettings::default();
        assert!(!settings.database_toolbox.enabled);
        assert!(settings.always_on_builtin_tools.is_empty());
    }

    #[tokio::test]
    async fn test_load_settings_migration() {
        // Create a temporary config file with legacy flags
        // Use the actual config path from the paths module
        let config_path = get_settings_path();
        let config_dir = config_path.parent().unwrap().to_path_buf();
        
        // Backup existing config if it exists
        let backup_path = config_dir.join("config.json.bak");
        let has_existing = config_path.exists();
        if has_existing {
            fs::copy(&config_path, &backup_path).await.unwrap();
        } else {
            fs::create_dir_all(&config_dir).await.unwrap();
        }

        let legacy_json = r#"{
            "system_prompt": "test prompt",
            "tool_search_enabled": true,
            "python_execution_enabled": false,
            "search_schemas_enabled": true,
            "execute_sql_enabled": true,
            "always_on_builtin_tools": ["python_execution"]
        }"#;
        
        fs::write(&config_path, legacy_json).await.unwrap();

        // Load settings - should migrate flags
        let settings = load_settings().await;

        // Verify migration
        assert!(settings.always_on_builtin_tools.contains(&"tool_search".to_string()));
        assert!(settings.always_on_builtin_tools.contains(&"python_execution".to_string()));
        assert!(settings.always_on_builtin_tools.contains(&"schema_search".to_string()));
        assert!(settings.always_on_builtin_tools.contains(&"sql_select".to_string()));
        assert_eq!(settings.always_on_builtin_tools.len(), 4);

        // Cleanup: restore or remove
        if has_existing {
            fs::rename(&backup_path, &config_path).await.unwrap();
        } else {
            fs::remove_file(&config_path).await.unwrap();
        }
    }
}
