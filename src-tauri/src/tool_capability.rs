//! Tool Capability Resolver
//!
//! Centralizes all logic for determining which tools are available,
//! which formats to use, and how to present tools to models.
//!
//! This module works with the three-tier state machine hierarchy:
//! - SettingsStateMachine (Tier 1) provides enabled capabilities
//! - AgenticStateMachine (Tier 2) provides context-aware tool availability
//! - MidTurnStateMachine (Tier 3) manages tool execution state

use crate::agentic_state::Capability;
use crate::protocol::{ModelInfo, ToolFormat, ToolSchema};
use crate::settings::{
    AppSettings, DatabaseToolboxConfig, McpServerConfig, ToolCallFormatConfig, ToolCallFormatName,
};
use crate::settings_state_machine::SettingsStateMachine;
use crate::state_machine::AgenticStateMachine;
use crate::tool_registry::ToolRegistry;
use std::collections::{HashMap, HashSet};

/// Built-in tool names
pub const BUILTIN_PYTHON_EXECUTION: &str = "python_execution";
pub const BUILTIN_TOOL_SEARCH: &str = "tool_search";
pub const BUILTIN_SCHEMA_SEARCH: &str = "schema_search";
pub const BUILTIN_SQL_SELECT: &str = "sql_select";

/// All built-in tool names
pub const ALL_BUILTINS: &[&str] = &[
    BUILTIN_PYTHON_EXECUTION,
    BUILTIN_TOOL_SEARCH,
    BUILTIN_SCHEMA_SEARCH,
    BUILTIN_SQL_SELECT,
];

/// Resolved tool capabilities for a specific context
#[derive(Debug, Clone)]
pub struct ResolvedToolCapabilities {
    /// Which built-in tools are available
    pub available_builtins: HashSet<String>,
    
    /// Format selection
    pub primary_format: ToolCallFormatName,
    pub enabled_formats: Vec<ToolCallFormatName>,
    pub use_native_tools: bool,
    
    /// MCP tools
    pub active_mcp_tools: Vec<(String, ToolSchema)>,  // Materialized/non-deferred
    pub deferred_mcp_tools: Vec<(String, ToolSchema)>, // Need tool_search
    
    /// Model info
    pub model_supports_native: bool,
    pub model_tool_format: ToolFormat,
    
    /// Max MCP tools to include in prompt (based on model size)
    pub max_mcp_tools_in_prompt: usize,
}

/// Launch-time tool filter (from CLI args)
/// Used to restrict which tools are available at runtime
#[derive(Debug, Clone, Default)]
pub struct ToolLaunchFilter {
    pub allowed_builtins: Option<HashSet<String>>,
    pub allowed_servers: Option<HashSet<String>>,
    pub allowed_tools: Option<HashSet<(String, String)>>,
}

impl ToolLaunchFilter {
    pub fn allow_all(&self) -> bool {
        self.allowed_builtins.is_none()
            && self.allowed_servers.is_none()
            && self.allowed_tools.is_none()
    }

    pub fn builtin_allowed(&self, name: &str) -> bool {
        match &self.allowed_builtins {
            None => true,
            Some(set) => set.contains(name),
        }
    }

    pub fn server_allowed(&self, server_id: &str) -> bool {
        match &self.allowed_servers {
            None => true,
            Some(set) => set.contains(server_id),
        }
    }

    pub fn tool_allowed(&self, server_id: &str, tool_name: &str) -> bool {
        if let Some(tools) = &self.allowed_tools {
            if !tools.contains(&(server_id.to_string(), tool_name.to_string())) {
                return false;
            }
        }
        self.server_allowed(server_id)
    }
}

/// Central resolver for tool capabilities
pub struct ToolCapabilityResolver;

