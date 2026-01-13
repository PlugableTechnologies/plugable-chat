//! Settings State Machine
//!
//! Computes the operational mode from settings flags. This module represents
//! Tier 1 of the three-tier state machine hierarchy:
//!
//! 1. SettingsStateMachine (this module) - Settings -> OperationalMode
//! 2. AgenticStateMachine - OperationalMode + Context -> AgenticState
//! 3. MidTurnStateMachine - AgenticState + Events -> MidTurnState
//!
//! The SettingsStateMachine runs when settings change (rarely) and produces
//! an OperationalMode that describes the system's operational posture.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::agentic_state::Capability;
use crate::settings::{AppSettings, ToolCallFormatName};
use crate::tool_capability::ToolLaunchFilter;

// ============ Simplified Mode (for HybridMode) ============

/// Simplified modes for composing HybridMode.
/// These represent the primary operational "facets" that can be combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimplifiedMode {
    /// SQL/database capability
    Sql,
    /// Python code execution capability
    Code,
    /// MCP tool orchestration capability
    Tool,
    /// RAG document retrieval capability
    Rag,
}

// ============ Operational Mode ============

/// The computed operational mode derived from settings flags.
/// This is the single source of truth for "what mode is the system in".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OperationalMode {
    /// No tools enabled - pure conversation
    Conversational,

    /// SQL/database focused: sql_select + schema_search + database_toolbox configured
    SqlMode {
        /// Whether schema_search is exposed as a tool (vs internal-only)
        schema_search_as_tool: bool,
        /// Whether schema_search runs automatically before prompting
        internal_schema_search: bool,
    },

    /// Python sandbox with tool calling: python_execution + code_mode primary
    CodeMode {
        /// Whether tool_search is enabled for discovering MCP tools
        tool_search_enabled: bool,
        /// Whether Python can call discovered tools
        python_tool_calling: bool,
    },

    /// MCP tool orchestration: native/hermes format + MCP servers enabled
    ToolMode {
        /// The primary tool calling format
        format: ToolCallFormatName,
        /// Whether tools are deferred (require tool_search discovery)
        deferred_discovery: bool,
    },

    /// Multiple capabilities enabled - composite mode
    HybridMode {
        /// Set of simplified modes that are active
        enabled_modes: HashSet<SimplifiedMode>,
        /// The primary format for tool calling
        primary_format: ToolCallFormatName,
    },
}

impl OperationalMode {
    /// Get the display name for the mode
    pub fn name(&self) -> &'static str {
        match self {
            OperationalMode::Conversational => "Conversational",
            OperationalMode::SqlMode { .. } => "SQL Mode",
            OperationalMode::CodeMode { .. } => "Code Mode",
            OperationalMode::ToolMode { .. } => "Tool Mode",
            OperationalMode::HybridMode { .. } => "Hybrid Mode",
        }
    }

    /// Check if SQL capabilities are active in this mode
    pub fn has_sql(&self) -> bool {
        match self {
            OperationalMode::SqlMode { .. } => true,
            OperationalMode::HybridMode { enabled_modes, .. } => {
                enabled_modes.contains(&SimplifiedMode::Sql)
            }
            _ => false,
        }
    }

    /// Check if Python execution is active in this mode
    pub fn has_code(&self) -> bool {
        match self {
            OperationalMode::CodeMode { .. } => true,
            OperationalMode::HybridMode { enabled_modes, .. } => {
                enabled_modes.contains(&SimplifiedMode::Code)
            }
            _ => false,
        }
    }

    /// Check if MCP tools are active in this mode
    pub fn has_tools(&self) -> bool {
        match self {
            OperationalMode::ToolMode { .. } => true,
            OperationalMode::HybridMode { enabled_modes, .. } => {
                enabled_modes.contains(&SimplifiedMode::Tool)
            }
            _ => false,
        }
    }

    /// Check if any tools are available in this mode
    pub fn has_any_tools(&self) -> bool {
        !matches!(self, OperationalMode::Conversational)
    }

    /// Get the primary tool calling format for this mode
    pub fn primary_format(&self) -> Option<ToolCallFormatName> {
        match self {
            OperationalMode::Conversational => None,
            OperationalMode::SqlMode { .. } => Some(ToolCallFormatName::Native),
            OperationalMode::CodeMode { .. } => Some(ToolCallFormatName::CodeMode),
            OperationalMode::ToolMode { format, .. } => Some(*format),
            OperationalMode::HybridMode { primary_format, .. } => Some(*primary_format),
        }
    }
}

