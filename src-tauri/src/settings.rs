use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::fs;

use crate::agentic_state::RelevancyThresholds;

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
            auto_approve_tools: false,
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

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
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
    /// Whether tool_search defers MCP tool exposure until discovery (off by default).
    #[serde(default)]
    pub tool_search_enabled: bool,
    /// Whether the python_execution built-in tool is enabled (disabled by default).
    /// When enabled, models can execute Python code in a sandboxed environment.
    /// Renamed from code_execution_enabled - alias preserved for backwards compatibility.
    #[serde(default, alias = "code_execution_enabled")]
    pub python_execution_enabled: bool,
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
    /// Whether the schema_search built-in tool is enabled (disabled by default).
    /// When enabled, models can search for relevant database tables using embeddings.
    #[serde(default, alias = "search_schemas_enabled")]
    pub schema_search_enabled: bool,
    /// Whether the sql_select built-in tool is enabled (disabled by default).
    /// When enabled, models can execute SQL queries via configured database sources.
    #[serde(default, alias = "execute_sql_enabled")]
    pub sql_select_enabled: bool,
    /// Whether schema_search runs internally only (not exposed as a tool to the model).
    #[serde(default)]
    pub schema_search_internal_only: bool,
    
    // ============ Relevancy Thresholds for State Machine ============
    
    /// Minimum RAG chunk relevancy to inject into context (default: 0.3)
    #[serde(default = "default_rag_chunk_min_relevancy")]
    pub rag_chunk_min_relevancy: f32,
    /// Minimum schema table relevancy to inject into context (default: 0.2)
    #[serde(default = "default_schema_table_min_relevancy")]
    pub schema_table_min_relevancy: f32,
    /// Minimum schema relevancy to enable sql_select (default: 0.4)
    #[serde(default = "default_sql_enable_min_relevancy")]
    pub sql_enable_min_relevancy: f32,
    /// RAG relevancy above which SQL context is suppressed (default: 0.6)
    #[serde(default = "default_rag_dominant_threshold")]
    pub rag_dominant_threshold: f32,
    
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

fn default_schema_table_min_relevancy() -> f32 {
    0.2
}

fn default_sql_enable_min_relevancy() -> f32 {
    0.4
}

fn default_rag_dominant_threshold() -> f32 {
    0.6
}

impl AppSettings {
    /// Get all enabled MCP server configurations, including database sources.
    pub fn get_all_mcp_configs(&self) -> Vec<McpServerConfig> {
        let mut configs = self.mcp_servers.clone();

        // Database sources are only active if the toolbox is enabled AND at least one tool 
        // (sql_select, schema_search, or python_execution) is enabled to use them.
        let db_tools_available = self.schema_search_enabled 
            || self.sql_select_enabled 
            || self.schema_search_internal_only
            || self.python_execution_enabled;

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
            auto_approve_tools: source.auto_approve_tools,
            defer_tools: source.defer_tools,
            python_name: None,
        }
    }

    /// Get the relevancy thresholds from settings.
    pub fn get_relevancy_thresholds(&self) -> RelevancyThresholds {
        RelevancyThresholds {
            rag_chunk_min: self.rag_chunk_min_relevancy,
            schema_table_min: self.schema_table_min_relevancy,
            sql_enable_min: self.sql_enable_min_relevancy,
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
        }
    };
    enforce_python_name(&mut base);
    base
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            system_prompt: default_system_prompt(),
            mcp_servers: vec![default_mcp_test_server()],
            chat_format_default: default_chat_format(),
            chat_format_overrides: HashMap::new(),
            tool_call_formats: ToolCallFormatConfig::default(),
            tool_system_prompts: HashMap::new(),
            tool_search_max_results: default_tool_search_max_results(),
            tool_search_enabled: false,
            python_execution_enabled: false,
            python_tool_calling_enabled: default_python_tool_calling_enabled(),
            legacy_tool_call_format_enabled: false,
            tool_use_examples_enabled: false,
            tool_use_examples_max: default_tool_use_examples_max(),
            database_toolbox: DatabaseToolboxConfig::default(),
            schema_search_enabled: false,
            sql_select_enabled: false,
            schema_search_internal_only: false,
            // Relevancy thresholds
            rag_chunk_min_relevancy: default_rag_chunk_min_relevancy(),
            schema_table_min_relevancy: default_schema_table_min_relevancy(),
            sql_enable_min_relevancy: default_sql_enable_min_relevancy(),
            rag_dominant_threshold: default_rag_dominant_threshold(),
        }
    }
}