impl ToolCapabilityResolver {
    /// Resolve all tool capabilities for the current context
    pub fn resolve(
        settings: &AppSettings,
        model_info: &ModelInfo,
        filter: &ToolLaunchFilter,
        server_configs: &[McpServerConfig],
        tool_registry: &ToolRegistry,
    ) -> ResolvedToolCapabilities {
        // Extract enabled built-ins from settings
        // TODO: Migrate to list-based enabled_builtins field
        let enabled_builtins = Self::extract_enabled_builtins(settings);
        
        // Get visible tools from registry (includes materialized deferred tools)
        let visible_tools = tool_registry.get_visible_tools_with_servers();
        
        // Determine which built-ins are available
        let available_builtins = Self::determine_available_builtins(
            &enabled_builtins,
            settings,
            filter,
            tool_registry,
        );
        
        // Select primary format with fallback
        let (primary_format, enabled_formats) = Self::select_formats(
            &settings.tool_call_formats,
            &available_builtins,
            model_info,
        );
        
        // Determine if we should use native tools
        let use_native_tools = primary_format == ToolCallFormatName::Native
            && model_info.tool_calling;
        
        // Separate MCP tools into active (materialized) and deferred
        let (active_mcp_tools, deferred_mcp_tools) = Self::categorize_mcp_tools(
            &visible_tools,
            tool_registry,
            server_configs,
            filter,
        );
        
        // Calculate max MCP tools in prompt based on model size
        let max_mcp_tools_in_prompt = Self::calculate_max_mcp_tools(model_info);
        
        ResolvedToolCapabilities {
            available_builtins,
            primary_format,
            enabled_formats,
            use_native_tools,
            active_mcp_tools,
            deferred_mcp_tools,
            model_supports_native: model_info.tool_calling,
            model_tool_format: model_info.tool_format,
            max_mcp_tools_in_prompt,
        }
    }
    
    /// Extract enabled built-ins from settings (temporary migration helper)
    /// TODO: Replace with direct access to settings.enabled_builtins once migrated
    fn extract_enabled_builtins(settings: &AppSettings) -> HashSet<String> {
        let mut enabled = HashSet::new();
        
        // Check individual boolean flags (current structure)
        if settings.python_execution_enabled {
            enabled.insert(BUILTIN_PYTHON_EXECUTION.to_string());
        }
        if settings.tool_search_enabled {
            enabled.insert(BUILTIN_TOOL_SEARCH.to_string());
        }
        if settings.schema_search_enabled {
            enabled.insert(BUILTIN_SCHEMA_SEARCH.to_string());
        }
        if settings.sql_select_enabled {
            enabled.insert(BUILTIN_SQL_SELECT.to_string());
        }
        
        enabled
    }
    
    /// Determine which built-ins are actually available (pass all checks)
    fn determine_available_builtins(
        enabled_builtins: &HashSet<String>,
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
        tool_registry: &ToolRegistry,
    ) -> HashSet<String> {
        let mut available = HashSet::new();
        
        // Check python_execution
        if enabled_builtins.contains(BUILTIN_PYTHON_EXECUTION)
            && filter.builtin_allowed(BUILTIN_PYTHON_EXECUTION)
            && settings.tool_call_formats.is_enabled(ToolCallFormatName::CodeMode)
        {
            available.insert(BUILTIN_PYTHON_EXECUTION.to_string());
        }
        
        // Check tool_search
        if enabled_builtins.contains(BUILTIN_TOOL_SEARCH)
            && filter.builtin_allowed(BUILTIN_TOOL_SEARCH)
            && Self::has_deferred_mcp_tools(tool_registry, settings)
        {
            available.insert(BUILTIN_TOOL_SEARCH.to_string());
        }
        
        // Check schema_search - only exposed as a tool if explicitly enabled
        // (internal schema search is auto-derived when sql_select is on but schema_search is off)
        if settings.schema_search_enabled
            && filter.builtin_allowed(BUILTIN_SCHEMA_SEARCH)
            && Self::has_enabled_database_sources(&settings.database_toolbox)
        {
            available.insert(BUILTIN_SCHEMA_SEARCH.to_string());
        }
        
        // Check sql_select
        if enabled_builtins.contains(BUILTIN_SQL_SELECT)
            && filter.builtin_allowed(BUILTIN_SQL_SELECT)
            && Self::has_enabled_database_sources(&settings.database_toolbox)
        {
            available.insert(BUILTIN_SQL_SELECT.to_string());
        }
        
        available
    }
    