// ============ Tool Availability ============

/// Per-chat-turn context that influences state machine computation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatTurnContext {
    /// Files/folders attached for RAG
    pub attached_files: Vec<String>,
    /// Database tables attached for SQL queries
    pub attached_tables: Vec<AttachedTableInfo>,
    /// Tools explicitly attached for this chat (built-in and MCP)
    pub attached_tools: Vec<String>,
    /// Tabular files attached for Python analysis (CSV, TSV, XLS, XLSX)
    pub attached_tabular_files: Vec<AttachedTabularFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedTableInfo {
    pub source_id: String,
    pub table_fq_name: String,
    pub column_count: usize,
    pub schema_text: Option<String>, // Full schema definition
}

/// Information about an attached tabular file (CSV, TSV, XLS, XLSX).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedTabularFile {
    /// Full path to the file
    pub file_path: String,
    /// File name for display
    pub file_name: String,
    /// Column headers
    pub headers: Vec<String>,
    /// Number of data rows
    pub row_count: usize,
    /// Variable index (1-indexed: headers1/rows1, headers2/rows2, etc.)
    pub variable_index: usize,
}

/// Configuration for a specific chat turn
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnConfiguration {
    pub mode: OperationalMode,
    pub enabled_tools: Vec<String>,
    pub schema_context: Option<String>, // SQL schemas to include in prompt
}

/// Describes which tools are available given the current settings.
/// This is a computed summary, not mutable state.
#[derive(Debug, Clone, Default)]
pub struct ToolAvailability {
    /// Built-in tools that are enabled
    pub enabled_builtins: HashSet<String>,
    /// Whether any MCP servers are enabled
    pub has_mcp_servers: bool,
    /// Number of enabled MCP servers
    pub mcp_server_count: usize,
    /// Whether database toolbox is enabled and configured
    pub database_configured: bool,
    /// Number of enabled database sources
    pub database_source_count: usize,
}

impl ToolAvailability {
    /// Check if a specific built-in tool is available
    pub fn is_builtin_available(&self, name: &str) -> bool {
        self.enabled_builtins.contains(name)
    }
}

// ============ Settings State Machine ============

/// The Settings State Machine computes the operational mode from settings.
///
/// This is Tier 1 of the three-tier state machine hierarchy. It runs when
/// settings change (rarely) and produces an OperationalMode that describes
/// the system's operational posture.
///
/// The SettingsStateMachine is the **single source of truth** for:
/// - Which capabilities are enabled
/// - What operational mode the system is in
/// - Which tools are available
#[derive(Debug, Clone)]
pub struct SettingsStateMachine {
    /// The computed operational mode
    current_mode: OperationalMode,
    /// Capabilities enabled by settings (independent of context)
    enabled_capabilities: HashSet<Capability>,
    /// Tool availability summary
    tool_availability: ToolAvailability,
    /// Relevancy thresholds (from settings)
    relevancy_thresholds: RelevancyThresholds,
}

/// Relevancy thresholds from settings (duplicated from agentic_state to avoid circular deps)
#[derive(Debug, Clone, Default)]
pub struct RelevancyThresholds {
    pub rag_chunk_min: f32,
    pub schema_relevancy: f32,
    pub rag_dominant_threshold: f32,
}

impl From<&AppSettings> for RelevancyThresholds {
    fn from(settings: &AppSettings) -> Self {
        Self {
            rag_chunk_min: settings.rag_chunk_min_relevancy,
            schema_relevancy: settings.schema_relevancy_threshold,
            rag_dominant_threshold: settings.rag_dominant_threshold,
        }
    }
}