/// Ensure the default MCP test server exists in settings (for migration)
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
}

/// Get the path to the config file
fn get_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".plugable-chat").join("config.json")
}

/// Load settings from the config file
pub async fn load_settings() -> AppSettings {
    let config_path = get_config_path();

    let mut settings = match fs::read_to_string(&config_path).await {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(settings) => {
                println!("Settings loaded from {:?}", config_path);
                settings
            }
            Err(e) => {
                println!("Failed to parse settings: {}, using defaults", e);
                AppSettings::default()
            }
        },
        Err(e) => {
            println!(
                "No config file found at {:?}: {}, using defaults",
                config_path, e
            );
            AppSettings::default()
        }
    };

    // Normalize tool format config after load
    settings.tool_call_formats.normalize();

    // Ensure default servers exist (migration)
    ensure_default_servers(&mut settings);

    settings
}

/// Save settings to the config file
pub async fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let config_path = get_config_path();

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
        // tool_search is disabled by default (no deferral)
        assert!(!settings.tool_search_enabled);
        // python_execution is disabled by default
        assert!(!settings.python_execution_enabled);
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
        assert!(!settings.schema_search_enabled);
        assert!(!settings.sql_select_enabled);
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
        });

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(settings.system_prompt, parsed.system_prompt);
        assert_eq!(settings.mcp_servers.len(), parsed.mcp_servers.len());
        assert_eq!(settings.mcp_servers[0].id, parsed.mcp_servers[0].id);
        assert_eq!(settings.tool_call_formats, parsed.tool_call_formats);
        assert_eq!(settings.chat_format_default, parsed.chat_format_default);
        assert_eq!(settings.chat_format_overrides, parsed.chat_format_overrides);
        assert_eq!(settings.schema_search_enabled, parsed.schema_search_enabled);
        assert_eq!(settings.sql_select_enabled, parsed.sql_select_enabled);
    }

    #[test]
    fn test_backwards_compat_database_tools_enabled() {
        let json = r#"{"system_prompt": "test", "mcp_servers": [], "search_schemas_enabled": true, "execute_sql_enabled": true}"#;
        let parsed: AppSettings = serde_json::from_str(json).unwrap();
        assert!(parsed.schema_search_enabled);
        assert!(parsed.sql_select_enabled);
    }

    #[test]
    fn test_backwards_compat_code_execution_enabled() {
        // Test that old config files with "code_execution_enabled" still work
        let json =
            r#"{"system_prompt": "test", "mcp_servers": [], "code_execution_enabled": true}"#;
        let parsed: AppSettings = serde_json::from_str(json).unwrap();
        assert!(parsed.python_execution_enabled);
        // Default for new flags should still apply
        assert!(parsed.python_tool_calling_enabled);
        assert!(!parsed.legacy_tool_call_format_enabled);
        assert_eq!(parsed.tool_call_formats, ToolCallFormatConfig::default());
    }

    #[test]
    fn tool_call_format_normalizes_primary_and_enabled() {
        let mut cfg = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::Hermes],
            primary: ToolCallFormatName::CodeMode,
        };
        cfg.normalize();

        assert_eq!(cfg.enabled, vec![ToolCallFormatName::Hermes]);
        assert_eq!(cfg.primary, ToolCallFormatName::Hermes);
    }

    #[test]
    fn tool_call_format_dedupes_enabled_preserving_order() {
        let mut cfg = ToolCallFormatConfig {
            enabled: vec![
                ToolCallFormatName::Hermes,
                ToolCallFormatName::Hermes,
                ToolCallFormatName::PureJson,
            ],
            primary: ToolCallFormatName::Hermes,
        };
        cfg.normalize();

        assert_eq!(
            cfg.enabled,
            vec![ToolCallFormatName::Hermes, ToolCallFormatName::PureJson]
        );
        assert_eq!(cfg.primary, ToolCallFormatName::Hermes);
    }

    // ============ Python Identifier Validation Tests ============

    #[test]
    fn test_validate_python_identifier_valid() {
        assert!(validate_python_identifier("my_module").is_ok());
        assert!(validate_python_identifier("weather_api").is_ok());
        assert!(validate_python_identifier("mcp_test_server").is_ok());
        assert!(validate_python_identifier("_private").is_ok());
        assert!(validate_python_identifier("module123").is_ok());
        assert!(validate_python_identifier("a").is_ok());
    }

    #[test]
    fn test_validate_python_identifier_invalid() {
        // Empty
        assert!(validate_python_identifier("").is_err());

        // Starts with digit
        assert!(validate_python_identifier("123module").is_err());

        // Contains uppercase
        assert!(validate_python_identifier("MyModule").is_err());
        assert!(validate_python_identifier("myModule").is_err());

        // Contains invalid characters
        assert!(validate_python_identifier("my-module").is_err());
        assert!(validate_python_identifier("my.module").is_err());
        assert!(validate_python_identifier("my module").is_err());
        assert!(validate_python_identifier("my@module").is_err());

        // Python keywords
        assert!(validate_python_identifier("import").is_err());
        assert!(validate_python_identifier("class").is_err());
        assert!(validate_python_identifier("def").is_err());
        assert!(validate_python_identifier("None").is_err());
    }

    #[test]
    fn test_to_python_identifier() {
        // Basic conversion
        assert_eq!(to_python_identifier("My Module"), "my_module");
        assert_eq!(to_python_identifier("mcp-test-server"), "mcp_test_server");
        assert_eq!(to_python_identifier("Weather API"), "weather_api");

        // Handle leading digits
        assert_eq!(to_python_identifier("123abc"), "_123abc");

        // Handle special characters
        assert_eq!(to_python_identifier("my.module.name"), "my_module_name");
        assert_eq!(to_python_identifier("test@server#1"), "testserver1");

        // Handle multiple separators
        assert_eq!(to_python_identifier("my--module__name"), "my_module_name");

        // Handle empty/invalid input
        assert_eq!(to_python_identifier("@#$"), "module");
        assert_eq!(to_python_identifier(""), "module");

        // Handle Python keywords
        assert_eq!(to_python_identifier("import"), "import_");
        assert_eq!(to_python_identifier("class"), "class_");

        // Handle trailing separators
        assert_eq!(to_python_identifier("module_"), "module");
        assert_eq!(to_python_identifier("module--"), "module");
    }

    #[test]
    fn test_mcp_server_get_python_name() {
        // With explicit python_name
        let mut config = McpServerConfig::new("my-server".to_string(), "My Server".to_string());
        config.python_name = Some("custom_name".to_string());
        assert_eq!(config.get_python_name(), "custom_name");

        // Without explicit python_name (derived from id)
        let config2 = McpServerConfig::new("mcp-weather-api".to_string(), "Weather".to_string());
        assert_eq!(config2.get_python_name(), "mcp_weather_api");
    }

    #[test]
    fn test_mcp_server_set_python_name() {
        let mut config = McpServerConfig::new("test".to_string(), "Test".to_string());

        // Valid name
        assert!(config.set_python_name("my_module").is_ok());
        assert_eq!(config.python_name, Some("my_module".to_string()));

        // Invalid name
        assert!(config.set_python_name("My-Module").is_err());
        assert!(config.set_python_name("123abc").is_err());
        assert!(config.set_python_name("import").is_err());
    }

    #[test]
    fn test_enforce_python_name_sanitizes_name_and_python_name() {
        let mut config = McpServerConfig::new("server-1".to_string(), "Server Name 1".to_string());
        enforce_python_name(&mut config);
        assert_eq!(config.name, "Server Name 1");
        assert_eq!(config.python_name.as_deref(), Some("server_name_1"));
    }

    #[test]
    fn test_tool_call_format_normalization_and_resolution() {
        // Primary not in enabled -> should normalize
        let mut cfg = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::CodeMode],
            primary: ToolCallFormatName::Hermes,
        };
        cfg.normalize();
        assert!(cfg.enabled.contains(&cfg.primary));

        // Code mode unavailable -> fall back to first non-code
        let mut cfg2 = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::CodeMode, ToolCallFormatName::Mistral],
            primary: ToolCallFormatName::CodeMode,
        };
        cfg2.normalize();
        let resolved = cfg2.resolve_primary_for_prompt(false, true);
        assert_eq!(resolved, ToolCallFormatName::Mistral);

        // Code mode available -> keep primary
        let resolved2 = cfg2.resolve_primary_for_prompt(true, true);
        assert_eq!(resolved2, ToolCallFormatName::CodeMode);

        // Native unavailable -> fall back
        let mut cfg3 = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::Native, ToolCallFormatName::Hermes],
            primary: ToolCallFormatName::Native,
        };
        cfg3.normalize();
        let resolved3 = cfg3.resolve_primary_for_prompt(true, false);
        assert_eq!(resolved3, ToolCallFormatName::Hermes);

        // Native available -> keep primary
        let resolved4 = cfg3.resolve_primary_for_prompt(true, true);
        assert_eq!(resolved4, ToolCallFormatName::Native);
    }

    // ============ Database Toolbox Configuration Tests ============

    #[test]
    fn test_supported_database_kind_methods() {
        assert_eq!(SupportedDatabaseKind::Bigquery.execute_tool_name(), "execute_sql");
        assert_eq!(SupportedDatabaseKind::Postgres.execute_tool_name(), "postgres-sql");
        assert_eq!(SupportedDatabaseKind::Mysql.execute_tool_name(), "mysql-sql");
        assert_eq!(SupportedDatabaseKind::Sqlite.execute_tool_name(), "sqlite-sql");
        assert_eq!(SupportedDatabaseKind::Spanner.execute_tool_name(), "spanner-sql");

        assert_eq!(SupportedDatabaseKind::Bigquery.sql_dialect(), "GoogleSQL");
        assert_eq!(SupportedDatabaseKind::Postgres.sql_dialect(), "PostgreSQL");

        assert_eq!(SupportedDatabaseKind::Bigquery.display_name(), "BigQuery");
        assert_eq!(SupportedDatabaseKind::Postgres.display_name(), "PostgreSQL");
    }

    #[test]
    fn test_database_source_config_new() {
        let source = DatabaseSourceConfig::new(
            "my-pg".to_string(),
            "My PostgreSQL".to_string(),
            SupportedDatabaseKind::Postgres,
        );
        assert_eq!(source.id, "my-pg");
        assert_eq!(source.name, "My PostgreSQL");
        assert_eq!(source.kind, SupportedDatabaseKind::Postgres);
        assert!(!source.enabled);
        assert!(source.project_id.is_none());
    }

    #[test]
    fn test_database_toolbox_config_default() {
        let config = DatabaseToolboxConfig::default();
        assert!(!config.enabled);
        assert!(config.sources.is_empty());
    }

    #[test]
    fn test_database_toolbox_config_serde_roundtrip() {
        let config = DatabaseToolboxConfig {
            enabled: true,
            sources: vec![
                DatabaseSourceConfig {
                    id: "bq-prod".to_string(),
                    name: "BigQuery Production".to_string(),
                    kind: SupportedDatabaseKind::Bigquery,
                    enabled: true,
                    transport: Transport::Stdio,
                    command: Some("/opt/homebrew/bin/toolbox".to_string()),
                    args: vec!["--stdio".to_string(), "--prebuilt".to_string(), "bigquery".to_string()],
                    env: HashMap::new(),
                    auto_approve_tools: false,
                    defer_tools: true,
                    project_id: Some("my-project".to_string()),
                    sql_dialect: None,
                    dataset_allowlist: None,
                    table_allowlist: None,
                },
            ],
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: DatabaseToolboxConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enabled, parsed.enabled);
        assert_eq!(config.sources.len(), parsed.sources.len());
        assert_eq!(config.sources[0].kind, parsed.sources[0].kind);
        assert_eq!(config.sources[0].command, parsed.sources[0].command);
    }

    #[test]
    fn test_app_settings_includes_database_toolbox() {
        let settings = AppSettings::default();
        assert!(!settings.database_toolbox.enabled);
        assert!(!settings.schema_search_enabled);
        assert!(!settings.sql_select_enabled);
    }
}