    /// Check if there are any deferred MCP tools (excluding database sources)
    fn has_deferred_mcp_tools(tool_registry: &ToolRegistry, settings: &AppSettings) -> bool {
        let deferred = tool_registry.get_deferred_tools();
        if deferred.is_empty() {
            return false;
        }

        let configs = settings.get_all_mcp_configs();
        let server_map: HashMap<String, &McpServerConfig> =
            configs.iter().map(|c| (c.id.clone(), c)).collect();

        deferred.iter().any(|(key, _)| {
            let server_id = key.splitn(2, "___").next().unwrap_or("unknown");
            match server_map.get(server_id) {
                Some(c) => c.enabled && !c.is_database_source,
                None => false,
            }
        })
    }
    
    /// Check if database toolbox has enabled sources
    fn has_enabled_database_sources(config: &DatabaseToolboxConfig) -> bool {
        config.enabled
            && config.sources.iter().any(|s| s.enabled)
    }
    
    /// Select primary format with fallback logic
    fn select_formats(
        format_config: &ToolCallFormatConfig,
        available_builtins: &HashSet<String>,
        model_info: &ModelInfo,
    ) -> (ToolCallFormatName, Vec<ToolCallFormatName>) {
        let enabled_formats = format_config.enabled.clone();
        
        // Resolve primary format with availability checks
        let primary = format_config.resolve_primary_for_prompt(
            available_builtins.contains(BUILTIN_PYTHON_EXECUTION), // code_mode_available
            model_info.tool_calling, // native_available
        );
        
        (primary, enabled_formats)
    }
    
    /// Categorize MCP tools into active (materialized) and deferred
    /// All MCP tools are deferred by default - only materialized ones are active
    fn categorize_mcp_tools(
        _visible_tools: &[(String, ToolSchema)],
        tool_registry: &ToolRegistry,
        server_configs: &[McpServerConfig],
        filter: &ToolLaunchFilter,
    ) -> (Vec<(String, ToolSchema)>, Vec<(String, ToolSchema)>) {
        let mut active = Vec::new();
        let mut deferred = Vec::new();

        // Build server config lookup
        let server_map: HashMap<String, &McpServerConfig> = server_configs
            .iter()
            .map(|c| (c.id.clone(), c))
            .collect();

        // Get all deferred tool keys from registry (for checking materialization)
        let deferred_tool_keys: HashSet<String> = tool_registry
            .get_deferred_tools()
            .iter()
            .map(|(key, _)| (*key).clone())
            .collect();

        // Iterate over ALL domain tools in the registry, not just visible ones
        for (key, schema) in tool_registry.get_all_domain_tools() {
            // key format: server_id___tool_name
            let server_id = key.splitn(2, "___").next().unwrap_or("unknown");

            // Check server is enabled and allowed, and NOT a database source
            // (Database sources are handled via sql_select/schema_search built-ins)
            let server_config = match server_map.get(server_id) {
                Some(c) if c.enabled && !c.is_database_source && filter.server_allowed(server_id) => c,
                _ => continue,
            };

            // Check tool is allowed
            if !filter.tool_allowed(server_id, &schema.name) {
                continue;
            }

            // Check if tool is materialized (visible but was originally deferred)
            let is_materialized = tool_registry.is_tool_visible(server_id, &schema.name) && deferred_tool_keys.contains(key);

            if is_materialized {
                // This tool was deferred but is now visible (materialized)
                active.push((server_id.to_string(), schema.clone()));
            } else if schema.defer_loading && server_config.defer_tools {
                // Still deferred
                deferred.push((server_id.to_string(), schema.clone()));
            } else {
                // Active tool (non-deferred or forced active by server config)
                active.push((server_id.to_string(), schema.clone()));
            }
        }

        (active, deferred)
    }
    
    /// Calculate max MCP tools to include in prompt based on model size
    fn calculate_max_mcp_tools(_model_info: &ModelInfo) -> usize {
        // Default to 2 for small models
        // Could be enhanced to check actual model size, but for now use default
        2
    }
    
    /// Check if a built-in tool should be included in the prompt
    pub fn should_include_builtin(
        tool_name: &str,
        capabilities: &ResolvedToolCapabilities,
    ) -> bool {
        capabilities.available_builtins.contains(tool_name)
    }
    