impl SettingsStateMachine {
    /// Create a new SettingsStateMachine from settings and launch filter.
    ///
    /// This computes the operational mode, enabled capabilities, and tool
    /// availability from the current settings.
    pub fn from_settings(settings: &AppSettings, filter: &ToolLaunchFilter) -> Self {
        let enabled_capabilities = Self::compute_enabled_capabilities(settings, filter);
        let tool_availability = Self::compute_tool_availability(settings, filter);
        let current_mode =
            Self::compute_operational_mode(settings, filter, &enabled_capabilities, &tool_availability);
        let relevancy_thresholds = RelevancyThresholds::from(settings);

        Self {
            current_mode,
            enabled_capabilities,
            tool_availability,
            relevancy_thresholds,
        }
    }

    /// Get the current operational mode
    pub fn operational_mode(&self) -> &OperationalMode {
        &self.current_mode
    }

    /// Get the enabled capabilities
    pub fn enabled_capabilities(&self) -> &HashSet<Capability> {
        &self.enabled_capabilities
    }

    /// Get the tool availability summary
    pub fn tool_availability(&self) -> &ToolAvailability {
        &self.tool_availability
    }

    /// Get the relevancy thresholds
    pub fn relevancy_thresholds(&self) -> &RelevancyThresholds {
        &self.relevancy_thresholds
    }

    /// Check if a specific capability is enabled
    pub fn is_capability_enabled(&self, cap: Capability) -> bool {
        self.enabled_capabilities.contains(&cap)
    }

    /// Check if a built-in tool is available
    pub fn is_builtin_available(&self, name: &str) -> bool {
        self.tool_availability.is_builtin_available(name)
    }

    /// Compute operational mode and enabled tools for a specific chat turn
    pub fn compute_for_turn(
        &self,
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
        turn_context: &ChatTurnContext,
    ) -> TurnConfiguration {
        let mut enabled_modes = HashSet::new();
        let mut enabled_tools = Vec::new();

        // 1. RAG Mode
        if !turn_context.attached_files.is_empty() {
            enabled_modes.insert(SimplifiedMode::Rag);
        }

        // 2. SQL Mode
        if !turn_context.attached_tables.is_empty() {
            enabled_modes.insert(SimplifiedMode::Sql);
            // Implicitly enable sql_select if tables are attached
            if filter.builtin_allowed("sql_select") {
                enabled_tools.push("sql_select".to_string());
            }
        }

        // 2b. Tabular Data Analysis Mode (CSV/TSV/Excel files)
        if !turn_context.attached_tabular_files.is_empty() {
            enabled_modes.insert(SimplifiedMode::Code);
            // Implicitly enable python_execution for tabular data analysis
            if filter.builtin_allowed("python_execution") {
                if !enabled_tools.contains(&"python_execution".to_string()) {
                    enabled_tools.push("python_execution".to_string());
                }
            }
        }

        // 3. User-attached Tools
        for tool_key in &turn_context.attached_tools {
            // tool_key is "builtin::name" or "serverId::name"
            if tool_key.starts_with("builtin::") {
                let name = &tool_key["builtin::".len()..];
                if filter.builtin_allowed(name) {
                    // Check if it's already added (like sql_select)
                    if !enabled_tools.contains(&name.to_string()) {
                        enabled_tools.push(name.to_string());
                    }
                    if name == "python_execution" {
                        enabled_modes.insert(SimplifiedMode::Code);
                    }
                }
            } else if let Some(sep_idx) = tool_key.find("::") {
                let server_id = &tool_key[..sep_idx];
                if filter.server_allowed(server_id) {
                    // For MCP tools, we just pass the full key
                    enabled_tools.push(tool_key.clone());
                    enabled_modes.insert(SimplifiedMode::Tool);
                }
            }
        }

        // 4. Resolve Mode
        let mode = if enabled_modes.is_empty() {
            OperationalMode::Conversational
        } else if enabled_modes.len() > 1 {
            OperationalMode::HybridMode {
                enabled_modes,
                primary_format: settings.tool_call_formats.primary,
            }
        } else {
            let only_mode = enabled_modes.into_iter().next().unwrap();
            match only_mode {
                SimplifiedMode::Sql => OperationalMode::SqlMode {
                    schema_search_as_tool: enabled_tools.contains(&"schema_search".to_string()),
                    internal_schema_search: false, // Per-chat tables usually don't need internal search
                },
                SimplifiedMode::Code => OperationalMode::CodeMode {
                    tool_search_enabled: enabled_tools.contains(&"tool_search".to_string()),
                    python_tool_calling: settings.python_tool_calling_enabled,
                },
                SimplifiedMode::Tool => OperationalMode::ToolMode {
                    format: settings.tool_call_formats.primary,
                    deferred_discovery: false, // Per-chat tools are usually explicit
                },
                SimplifiedMode::Rag => OperationalMode::Conversational, // RAG doesn't have a specific mode yet
            }
        };

        // 5. Build schema context if tables attached
        let schema_context = if !turn_context.attached_tables.is_empty() {
            let mut ctx = String::from("Attached Database Table Schemas:\n\n");
            for table in &turn_context.attached_tables {
                if let Some(ref schema) = table.schema_text {
                    ctx.push_str(schema);
                    ctx.push_str("\n\n");
                } else {
                    ctx.push_str(&format!("Table: {} ({} columns)\n\n", table.table_fq_name, table.column_count));
                }
            }
            Some(ctx)
        } else {
            None
        };

        TurnConfiguration {
            mode,
            enabled_tools,
            schema_context,
        }
    }

