//! Tool Capability Resolver
//!
//! Centralizes all logic for determining which tools are available,
//! which formats to use, and how to present tools to models.

use crate::protocol::{ModelInfo, ToolFormat, ToolSchema};
use crate::settings::{
    AppSettings, DatabaseToolboxConfig, McpServerConfig, ToolCallFormatConfig, ToolCallFormatName,
};
use crate::tool_registry::ToolRegistry;
use std::collections::{HashMap, HashSet};

/// Built-in tool names
pub const BUILTIN_PYTHON_EXECUTION: &str = "python_execution";
pub const BUILTIN_TOOL_SEARCH: &str = "tool_search";
pub const BUILTIN_SEARCH_SCHEMAS: &str = "search_schemas";
pub const BUILTIN_EXECUTE_SQL: &str = "execute_sql";

/// All built-in tool names
pub const ALL_BUILTINS: &[&str] = &[
    BUILTIN_PYTHON_EXECUTION,
    BUILTIN_TOOL_SEARCH,
    BUILTIN_SEARCH_SCHEMAS,
    BUILTIN_EXECUTE_SQL,
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
        if settings.search_schemas_enabled {
            enabled.insert(BUILTIN_SEARCH_SCHEMAS.to_string());
        }
        if settings.execute_sql_enabled {
            enabled.insert(BUILTIN_EXECUTE_SQL.to_string());
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
            && Self::has_deferred_mcp_tools(tool_registry)
        {
            available.insert(BUILTIN_TOOL_SEARCH.to_string());
        }
        
        // Check search_schemas
        if enabled_builtins.contains(BUILTIN_SEARCH_SCHEMAS)
            && filter.builtin_allowed(BUILTIN_SEARCH_SCHEMAS)
            && Self::has_enabled_database_sources(&settings.database_toolbox)
        {
            available.insert(BUILTIN_SEARCH_SCHEMAS.to_string());
        }
        
        // Check execute_sql
        if enabled_builtins.contains(BUILTIN_EXECUTE_SQL)
            && filter.builtin_allowed(BUILTIN_EXECUTE_SQL)
            && Self::has_enabled_database_sources(&settings.database_toolbox)
        {
            available.insert(BUILTIN_EXECUTE_SQL.to_string());
        }
        
        available
    }
    
    /// Check if there are any deferred MCP tools
    fn has_deferred_mcp_tools(tool_registry: &ToolRegistry) -> bool {
        !tool_registry.get_deferred_tools().is_empty()
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
        visible_tools: &[(String, ToolSchema)],
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
        
        // Get all deferred tools from registry
        let deferred_tool_keys: HashSet<String> = tool_registry
            .get_deferred_tools()
            .iter()
            .map(|(key, _)| (*key).clone())
            .collect();
        
        for (server_id, schema) in visible_tools {
            // Skip built-in tools
            if server_id == "builtin" {
                continue;
            }
            
            // Check server is enabled and allowed
            let _server_config = match server_map.get(server_id) {
                Some(c) if c.enabled && filter.server_allowed(server_id) => c,
                _ => continue,
            };
            
            // Check tool is allowed
            if !filter.tool_allowed(server_id, &schema.name) {
                continue;
            }
            
            // Check if tool is materialized (visible but was originally deferred)
            let tool_key = format!("{}___{}", server_id, schema.name);
            if deferred_tool_keys.contains(&tool_key) {
                // This tool was deferred but is now visible (materialized)
                active.push((server_id.clone(), schema.clone()));
            } else if schema.defer_loading {
                // Still deferred
                deferred.push((server_id.clone(), schema.clone()));
            } else {
                // Non-deferred tool (shouldn't happen with new architecture, but handle gracefully)
                active.push((server_id.clone(), schema.clone()));
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
    ) -> Option<String> {
        match primary_format {
            ToolCallFormatName::Native => None, // Native tools don't need format instructions
            ToolCallFormatName::Hermes => Some(
                "When you need to use a tool, output ONLY:\n<tool_call>{\"name\": \"tool_name\", \"arguments\": {...}}</tool_call>".to_string(),
            ),
            ToolCallFormatName::Mistral => Some(
                "When you need to use a tool, output:\n[TOOL_CALLS] [{\"name\": \"tool_name\", \"arguments\": {...}}]".to_string(),
            ),
            ToolCallFormatName::Pythonic => Some(
                "When you need to use a tool, output:\ntool_name(arg1=\"value\", arg2=123)".to_string(),
            ),
            ToolCallFormatName::PureJson => Some(
                "When you need to use a tool, output a JSON object:\n{\"name\": \"tool_name\", \"arguments\": {...}}".to_string(),
            ),
            ToolCallFormatName::CodeMode => None, // Code mode has its own prompt section
        }
    }
}