    /// Check if an MCP tool should be included in the prompt
    pub fn should_include_mcp_tool(
        server_id: &str,
        tool_name: &str,
        capabilities: &ResolvedToolCapabilities,
    ) -> bool {
        // Only include active (materialized) MCP tools
        capabilities
            .active_mcp_tools
            .iter()
            .any(|(s, schema)| s == server_id && schema.name == tool_name)
    }
    
    /// Generate format-specific prompt instructions
    pub fn get_prompt_format_instructions(
        primary_format: ToolCallFormatName,
        model_tool_format: ToolFormat,
    ) -> Option<String> {
        // Even if primary is Native, we provide instructions if the model family has a preferred tag format.
        // Local models (like Phi, Qwen, Granite) often need the explicit tag to trigger tool calling.
        let effective_format = if primary_format == ToolCallFormatName::Native {
            match model_tool_format {
                ToolFormat::Hermes => ToolCallFormatName::Hermes,
                ToolFormat::Granite => ToolCallFormatName::Mistral, // Will add Granite specific below
                _ => ToolCallFormatName::Native,
            }
        } else {
            primary_format
        };

        match effective_format {
            ToolCallFormatName::Native => None, // Truly native models (like GPT-4) don't need instructions
            ToolCallFormatName::Hermes => Some(
                "## Tool Calling Format\n\nWhen you need to use a tool, output ONLY:\n<tool_call>{\"name\": \"tool_name\", \"arguments\": {...}}</tool_call>".to_string(),
            ),
            ToolCallFormatName::Mistral => {
                match model_tool_format {
                    ToolFormat::Granite => Some(
                        "## Function Calling Format\n\nWhen you need to call a function, output:\n<function_call>{\"name\": \"function_name\", \"arguments\": {...}}</function_call>".to_string()
                    ),
                    _ => Some(
                        "## Tool Calling Format\n\nWhen you need to use a tool, output:\n[TOOL_CALLS] [{\"name\": \"tool_name\", \"arguments\": {...}}]".to_string(),
                    )
                }
            },
            ToolCallFormatName::Pythonic => Some(
                "## Tool Calling Format\n\nWhen you need to use a tool, output:\ntool_name(arg1=\"value\", arg2=123)".to_string(),
            ),
            ToolCallFormatName::PureJson => Some(
                "## Tool Calling Format\n\nWhen you need to use a tool, output a JSON object:\n{\"name\": \"tool_name\", \"arguments\": {...}}".to_string(),
            ),
            ToolCallFormatName::CodeMode => None, // Code mode has its own prompt section
        }
    }

    // ============ State Machine Integration ============

    /// Check if a tool is allowed based on the state machine's current state.
    /// 
    /// This delegates to the state machine for tool validation, ensuring
    /// tools are only available when appropriate for the current context.
    pub fn is_tool_allowed_by_state(
        state_machine: &AgenticStateMachine,
        tool_name: &str,
    ) -> bool {
        state_machine.is_tool_allowed(tool_name)
    }

    /// Get the list of allowed tool names from the state machine.
    pub fn get_allowed_tools_from_state(
        state_machine: &AgenticStateMachine,
    ) -> Vec<String> {
        state_machine.allowed_tool_names()
    }

    /// Convert state machine capabilities to available builtins set.
    /// 
    /// This provides backwards compatibility with code that expects
    /// the available_builtins HashSet.
    pub fn capabilities_to_builtins(
        capabilities: &HashSet<Capability>,
    ) -> HashSet<String> {
        let mut builtins = HashSet::new();
        
        if capabilities.contains(&Capability::PythonExecution) {
            builtins.insert(BUILTIN_PYTHON_EXECUTION.to_string());
        }
        if capabilities.contains(&Capability::ToolSearch) {
            builtins.insert(BUILTIN_TOOL_SEARCH.to_string());
        }
        if capabilities.contains(&Capability::SchemaSearch) {
            builtins.insert(BUILTIN_SCHEMA_SEARCH.to_string());
        }
        if capabilities.contains(&Capability::SqlQuery) {
            builtins.insert(BUILTIN_SQL_SELECT.to_string());
        }
        
        builtins
    }