    // ============ Private Computation Methods ============

    /// Compute which capabilities are enabled based on settings and filter.
    /// 
    /// Built-in tools require BOTH their *_enabled flag AND presence in always_on_builtin_tools
    /// to be considered globally enabled. Per-chat attached tools are handled separately
    /// in compute_for_turn().
    fn compute_enabled_capabilities(
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
    ) -> HashSet<Capability> {
        let mut caps = HashSet::new();
        let always_on = &settings.always_on_builtin_tools;

        // RAG is always available if we support attachments
        // (gated by actual attachment presence at runtime)
        caps.insert(Capability::Rag);

        // Schema search - requires presence in always_on_builtin_tools
        if always_on.contains(&"schema_search".to_string())
            && filter.builtin_allowed("schema_search") 
        {
            caps.insert(Capability::SchemaSearch);
        }

        // SQL query - requires presence in always_on_builtin_tools
        if always_on.contains(&"sql_select".to_string())
            && filter.builtin_allowed("sql_select") 
        {
            caps.insert(Capability::SqlQuery);
        }

        // Python execution - requires presence in always_on_builtin_tools
        if always_on.contains(&"python_execution".to_string())
            && filter.builtin_allowed("python_execution") 
        {
            caps.insert(Capability::PythonExecution);
        }

        // Tool search - requires presence in always_on_builtin_tools
        if always_on.contains(&"tool_search".to_string())
            && filter.builtin_allowed("tool_search") 
        {
            caps.insert(Capability::ToolSearch);
        }

        // MCP tools (if any non-database MCP servers are enabled)
        let has_enabled_mcp_servers = settings
            .mcp_servers
            .iter()
            .any(|s| s.enabled && filter.server_allowed(&s.id));

        if has_enabled_mcp_servers {
            caps.insert(Capability::McpTools);
        }

        caps
    }

    /// Compute tool availability from settings.
    /// 
    /// Built-in tools require BOTH their *_enabled flag AND presence in always_on_builtin_tools
    /// to be considered globally available.
    fn compute_tool_availability(
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
    ) -> ToolAvailability {
        let mut enabled_builtins = HashSet::new();
        let always_on = &settings.always_on_builtin_tools;

        // Check each built-in - requires presence in always_on_builtin_tools
        if always_on.contains(&"python_execution".to_string())
            && filter.builtin_allowed("python_execution")
            && settings
                .tool_call_formats
                .is_enabled(ToolCallFormatName::CodeMode)
        {
            enabled_builtins.insert("python_execution".to_string());
        }

        if always_on.contains(&"tool_search".to_string())
            && filter.builtin_allowed("tool_search") 
        {
            enabled_builtins.insert("tool_search".to_string());
        }

        // schema_search is only exposed as a tool if explicitly enabled
        // (internal schema search is auto-derived when sql_select is on but schema_search is off)
        if always_on.contains(&"schema_search".to_string())
            && filter.builtin_allowed("schema_search") 
        {
            enabled_builtins.insert("schema_search".to_string());
        }

        if always_on.contains(&"sql_select".to_string())
            && filter.builtin_allowed("sql_select") 
        {
            enabled_builtins.insert("sql_select".to_string());
        }

        // Count MCP servers
        let enabled_mcp_servers: Vec<_> = settings
            .mcp_servers
            .iter()
            .filter(|s| s.enabled && filter.server_allowed(&s.id))
            .collect();

        // Check database configuration
        let database_configured = settings.database_toolbox.enabled;
        let database_source_count = settings
            .database_toolbox
            .sources
            .iter()
            .filter(|s| s.enabled)
            .count();

        ToolAvailability {
            enabled_builtins,
            has_mcp_servers: !enabled_mcp_servers.is_empty(),
            mcp_server_count: enabled_mcp_servers.len(),
            database_configured,
            database_source_count,
        }
    }

    /// Compute the operational mode from settings and derived data.
    fn compute_operational_mode(
        settings: &AppSettings,
        _filter: &ToolLaunchFilter,
        capabilities: &HashSet<Capability>,
        tool_availability: &ToolAvailability,
    ) -> OperationalMode {
        // Count how many major modes are active
        let has_sql = capabilities.contains(&Capability::SqlQuery)
            || capabilities.contains(&Capability::SchemaSearch);
        let has_code = capabilities.contains(&Capability::PythonExecution);
        let has_tools = capabilities.contains(&Capability::McpTools);

        let mode_count = [has_sql, has_code, has_tools]
            .iter()
            .filter(|&&b| b)
            .count();

        // No tools enabled -> Conversational
        if mode_count == 0 {
            return OperationalMode::Conversational;
        }

        // Multiple modes -> HybridMode
        if mode_count > 1 {
            let mut enabled_modes = HashSet::new();
            if has_sql {
                enabled_modes.insert(SimplifiedMode::Sql);
            }
            if has_code {
                enabled_modes.insert(SimplifiedMode::Code);
            }
            if has_tools {
                enabled_modes.insert(SimplifiedMode::Tool);
            }

            let primary_format = settings.tool_call_formats.primary;

            return OperationalMode::HybridMode {
                enabled_modes,
                primary_format,
            };
        }

        // Single mode -> specific mode type
        if has_sql {
            return OperationalMode::SqlMode {
                schema_search_as_tool: tool_availability.is_builtin_available("schema_search"),
                // Internal schema search is auto-derived: ON when sql_select is enabled but schema_search is not
                internal_schema_search: settings.should_run_internal_schema_search(),
            };
        }

        if has_code {
            return OperationalMode::CodeMode {
                tool_search_enabled: capabilities.contains(&Capability::ToolSearch),
                python_tool_calling: settings.python_tool_calling_enabled,
            };
        }

        if has_tools {
            // Check if tools are deferred
            let deferred_discovery = tool_availability.is_builtin_available("tool_search")
                && settings.mcp_servers.iter().any(|s| s.enabled && s.defer_tools);

            return OperationalMode::ToolMode {
                format: settings.tool_call_formats.primary,
                deferred_discovery,
            };
        }

        // Fallback (shouldn't reach here)
        OperationalMode::Conversational
    }

    /// Refresh the state machine with new settings.
    /// Returns true if the operational mode changed.
    pub fn refresh(&mut self, settings: &AppSettings, filter: &ToolLaunchFilter) -> bool {
        let old_mode_name = self.current_mode.name();

        self.enabled_capabilities = Self::compute_enabled_capabilities(settings, filter);
        self.tool_availability = Self::compute_tool_availability(settings, filter);
        self.current_mode = Self::compute_operational_mode(
            settings,
            filter,
            &self.enabled_capabilities,
            &self.tool_availability,
        );
        self.relevancy_thresholds = RelevancyThresholds::from(settings);

        let new_mode_name = self.current_mode.name();
        let changed = old_mode_name != new_mode_name;

        if changed {
            println!(
                "[SettingsStateMachine] Mode changed: {} -> {}",
                old_mode_name, new_mode_name
            );
        }

        changed
    }
}