    /// Update ResolvedToolCapabilities based on state machine.
    /// 
    /// This can be used to overlay state machine restrictions on top
    /// of the standard capability resolution.
    pub fn apply_state_machine_filter(
        mut capabilities: ResolvedToolCapabilities,
        state_machine: &AgenticStateMachine,
    ) -> ResolvedToolCapabilities {
        // Filter available builtins based on state
        let allowed_tools = state_machine.allowed_tool_names();
        let allowed_set: HashSet<String> = allowed_tools.into_iter().collect();
        
        capabilities.available_builtins = capabilities
            .available_builtins
            .intersection(&allowed_set)
            .cloned()
            .collect();
        
        // Filter active MCP tools based on state
        capabilities.active_mcp_tools = capabilities
            .active_mcp_tools
            .into_iter()
            .filter(|(_, schema)| state_machine.is_tool_allowed(&schema.name))
            .collect();
        
        capabilities
    }

    // ============ SettingsStateMachine Integration (Three-Tier Hierarchy) ============

    /// Resolve tool capabilities using the SettingsStateMachine (Tier 1).
    /// 
    /// This is the preferred method for the three-tier architecture.
    /// It delegates capability checking to the SettingsStateMachine.
    pub fn resolve_from_settings_sm(
        settings_sm: &SettingsStateMachine,
        model_info: &ModelInfo,
        settings: &AppSettings,
        server_configs: &[McpServerConfig],
        tool_registry: &ToolRegistry,
    ) -> ResolvedToolCapabilities {
        // Get available builtins from SettingsStateMachine
        let available_builtins = settings_sm.tool_availability().enabled_builtins.clone();
        
        // Get enabled capabilities from SettingsStateMachine
        let enabled_capabilities = settings_sm.enabled_capabilities();
        
        // Select primary format with fallback
        let code_mode_available = enabled_capabilities.contains(&Capability::PythonExecution);
        let (primary_format, enabled_formats) = (
            settings.tool_call_formats.resolve_primary_for_prompt(
                code_mode_available,
                model_info.tool_calling,
            ),
            settings.tool_call_formats.enabled.clone(),
        );
        
        // Determine if we should use native tools
        let use_native_tools = primary_format == ToolCallFormatName::Native
            && model_info.tool_calling;
        
        // Get filter from tool availability - for now we use a permissive filter
        // since SettingsStateMachine already applied the filter
        let filter = ToolLaunchFilter::default();
        
        // Separate MCP tools into active (materialized) and deferred
        let visible_tools = tool_registry.get_visible_tools_with_servers();
        let (active_mcp_tools, deferred_mcp_tools) = Self::categorize_mcp_tools(
            &visible_tools,
            tool_registry,
            server_configs,
            &filter,
        );
        
        // Calculate max MCP tools in prompt based on model size
        let max_mcp_tools_in_prompt = Self::calculate_max_mcp_tools(model_info);
        
        ResolvedToolCapabilities {
            available_builtins,
            primary_format,
            enabled_formats,
            use_native_tools,
            active_mcp_tools,
            deferred_mcp_tools,
            model_supports_native: model_info.tool_calling,
            model_tool_format: model_info.tool_format,
            max_mcp_tools_in_prompt,
        }
    }

    /// Get enabled capabilities from SettingsStateMachine.
    pub fn get_enabled_capabilities_from_sm(
        settings_sm: &SettingsStateMachine,
    ) -> HashSet<Capability> {
        settings_sm.enabled_capabilities().clone()
    }

    /// Get available builtins from SettingsStateMachine.
    pub fn get_available_builtins_from_sm(
        settings_sm: &SettingsStateMachine,
    ) -> HashSet<String> {
        settings_sm.tool_availability().enabled_builtins.clone()
    }

    /// Check if a capability is enabled via SettingsStateMachine.
    pub fn is_capability_enabled_from_sm(
        settings_sm: &SettingsStateMachine,
        capability: Capability,
    ) -> bool {
        settings_sm.is_capability_enabled(capability)
    }
}