// ============ Tests ============

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AppSettings;

    fn default_filter() -> ToolLaunchFilter {
        ToolLaunchFilter::default()
    }

    #[test]
    fn test_conversational_mode_when_no_tools_enabled() {
        let settings = AppSettings::default();
        let filter = default_filter();

        let sm = SettingsStateMachine::from_settings(&settings, &filter);

        assert!(matches!(
            sm.operational_mode(),
            OperationalMode::Conversational
        ));
        assert!(!sm.operational_mode().has_any_tools());
    }

    #[test]
    fn test_sql_mode_when_only_sql_enabled() {
        let mut settings = AppSettings::default();
        settings.always_on_builtin_tools.push("sql_select".to_string());
        settings.always_on_builtin_tools.push("schema_search".to_string());
        settings.database_toolbox.enabled = true;

        let filter = default_filter();
        let sm = SettingsStateMachine::from_settings(&settings, &filter);

        assert!(matches!(sm.operational_mode(), OperationalMode::SqlMode { .. }));
        assert!(sm.operational_mode().has_sql());
        assert!(!sm.operational_mode().has_code());
    }

    #[test]
    fn test_code_mode_when_only_python_enabled() {
        let mut settings = AppSettings::default();
        settings.always_on_builtin_tools.push("python_execution".to_string());
        // Ensure code mode is in enabled formats
        settings.tool_call_formats.enabled.push(ToolCallFormatName::CodeMode);

        let filter = default_filter();
        let sm = SettingsStateMachine::from_settings(&settings, &filter);

        assert!(matches!(sm.operational_mode(), OperationalMode::CodeMode { .. }));
        assert!(sm.operational_mode().has_code());
        assert!(!sm.operational_mode().has_sql());
    }

    #[test]
    fn test_hybrid_mode_when_multiple_enabled() {
        let mut settings = AppSettings::default();
        settings.always_on_builtin_tools.push("python_execution".to_string());
        settings.always_on_builtin_tools.push("sql_select".to_string());
        settings.always_on_builtin_tools.push("schema_search".to_string());
        settings.database_toolbox.enabled = true;
        settings.tool_call_formats.enabled.push(ToolCallFormatName::CodeMode);

        let filter = default_filter();
        let sm = SettingsStateMachine::from_settings(&settings, &filter);

        match sm.operational_mode() {
            OperationalMode::HybridMode { enabled_modes, .. } => {
                assert!(enabled_modes.contains(&SimplifiedMode::Sql));
                assert!(enabled_modes.contains(&SimplifiedMode::Code));
            }
            _ => panic!("Expected HybridMode"),
        }

        assert!(sm.operational_mode().has_sql());
        assert!(sm.operational_mode().has_code());
    }

    #[test]
    fn test_capability_check() {
        let mut settings = AppSettings::default();
        settings.always_on_builtin_tools.push("python_execution".to_string());

        let filter = default_filter();
        let sm = SettingsStateMachine::from_settings(&settings, &filter);

        assert!(sm.is_capability_enabled(Capability::PythonExecution));
        assert!(sm.is_capability_enabled(Capability::Rag)); // Always enabled
        assert!(!sm.is_capability_enabled(Capability::SqlQuery));
    }

    #[test]
    fn test_tool_availability() {
        let mut settings = AppSettings::default();
        settings.always_on_builtin_tools.push("python_execution".to_string());
        settings.tool_call_formats.enabled.push(ToolCallFormatName::CodeMode);

        let filter = default_filter();
        let sm = SettingsStateMachine::from_settings(&settings, &filter);

        assert!(sm.is_builtin_available("python_execution"));
        assert!(!sm.is_builtin_available("sql_select"));
    }

    #[test]
    fn test_refresh_detects_mode_change() {
        let settings = AppSettings::default();
        let filter = default_filter();

        let mut sm = SettingsStateMachine::from_settings(&settings, &filter);
        assert!(matches!(
            sm.operational_mode(),
            OperationalMode::Conversational
        ));

        // Enable Python
        let mut new_settings = settings.clone();
        new_settings.always_on_builtin_tools.push("python_execution".to_string());
        new_settings.tool_call_formats.enabled.push(ToolCallFormatName::CodeMode);

        let changed = sm.refresh(&new_settings, &filter);
        assert!(changed);
        assert!(matches!(sm.operational_mode(), OperationalMode::CodeMode { .. }));
    }
}

