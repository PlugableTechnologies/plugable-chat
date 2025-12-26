//! Agentic State Machine Controller
//!
//! Tier 2 of the three-tier state machine hierarchy:
//! 1. SettingsStateMachine - Settings -> OperationalMode
//! 2. AgenticStateMachine (this module) - OperationalMode + Context -> AgenticState
//! 3. MidTurnStateMachine - AgenticState + Events -> MidTurnState
//!
//! The AgenticStateMachine manages state computation, transitions, tool availability,
//! and prompt generation for each turn.
//!
//! ## Architecture
//!
//! The state machine is the **single source of truth** for:
//! - What tools are allowed in the current state
//! - What the system prompt should contain
//!
//! Enabled capabilities are now provided by the SettingsStateMachine.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::agentic_state::{
    AgenticState, Capability, McpToolContext, PromptContext, 
    RagChunk, RelevancyThresholds, StateEvent, TableInfo,
};
use crate::protocol::ToolSchema;
use crate::settings::{AppSettings, ToolCallFormatName};
use crate::settings_state_machine::{OperationalMode, SettingsStateMachine};
use crate::tool_capability::ToolLaunchFilter;

// ============ State Machine ============

/// Tier 2 state machine controller for the agentic loop.
/// 
/// This is the turn-level state machine that manages:
/// - Computing initial state from OperationalMode and context
/// - State transitions after tool execution
/// - Tool availability validation
/// - System prompt generation (single source of truth)
///
/// ## Three-Tier Hierarchy
///
/// The AgenticStateMachine receives its enabled capabilities from
/// the SettingsStateMachine (Tier 1) and produces states that can
/// be consumed by the MidTurnStateMachine (Tier 3).
///
/// ## Prompt Generation
///
/// The state machine is responsible for generating the full system prompt.
/// This ensures that what we tell the model matches what we allow:
/// - Capabilities section reflects `enabled_capabilities` (from SettingsStateMachine)
/// - Tool sections only show allowed tools
/// - Format instructions match `tool_call_format`
#[derive(Debug, Clone)]
pub struct AgenticStateMachine {
    /// Current state of the machine
    current_state: AgenticState,
    /// Capabilities enabled by settings (provided by SettingsStateMachine)
    enabled_capabilities: HashSet<Capability>,
    /// Relevancy thresholds for state transitions
    thresholds: RelevancyThresholds,
    /// History of states for debugging
    state_history: Vec<AgenticState>,
    /// Base system prompt (user-configured)
    base_prompt: String,
    
    // === Prompt Context (for unified prompt generation) ===
    
    /// MCP tool context (active/deferred tools, server info)
    mcp_context: McpToolContext,
    /// Tool call format to use for instructions
    tool_call_format: ToolCallFormatName,
    /// Custom prompts per tool (key: "server_id::tool_name")
    custom_tool_prompts: HashMap<String, String>,
    /// Whether Python is the primary tool calling mode (Code Mode)
    python_primary: bool,
    /// Whether user has attached documents
    has_attachments: bool,
    
    // === Auto-Discovery Context ===
    
    /// Auto-discovered tools from tool_search (for this turn)
    auto_tool_search: Option<crate::tools::tool_search::ToolSearchOutput>,
    /// Auto-discovered schema from schema_search (for this turn)
    auto_schema_search: Option<crate::tools::schema_search::SchemaSearchOutput>,
}

impl AgenticStateMachine {
    /// Create a new state machine from SettingsStateMachine and prompt context.
    /// 
    /// This is the **preferred constructor** for the three-tier architecture.
    /// It takes capabilities and thresholds from the SettingsStateMachine (Tier 1)
    /// and creates a turn-level state machine (Tier 2).
    ///
    /// # Arguments
    /// * `settings_sm` - The SettingsStateMachine providing capabilities and thresholds
    /// * `prompt_context` - Context for prompt generation (MCP tools, format, etc.)
    pub fn new_from_settings_sm(
        settings_sm: &SettingsStateMachine,
        prompt_context: PromptContext,
    ) -> Self {
        let enabled_capabilities = settings_sm.enabled_capabilities().clone();
        let thresholds = RelevancyThresholds {
            rag_chunk_min: settings_sm.relevancy_thresholds().rag_chunk_min,
            schema_table_min: settings_sm.relevancy_thresholds().schema_table_min,
            sql_enable_min: settings_sm.relevancy_thresholds().sql_enable_min,
            rag_dominant_threshold: settings_sm.relevancy_thresholds().rag_dominant_threshold,
        };
        
        // Determine initial state based on operational mode
        let initial_state = Self::compute_initial_state_from_mode(
            settings_sm.operational_mode(),
            &enabled_capabilities,
        );
        
        Self {
            current_state: initial_state,
            enabled_capabilities,
            thresholds,
            state_history: Vec::new(),
            base_prompt: prompt_context.base_prompt,
            mcp_context: prompt_context.mcp_context,
            tool_call_format: prompt_context.tool_call_format,
            custom_tool_prompts: prompt_context.custom_tool_prompts,
            python_primary: prompt_context.python_primary,
            has_attachments: prompt_context.has_attachments,
            auto_tool_search: None,
            auto_schema_search: None,
        }
    }

    /// Compute initial state from OperationalMode.
    /// 
    /// This maps the high-level OperationalMode to the appropriate
    /// turn-start AgenticState.
    fn compute_initial_state_from_mode(
        mode: &OperationalMode,
        enabled_capabilities: &HashSet<Capability>,
    ) -> AgenticState {
        match mode {
            OperationalMode::Conversational => AgenticState::Conversational,
            
            OperationalMode::SqlMode { .. } => AgenticState::SqlRetrieval {
                discovered_tables: Vec::new(),
                max_table_relevancy: 0.0,
            },
            
            OperationalMode::CodeMode { .. } => AgenticState::CodeExecution {
                available_tools: Vec::new(),
            },
            
            OperationalMode::ToolMode { .. } => AgenticState::ToolOrchestration {
                materialized_tools: Vec::new(),
            },
            
            OperationalMode::HybridMode { enabled_modes, .. } => {
                // For hybrid mode, use priority order based on which modes are enabled
                use crate::settings_state_machine::SimplifiedMode;
                
                if enabled_modes.contains(&SimplifiedMode::Code) {
                    AgenticState::CodeExecution {
                        available_tools: Vec::new(),
                    }
                } else if enabled_modes.contains(&SimplifiedMode::Tool) {
                    AgenticState::ToolOrchestration {
                        materialized_tools: Vec::new(),
                    }
                } else if enabled_modes.contains(&SimplifiedMode::Sql) {
                    AgenticState::SqlRetrieval {
                        discovered_tables: Vec::new(),
                        max_table_relevancy: 0.0,
                    }
                } else {
                    // Fallback to capability-based selection
                    Self::compute_default_initial_state(enabled_capabilities)
                }
            }
        }
    }

    /// Create a new state machine with minimal context (legacy constructor).
    /// 
    /// **Deprecated**: Prefer `new_from_settings_sm()` for the three-tier architecture.
    pub fn new(
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
        thresholds: RelevancyThresholds,
        base_prompt: String,
    ) -> Self {
        let enabled_capabilities = Self::compute_enabled_capabilities(settings, filter);
        let initial_state = Self::compute_default_initial_state(&enabled_capabilities);
        
        Self {
            current_state: initial_state,
            enabled_capabilities,
            thresholds,
            state_history: Vec::new(),
            base_prompt,
            // Default context (minimal)
            mcp_context: McpToolContext::default(),
            tool_call_format: ToolCallFormatName::Hermes,
            custom_tool_prompts: HashMap::new(),
            python_primary: settings.python_execution_enabled,
            has_attachments: false,
            auto_tool_search: None,
            auto_schema_search: None,
        }
    }

    /// Create a new state machine with full prompt context (legacy constructor).
    /// 
    /// **Deprecated**: Prefer `new_from_settings_sm()` for the three-tier architecture.
    pub fn new_with_context(
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
        thresholds: RelevancyThresholds,
        prompt_context: PromptContext,
    ) -> Self {
        let enabled_capabilities = Self::compute_enabled_capabilities(settings, filter);
        
        // Determine initial state based on enabled capabilities.
        // If no RAG/Schema context is available yet (we don't know relevancy scores),
        // we should start in a state that allows tool execution based on capabilities.
        let initial_state = Self::compute_default_initial_state(&enabled_capabilities);
        
        Self {
            current_state: initial_state,
            enabled_capabilities,
            thresholds,
            state_history: Vec::new(),
            base_prompt: prompt_context.base_prompt,
            mcp_context: prompt_context.mcp_context,
            tool_call_format: prompt_context.tool_call_format,
            custom_tool_prompts: prompt_context.custom_tool_prompts,
            python_primary: prompt_context.python_primary,
            has_attachments: prompt_context.has_attachments,
            auto_tool_search: None,
            auto_schema_search: None,
        }
    }
    
    /// Compute the default initial state based on enabled capabilities.
    /// 
    /// This is used when creating the state machine before RAG/Schema relevancy
    /// scores are known. It ensures tools are allowed based on what's enabled.
    fn compute_default_initial_state(enabled_capabilities: &HashSet<Capability>) -> AgenticState {
        // Priority order: CodeExecution > ToolOrchestration > SqlRetrieval > Conversational
        if enabled_capabilities.contains(&Capability::PythonExecution) {
            AgenticState::CodeExecution {
                available_tools: Vec::new(),
            }
        } else if enabled_capabilities.contains(&Capability::McpTools) 
            || enabled_capabilities.contains(&Capability::ToolSearch) {
            AgenticState::ToolOrchestration {
                materialized_tools: Vec::new(),
            }
        } else if enabled_capabilities.contains(&Capability::SqlQuery) 
            || enabled_capabilities.contains(&Capability::SchemaSearch) {
            AgenticState::SqlRetrieval {
                discovered_tables: Vec::new(),
                max_table_relevancy: 0.0,
            }
        } else {
            AgenticState::Conversational
        }
    }

    /// Compute which capabilities are enabled based on settings and CLI filter.
    fn compute_enabled_capabilities(
        settings: &AppSettings,
        filter: &ToolLaunchFilter,
    ) -> HashSet<Capability> {
        let mut caps = HashSet::new();

        // RAG is always available if we support attachments
        // (gated by actual attachment presence at runtime)
        caps.insert(Capability::Rag);

        // Schema search
        if settings.schema_search_enabled && filter.builtin_allowed("schema_search") {
            caps.insert(Capability::SchemaSearch);
        }

        // SQL query
        if settings.sql_select_enabled && filter.builtin_allowed("sql_select") {
            caps.insert(Capability::SqlQuery);
        }

        // Python execution
        if settings.python_execution_enabled && filter.builtin_allowed("python_execution") {
            caps.insert(Capability::PythonExecution);
        }

        // Tool search
        if settings.tool_search_enabled && filter.builtin_allowed("tool_search") {
            caps.insert(Capability::ToolSearch);
        }

        // MCP tools (if any non-database MCP servers are enabled)
        // Note: We only check standard MCP servers here, NOT database sources.
        // Database sources are accessed via sql_select/schema_search, not directly as MCP tools.
        let has_enabled_mcp_servers = settings.mcp_servers
            .iter()
            .any(|s| s.enabled && filter.server_allowed(&s.id));

        if has_enabled_mcp_servers {
            caps.insert(Capability::McpTools);
        }

        caps
    }

    /// Compute the initial state based on context (RAG and schema search results).
    /// 
    /// This is called at the start of each user turn to determine the appropriate
    /// starting state based on relevancy scores.
    pub fn compute_initial_state(
        &mut self,
        rag_relevancy: f32,
        schema_relevancy: f32,
        discovered_tables: Vec<TableInfo>,
        _rag_chunks: Vec<RagChunk>,
    ) {
        let rag_passes = rag_relevancy >= self.thresholds.rag_chunk_min
            && self.enabled_capabilities.contains(&Capability::Rag);
        let schema_passes = schema_relevancy >= self.thresholds.schema_table_min
            && self.enabled_capabilities.contains(&Capability::SchemaSearch);
        let sql_enabled = schema_relevancy >= self.thresholds.sql_enable_min
            && self.enabled_capabilities.contains(&Capability::SqlQuery);
        let rag_dominant = rag_relevancy >= self.thresholds.rag_dominant_threshold;

        // Determine initial state based on relevancy
        let new_state = match (rag_passes, schema_passes, rag_dominant) {
            // RAG is dominant - suppress SQL context to focus the model
            (true, _, true) => AgenticState::RagRetrieval {
                max_chunk_relevancy: rag_relevancy,
                schema_relevancy: 0.0, // Suppressed
            },

            // Both relevant, neither dominant - hybrid mode
            (true, true, false) => {
                let mut active = HashSet::new();
                active.insert(Capability::Rag);
                if sql_enabled {
                    active.insert(Capability::SqlQuery);
                }
                active.insert(Capability::SchemaSearch);
                AgenticState::Hybrid {
                    active_capabilities: active,
                    rag_relevancy,
                    schema_relevancy,
                }
            }

            // Only RAG relevant
            (true, false, _) => AgenticState::RagRetrieval {
                max_chunk_relevancy: rag_relevancy,
                schema_relevancy,
            },

            // Only schema relevant
            (false, true, _) => AgenticState::SqlRetrieval {
                discovered_tables,
                max_table_relevancy: schema_relevancy,
            },

            // Neither passes threshold - check for other capabilities
            (false, false, _) => {
                if self.enabled_capabilities.contains(&Capability::PythonExecution) {
                    AgenticState::CodeExecution {
                        available_tools: Vec::new(),
                    }
                } else if self.enabled_capabilities.contains(&Capability::McpTools) {
                    AgenticState::ToolOrchestration {
                        materialized_tools: Vec::new(),
                    }
                } else {
                    AgenticState::Conversational
                }
            }
        };

        self.transition_to(new_state);
    }

    /// Set auto-discovery context for this turn.
    /// 
    /// This sets the results from automatic tool_search and schema_search
    /// that ran before the model's first response. The state machine will
    /// include this context in the system prompt it generates.
    pub fn set_auto_discovery_context(
        &mut self,
        tool_search: Option<crate::tools::tool_search::ToolSearchOutput>,
        schema_search: Option<crate::tools::schema_search::SchemaSearchOutput>,
    ) {
        self.auto_tool_search = tool_search;
        self.auto_schema_search = schema_search;
    }

    /// Transition to a new state, recording history.
    fn transition_to(&mut self, new_state: AgenticState) {
        // Record current state in history
        self.state_history.push(self.current_state.clone());
        self.current_state = new_state;

        println!(
            "[StateMachine] Transitioned to: {} (history depth: {})",
            self.current_state.name(),
            self.state_history.len()
        );
    }

    /// Handle a state event and potentially transition to a new state.
    pub fn handle_event(&mut self, event: StateEvent) {
        let new_state = match event {
            StateEvent::RagRetrieved {
                chunks,
                max_relevancy,
            } => AgenticState::RagContextInjected {
                chunks,
                max_relevancy,
                user_can_see_sources: true,
            },

            StateEvent::SchemaSearched {
                tables,
                max_relevancy,
            } => {
                let sql_enabled = max_relevancy >= self.thresholds.sql_enable_min
                    && self.enabled_capabilities.contains(&Capability::SqlQuery);
                AgenticState::SchemaContextInjected {
                    tables,
                    max_relevancy,
                    sql_enabled,
                }
            }

            StateEvent::SqlExecuted { results: _, row_count } => {
                AgenticState::SqlResultCommentary {
                    results_shown_to_user: true,
                    row_count,
                    query_context: format!("{} rows returned", row_count),
                }
            }

            StateEvent::PythonExecuted { stdout, stderr } => {
                if stderr.trim().is_empty() {
                    // No stderr - task may be complete
                    AgenticState::Conversational
                } else {
                    // Has stderr - handoff for continuation
                    AgenticState::CodeExecutionHandoff {
                        stdout_shown_to_user: stdout,
                        stderr_for_model: stderr,
                    }
                }
            }

            StateEvent::ToolSearchCompleted { discovered, schemas } => {
                AgenticState::ToolsDiscovered {
                    newly_materialized: discovered,
                    available_for_call: schemas,
                }
            }

            StateEvent::McpToolExecuted { .. } => {
                // After MCP tool execution, stay in orchestration or go conversational
                AgenticState::Conversational
            }

            StateEvent::ModelResponseFinal => {
                // Model produced final response - go to conversational
                AgenticState::Conversational
            }
        };

        self.transition_to(new_state);
    }

    /// Get the current state.
    pub fn current_state(&self) -> &AgenticState {
        &self.current_state
    }

    /// Get the enabled capabilities.
    pub fn enabled_capabilities(&self) -> &HashSet<Capability> {
        &self.enabled_capabilities
    }

    /// Get the thresholds.
    pub fn thresholds(&self) -> &RelevancyThresholds {
        &self.thresholds
    }

    /// Check if a specific tool is allowed in the current state.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        match &self.current_state {
            AgenticState::Conversational => false,

            AgenticState::RagRetrieval { .. } => false,

            AgenticState::SqlRetrieval { .. } => {
                tool_name == "sql_select" || tool_name == "schema_search"
            }

            AgenticState::ToolOrchestration { materialized_tools } => {
                tool_name == "tool_search" || materialized_tools.contains(&tool_name.to_string())
            }

            AgenticState::CodeExecution { available_tools } => {
                tool_name == "python_execution"
                    || tool_name == "tool_search"
                    || available_tools.contains(&tool_name.to_string())
            }

            AgenticState::Hybrid { active_capabilities, .. } => {
                if tool_name == "sql_select" {
                    active_capabilities.contains(&Capability::SqlQuery)
                } else if tool_name == "schema_search" {
                    active_capabilities.contains(&Capability::SchemaSearch)
                } else if tool_name == "python_execution" {
                    active_capabilities.contains(&Capability::PythonExecution)
                } else if tool_name == "tool_search" {
                    active_capabilities.contains(&Capability::ToolSearch)
                } else {
                    active_capabilities.contains(&Capability::McpTools)
                }
            }

            AgenticState::RagContextInjected { .. } => false,

            AgenticState::SchemaContextInjected { sql_enabled, .. } => {
                *sql_enabled && tool_name == "sql_select"
            }

            AgenticState::SqlResultCommentary { .. } => false,

            AgenticState::CodeExecutionHandoff { .. } => tool_name == "python_execution",

            AgenticState::ToolsDiscovered { available_for_call, .. } => {
                tool_name == "python_execution"
                    || available_for_call.iter().any(|s| s.name == tool_name)
            }
        }
    }

    /// Get the list of tool names allowed in the current state.
    pub fn allowed_tool_names(&self) -> Vec<String> {
        match &self.current_state {
            AgenticState::Conversational => vec![],

            AgenticState::RagRetrieval { .. } => vec![],

            AgenticState::SqlRetrieval { .. } => {
                vec!["sql_select".to_string(), "schema_search".to_string()]
            }

            AgenticState::ToolOrchestration { materialized_tools } => {
                let mut tools = vec!["tool_search".to_string()];
                tools.extend(materialized_tools.clone());
                tools
            }

            AgenticState::CodeExecution { available_tools } => {
                let mut tools = vec!["python_execution".to_string(), "tool_search".to_string()];
                tools.extend(available_tools.clone());
                tools
            }

            AgenticState::Hybrid { active_capabilities, .. } => {
                let mut tools = vec![];
                if active_capabilities.contains(&Capability::SqlQuery) {
                    tools.push("sql_select".to_string());
                }
                if active_capabilities.contains(&Capability::SchemaSearch) {
                    tools.push("schema_search".to_string());
                }
                if active_capabilities.contains(&Capability::PythonExecution) {
                    tools.push("python_execution".to_string());
                }
                if active_capabilities.contains(&Capability::ToolSearch) {
                    tools.push("tool_search".to_string());
                }
                tools
            }

            AgenticState::RagContextInjected { .. } => vec![],

            AgenticState::SchemaContextInjected { sql_enabled, .. } => {
                if *sql_enabled {
                    vec!["sql_select".to_string()]
                } else {
                    vec![]
                }
            }

            AgenticState::SqlResultCommentary { .. } => vec![],

            AgenticState::CodeExecutionHandoff { .. } => vec!["python_execution".to_string()],

            AgenticState::ToolsDiscovered { available_for_call, .. } => {
                let mut tools = vec!["python_execution".to_string()];
                tools.extend(available_for_call.iter().map(|s| s.name.clone()));
                tools
            }
        }
    }

    /// Check if the current state should trigger another iteration (loop continuation).
    pub fn should_continue_loop(&self) -> bool {
        matches!(
            &self.current_state,
            AgenticState::CodeExecutionHandoff { .. }
                | AgenticState::ToolsDiscovered { .. }
                | AgenticState::SqlResultCommentary { .. }
        )
    }

    /// Get the state history for debugging.
    pub fn state_history(&self) -> &[AgenticState] {
        &self.state_history
    }

    /// Reset the state machine for a new turn.
    pub fn reset(&mut self) {
        self.state_history.clear();
        self.current_state = AgenticState::Conversational;
    }

    /// Build the system prompt for the current state.
    /// 
    /// This is the **single source of truth** for system prompt generation.
    /// It ensures alignment between what we tell the model and what we allow:
    /// - Capabilities section reflects `enabled_capabilities`
    /// - Tool sections only show allowed tools
    /// - Format instructions match `tool_call_format`
    pub fn build_system_prompt(&self) -> String {
        let mut sections: Vec<String> = vec![self.base_prompt.clone()];

        // 1. Capabilities section (based on enabled_capabilities)
        if let Some(caps) = self.build_capabilities_section() {
            sections.push(caps);
        }

        // 2. Factual grounding (only if we have data retrieval tools)
        if self.has_data_retrieval_tools() {
            sections.push(self.build_factual_grounding_section());
        }

        // 3. State-specific context
        if let Some(state_ctx) = self.build_state_context_section() {
            sections.push(state_ctx);
        }

        // 4. Auto-discovery context (tool_search and schema_search results)
        if let Some(auto_ctx) = self.build_auto_discovery_section() {
            sections.push(auto_ctx);
        }

        // 5. Tool format instructions (if not in Python primary mode)
        if !self.python_primary {
            if let Some(format) = self.build_format_instructions() {
                sections.push(format);
            }
        }

        // 6. MCP tool sections (active and deferred)
        if let Some(mcp) = self.build_mcp_tool_section() {
            sections.push(mcp);
        }

        // 7. Python execution section (if enabled and in code mode)
        if self.python_primary && self.enabled_capabilities.contains(&Capability::PythonExecution) {
            sections.push(self.build_python_section());
        }

        sections.join("\n\n")
    }

    // ============ Prompt Section Builders ============

    /// Build the Capabilities section based on enabled_capabilities.
    fn build_capabilities_section(&self) -> Option<String> {
        let has_sql = self.enabled_capabilities.contains(&Capability::SqlQuery)
            || self.enabled_capabilities.contains(&Capability::SchemaSearch);
        let has_python = self.enabled_capabilities.contains(&Capability::PythonExecution);
        let has_mcp = self.enabled_capabilities.contains(&Capability::McpTools)
            || self.enabled_capabilities.contains(&Capability::ToolSearch);
        let has_rag = self.has_attachments;

        // If no tools are enabled, return None
        if !has_sql && !has_python && !has_mcp && !has_rag {
            return None;
        }

        let mut capability_list: Vec<&str> = Vec::new();

        if has_sql {
            capability_list.push("execute SQL queries against configured databases");
        }
        if has_python {
            capability_list.push("perform calculations in a Python sandbox");
        }
        if has_mcp {
            capability_list.push("use external tools via MCP servers");
        }
        if has_rag {
            capability_list.push("answer questions from attached documents");
        }

        if capability_list.is_empty() {
            return None;
        }

        let capabilities_str = match capability_list.len() {
            1 => capability_list[0].to_string(),
            2 => format!("{} and {}", capability_list[0], capability_list[1]),
            _ => {
                let last = capability_list.pop().unwrap();
                format!("{}, and {}", capability_list.join(", "), last)
            }
        };

        Some(format!(
            "## Capabilities\n\n\
            You are equipped with specialized tools to {}. \
            You MUST use these tools whenever the user's request requires factual data or tool execution. \
            Do NOT claim you cannot perform these tasks; use the tools listed below.",
            capabilities_str
        ))
    }

    /// Check if we have data retrieval tools enabled.
    fn has_data_retrieval_tools(&self) -> bool {
        self.enabled_capabilities.contains(&Capability::SqlQuery)
            || self.enabled_capabilities.contains(&Capability::McpTools)
            || self.enabled_capabilities.contains(&Capability::ToolSearch)
            || self.has_attachments
    }

    /// Build the Factual Grounding section based on enabled tools.
    fn build_factual_grounding_section(&self) -> String {
        let has_sql = self.enabled_capabilities.contains(&Capability::SqlQuery);
        let has_mcp = self.enabled_capabilities.contains(&Capability::McpTools)
            || self.enabled_capabilities.contains(&Capability::ToolSearch);

        // Build tool-specific examples
        let mut tool_examples: Vec<&str> = Vec::new();
        if has_sql {
            tool_examples.push("`sql_select`");
        }
        if has_mcp {
            tool_examples.push("MCP tools");
        }

        let examples_str = if tool_examples.is_empty() {
            "the appropriate tools".to_string()
        } else {
            tool_examples.join(" or ")
        };

        format!(
            "## Factual Grounding\n\n\
            **CRITICAL**: Never make up, infer, or guess data values. All factual information \
            (numbers, dates, totals, etc.) MUST come from executing tools like {} or \
            referencing the provided context. If you need data, use the appropriate tool first. \
            If you cannot get the data, say so explicitly rather than inventing results.",
            examples_str
        )
    }

    /// Build the state-specific context section.
    fn build_state_context_section(&self) -> Option<String> {
        match &self.current_state {
            AgenticState::Conversational => None,

            AgenticState::RagRetrieval { max_chunk_relevancy, .. } => Some(format!(
                "## Document Context\n\n\
                The user has attached documents to this conversation (relevancy score: {:.2}).\n\
                Answer the user's question using the context provided in their message.\n\
                The document content has already been extracted and is included above.\n\n\
                If the provided context doesn't fully answer the question, say so clearly.",
                max_chunk_relevancy
            )),

            AgenticState::SqlRetrieval { discovered_tables, max_table_relevancy } => {
                let table_list = self.format_table_list(discovered_tables);
                
                // Get the appropriate tool call format syntax
                let tool_call_syntax = match self.tool_call_format {
                    ToolCallFormatName::Native => "<tool_call>{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}</tool_call>".to_string(),
                    ToolCallFormatName::Hermes => "<tool_call>{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}</tool_call>".to_string(),
                    ToolCallFormatName::Mistral => "[TOOL_CALLS] [{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}]".to_string(),
                    ToolCallFormatName::Pythonic => "sql_select(sql=\"SELECT ...\")".to_string(),
                    ToolCallFormatName::PureJson => "{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}".to_string(),
                    ToolCallFormatName::CodeMode => "sql_select(sql=\"SELECT ...\")".to_string(),
                };
                
                let base_sql_instructions = format!(
                    "Use the `sql_select` tool to query these tables.\n\n\
                    **ACTION REQUIRED**: Execute queries using the tool-call format:\n\
                    ```\n\
                    {}\n\
                    ```\n\n\
                    **CRITICAL REQUIREMENTS**:\n\
                    - Execute queries to answer data questions - do NOT return SQL code\n\
                    - ONLY use columns explicitly listed in the schema above\n\
                    - NEVER guess column names - if not listed, it doesn't exist\n\
                    - Prefer aggregation (SUM, COUNT, AVG, etc.) of numeric columns to get final answers directly\n\
                    - Limit results to 25 rows or less through the design of the query\n\
                    - Avoid TO_CHAR function; use CAST(column AS STRING) instead\n\
                    - NEVER display SQL code to the user - only show query results\n\
                    - If a query fails, read the error and fix it rather than inventing results",
                    tool_call_syntax
                );
                
                let mut section = format!(
                    "## Database Context\n\n\
                    The following database tables are relevant to the user's question (max relevancy: {:.2}):\n\n\
                    {}\n\n\
                    ## SQL Execution\n\n\
                    {}",
                    max_table_relevancy,
                    table_list,
                    base_sql_instructions
                );

                // Add custom sql_select prompt if available
                if let Some(custom) = self.custom_tool_prompts.get("builtin::sql_select") {
                    let trimmed = custom.trim();
                    if !trimmed.is_empty() {
                        section.push_str("\n\n**Additional SQL Instructions**:\n");
                        section.push_str(trimmed);
                    }
                }

                Some(section)
            }

            AgenticState::ToolOrchestration { materialized_tools } => {
                let tools_str = if materialized_tools.is_empty() {
                    "No tools discovered yet. Use `tool_search` to find relevant tools.".to_string()
                } else {
                    format!("Available tools: {}", materialized_tools.join(", "))
                };
                Some(format!(
                    "## MCP Tool Orchestration\n\n\
                    {}\n\n\
                    Call `tool_search` to discover additional tools if needed.",
                    tools_str
                ))
            }

            AgenticState::CodeExecution { available_tools } => {
                Some(self.python_execution_prompt(available_tools))
            }

            AgenticState::Hybrid { active_capabilities, rag_relevancy, schema_relevancy } => {
                let mut parts = Vec::new();
                if active_capabilities.contains(&Capability::Rag) {
                    parts.push(format!(
                        "## Document Context (relevancy: {:.2})\n\n\
                        Document content has been provided in the conversation above.",
                        rag_relevancy
                    ));
                }
                if active_capabilities.contains(&Capability::SqlQuery) {
                    parts.push(format!(
                        "## Database Context (relevancy: {:.2})\n\n\
                        Database tables are available. Use `sql_select` to query them.",
                        schema_relevancy
                    ));
                }
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("\n\n"))
                }
            }

            AgenticState::RagContextInjected { chunks, max_relevancy, .. } => {
                let chunks_text = self.format_rag_chunks(chunks);
                Some(format!(
                    "## Retrieved Document Context\n\n\
                    The following excerpts are relevant to the user's question (max relevancy: {:.2}):\n\n\
                    {}\n\n\
                    Answer the user's question using this context. Cite sources when helpful.\n\
                    If the context doesn't fully answer the question, say so clearly.",
                    max_relevancy,
                    chunks_text
                ))
            }

            AgenticState::SchemaContextInjected { tables, max_relevancy, sql_enabled } => {
                let table_list = self.format_table_list(tables);
                let sql_instructions = if *sql_enabled {
                    // Get the appropriate tool call format syntax
                    let tool_call_syntax = match self.tool_call_format {
                        ToolCallFormatName::Native => "<tool_call>{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}</tool_call>".to_string(),
                        ToolCallFormatName::Hermes => "<tool_call>{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}</tool_call>".to_string(),
                        ToolCallFormatName::Mistral => "[TOOL_CALLS] [{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}]".to_string(),
                        ToolCallFormatName::Pythonic => "sql_select(sql=\"SELECT ...\")".to_string(),
                        ToolCallFormatName::PureJson => "{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}".to_string(),
                        ToolCallFormatName::CodeMode => "sql_select(sql=\"SELECT ...\")".to_string(),
                    };
                    
                    let mut instr = format!(
                        "Use the `sql_select` tool to query these tables.\n\n\
                        **ACTION REQUIRED**: Execute queries using the tool-call format:\n\
                        ```\n\
                        {}\n\
                        ```\n\n\
                        **CRITICAL REQUIREMENTS**:\n\
                        - Execute queries to answer data questions - do NOT return SQL code\n\
                        - ONLY use columns explicitly listed in the schema above\n\
                        - NEVER guess column names - if not listed, it doesn't exist\n\
                        - Prefer aggregation (SUM, COUNT, AVG, etc.) of numeric columns to get final answers directly\n\
                        - Limit results to 25 rows or less through the design of the query\n\
                        - Avoid TO_CHAR function; use CAST(column AS STRING) or FORMAT_DATE instead",
                        tool_call_syntax
                    );
                    if let Some(custom) = self.custom_tool_prompts.get("builtin::sql_select") {
                        let trimmed = custom.trim();
                        if !trimmed.is_empty() {
                            instr.push_str("\n\n**Additional SQL Instructions**:\n");
                            instr.push_str(trimmed);
                        }
                    }
                    instr
                } else {
                    "Note: SQL execution is not available for this query (relevancy below threshold).".to_string()
                };
                Some(format!(
                    "## Discovered Database Tables\n\n\
                    The following tables are relevant to the user's question (max relevancy: {:.2}):\n\n\
                    {}\n\n\
                    {}",
                    max_relevancy,
                    table_list,
                    sql_instructions
                ))
            }

            AgenticState::SqlResultCommentary { row_count, query_context, .. } => Some(format!(
                "## Query Results Commentary\n\n\
                The user has received the query results in table form ({} rows, context: {}).\n\n\
                Your role now is to:\n\
                1. Provide helpful commentary explaining what the data shows\n\
                2. Highlight any notable patterns, outliers, or insights\n\
                3. Suggest potential follow-up queries or next steps if relevant\n\
                4. Answer the user's original question based on the results\n\n\
                Do NOT re-display the data - the user already sees it. Focus on interpretation and guidance.",
                row_count,
                query_context
            )),

            AgenticState::CodeExecutionHandoff { stderr_for_model, .. } => Some(format!(
                "## Python Execution Handoff\n\n\
                Your previous Python program produced the following on stderr (handoff channel):\n\
                ---\n\
                {}\n\
                ---\n\n\
                The user has already seen the stdout output. Continue processing based on the stderr handoff.\n\
                If you need to run more code, output another ```python block.\n\
                If the task is complete, provide a final summary to the user.",
                stderr_for_model
            )),

            AgenticState::ToolsDiscovered { newly_materialized, available_for_call } => {
                let tools_str = newly_materialized.join(", ");
                let schema_summary = self.format_tool_schemas(available_for_call);
                Some(format!(
                    "## Tools Discovered\n\n\
                    The following tools are now available: {}\n\n\
                    {}\n\n\
                    Call these tools to complete the user's task.",
                    tools_str,
                    schema_summary
                ))
            }
        }
    }

    /// Build the auto-discovery context section.
    /// 
    /// This renders the results from automatic tool_search and schema_search
    /// that ran before the model's first response.
    fn build_auto_discovery_section(&self) -> Option<String> {
        let mut sections: Vec<String> = Vec::new();

        // Auto tool search results
        if let Some(ref output) = self.auto_tool_search {
            if !output.tools.is_empty() {
                let mut body = String::from("Auto-discovered MCP tools for this prompt:");
                for tool in &output.tools {
                    let desc = tool.description.as_deref().unwrap_or("").trim();
                    let mut line = format!(
                        "\n- {}::{} (score {:.2})",
                        tool.server_id, tool.name, tool.score
                    );
                    if !desc.is_empty() {
                        line.push_str(&format!(" — {}", desc));
                    }
                    body.push_str(&line);
                }
                sections.push(format!("### Auto tool search\n{}", body));
            }
        }

        // Auto schema search results (only if state doesn't already have schema context)
        if !self.current_state.has_schema_context() {
            if let Some(ref output) = self.auto_schema_search {
                if !output.tables.is_empty() {
                    // Apply rule: if we have RAG results (attachments), only include SQL context
                    // if the highest relevance score is > 40%.
                    let max_score = output.tables.iter().map(|t| t.relevance).fold(0.0f32, f32::max);
                    if self.has_attachments && max_score <= 0.40 {
                        println!(
                            "[StateMachine] Auto schema_search suppressed: RAG available and max SQL score ({:.2}) <= 0.40",
                            max_score
                        );
                    } else {
                        let mut body = String::from("Auto-discovered database tables for this prompt:");
                        for table in &output.tables {
                            let mut line = format!(
                                "\n- {} [{} Syntax | {}] (score {:.2})",
                                table.table_name, table.sql_dialect, table.source_id, table.relevance
                            );
                            if let Some(desc) = table.description.as_deref() {
                                if !desc.trim().is_empty() {
                                    line.push_str(&format!(" — {}", desc.trim()));
                                }
                            }
                            if !table.relevant_columns.is_empty() {
                                let cols: Vec<String> = table
                                    .relevant_columns
                                    .iter()
                                    .take(40) // Show up to 40 columns
                                    .map(|c| format!("{} ({})", c.name, c.data_type))
                                    .collect();
                                let cols_str = if cols.len() < table.relevant_columns.len() {
                                    format!("{}, ... ({} more)", cols.join(", "), table.relevant_columns.len() - cols.len())
                                } else {
                                    cols.join(", ")
                                };
                                line.push_str(&format!("\n  cols: {}", cols_str));
                            }
                            body.push_str(&line);
                        }
                        body.push_str("\n\n**ACTION REQUIRED**: These tables were auto-discovered because the user's question likely requires querying this database. You MUST:\n\
1. Write a SQL query using ONLY the table and columns listed above. Do NOT assume other columns exist.\n\
2. Execute the query using `sql_select` with the tool-call format:\n\
   ```\n\
   <tool_call>{\"name\": \"sql_select\", \"arguments\": {\"sql\": \"SELECT ...\"}}</tool_call>\n\
   ```\n\
   The `source_id` is optional when only one database is enabled.\n\
3. **AGGREGATION PREFERRED**: Use SQL aggregation (SUM, COUNT, AVG, etc.) of numeric columns to get final answers directly.\n\
4. **ROW LIMIT**: Limit results to 25 rows or less through the design of the query.\n\
5. **AVOID TO_CHAR**: Use CAST(column AS STRING) instead of TO_CHAR.\n\
6. **COLUMNS & DATES**: Check listed columns for date/time fields. If none listed, use `schema_search` first.\n\n\
**CRITICAL - NO HALLUCINATIONS**:\n\
- ONLY use columns explicitly listed in the `cols:` section above.\n\
- NEVER guess or invent column names.\n\
- If a query returns `null`, the data is missing - do NOT assume the tool failed.\n\
- NEVER make up data values. All facts MUST come from executing `sql_select`.\n\
- NEVER display SQL code to the user - only show query results.\n\
- If the query fails, read the error and fix it rather than inventing results.");
                        sections.push(format!("### Auto schema search\n{}", body));
                    }
                } else if output.summary.contains("WARNING") {
                    sections.push(format!("### Auto schema search\n{}", output.summary));
                }
            }
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("## Auto-discovered context\n\n{}", sections.join("\n\n")))
        }
    }

    /// Build tool format instructions based on tool_call_format.
    fn build_format_instructions(&self) -> Option<String> {
        // Don't add format instructions if no tools are available
        if !self.enabled_capabilities.contains(&Capability::SqlQuery)
            && !self.enabled_capabilities.contains(&Capability::McpTools)
            && !self.enabled_capabilities.contains(&Capability::SchemaSearch)
            && !self.enabled_capabilities.contains(&Capability::ToolSearch)
        {
            return None;
        }

        match self.tool_call_format {
            ToolCallFormatName::Native => None, // Native tools don't need instructions
            ToolCallFormatName::Hermes => Some(
                "## Tool Calling Format\n\n\
                When you need to use a tool, output ONLY:\n\
                <tool_call>{\"name\": \"tool_name\", \"arguments\": {...}}</tool_call>".to_string()
            ),
            ToolCallFormatName::Mistral => Some(
                "## Tool Calling Format\n\n\
                When you need to use a tool, output:\n\
                [TOOL_CALLS] [{\"name\": \"tool_name\", \"arguments\": {...}}]".to_string()
            ),
            ToolCallFormatName::Pythonic => Some(
                "## Tool Calling Format\n\n\
                When you need to use a tool, output:\n\
                tool_name(arg1=\"value\", arg2=123)".to_string()
            ),
            ToolCallFormatName::PureJson => Some(
                "## Tool Calling Format\n\n\
                When you need to use a tool, output a JSON object:\n\
                {\"name\": \"tool_name\", \"arguments\": {...}}".to_string()
            ),
            ToolCallFormatName::CodeMode => None, // Code mode has its own section
        }
    }

    /// Build MCP tool section from mcp_context.
    fn build_mcp_tool_section(&self) -> Option<String> {
        if !self.mcp_context.has_any_tools() {
            return None;
        }

        let mut parts = Vec::new();

        // Active tools (can be called immediately)
        if self.mcp_context.has_active_tools() {
            parts.push("## Active MCP Tools (Ready to Use)\n\nThese tools can be called immediately:".to_string());
            
            for (server_id, tools) in &self.mcp_context.active_tools {
                if tools.is_empty() {
                    continue;
                }
                
                parts.push(format!("\n### Server: `{}`\n", server_id));
                
                for tool in tools {
                    let mut tool_desc = format!("**{}**", tool.name);
                    if let Some(desc) = &tool.description {
                        tool_desc.push_str(&format!(": {}", desc));
                    }
                    parts.push(tool_desc);

                    // Add custom tool prompt if available
                    let prompt_key = format!("{}::{}", server_id, tool.name);
                    if let Some(custom_prompt) = self.custom_tool_prompts.get(&prompt_key) {
                        let trimmed = custom_prompt.trim();
                        if !trimmed.is_empty() {
                            parts.push(format!("  *Instruction*: {}", trimmed));
                        }
                    }
                    
                    // Add parameter info if available
                    if let Some(schema) = &tool.parameters_schema {
                        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                            let required: Vec<&str> = schema
                                .get("required")
                                .and_then(|r| r.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                                .unwrap_or_default();
                            
                            parts.push("  Arguments:".to_string());
                            for (name, prop) in props {
                                let prop_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                                let is_required = required.contains(&name.as_str());
                                let req_marker = if is_required { " [REQUIRED]" } else { "" };
                                parts.push(format!("  - `{}` ({}){}", name, prop_type, req_marker));
                            }
                        }
                    }
                }
            }
        }

        // Deferred tools (require discovery)
        if self.mcp_context.has_deferred_tools() {
            let count = self.mcp_context.deferred_tool_count();
            let server_count = self.mcp_context.deferred_tools.len();
            parts.push(format!(
                "\n## Deferred MCP Tools\n\n\
                There are {} tools available across {} server(s). \
                Use `tool_search` to discover relevant tools before using them.",
                count, server_count
            ));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    /// Build Python execution section for code mode.
    fn build_python_section(&self) -> String {
        let has_tool_search = self.enabled_capabilities.contains(&Capability::ToolSearch);
        let has_deferred = self.mcp_context.has_deferred_tools();

        let mut parts = vec![
            "## Python Execution (Code Mode)\n\n\
            You must return exactly one runnable Python program. Do not return explanations or multiple blocks.\n\n\
            Output format: a single ```python ... ``` block. We will execute it and surface any print output directly to the user.".to_string()
        ];

        parts.push(
            "**stdout/stderr Semantics**:\n\
            - Use `print(...)` for user-facing output (shown to user)\n\
            - Use `sys.stderr.write(...)` for handoff text (triggers continuation)".to_string()
        );

        parts.push(
            "**Allowed imports**: math, json, random, re, datetime, collections, itertools, functools, \
            operator, string, textwrap, copy, types, typing, abc, numbers, decimal, fractions, \
            statistics, hashlib, base64, binascii, html.".to_string()
        );

        if has_tool_search && has_deferred {
            parts.push(
                "**Tool Discovery**: Use `tool_search(relevant_to=\"...\")` to discover MCP tools before calling them. \
                Tools are NOT available until discovered.".to_string()
            );
        }

        // Add custom python_execution prompt if available
        if let Some(custom) = self.custom_tool_prompts.get("builtin::python_execution") {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                parts.push(format!("**Additional Instructions**:\n{}", trimmed));
            }
        }

        parts.join("\n\n")
    }

    /// Generate the factual grounding section (anti-hallucination) - legacy version.
    #[allow(dead_code)]
    fn factual_grounding_section(&self) -> String {
        "## Factual Grounding\n\n\
        **CRITICAL**: Never make up, infer, or guess data values. All factual information \
        (numbers, dates, totals, sales figures, etc.) MUST come from executing tools or \
        referencing the provided context. If you need data, use the appropriate tool first. \
        If you cannot get the data, say so explicitly rather than inventing results.".to_string()
    }

    /// Generate the Python execution prompt section.
    fn python_execution_prompt(&self, available_tools: &[String]) -> String {
        let tools_section = if available_tools.is_empty() {
            "Use `tool_search` to discover available tools if needed.".to_string()
        } else {
            format!("Available tools: {}", available_tools.join(", "))
        };

        let mut prompt = format!(
            "## Python Execution\n\n\
            You must return exactly one runnable Python program. Do not return explanations or multiple blocks.\n\n\
            Output format: a single ```python ... ``` block. We will execute it and surface any print output directly to the user.\n\n\
            **stdout/stderr Semantics**:\n\
            - Use `print(...)` for user-facing output (shown to user)\n\
            - Use `sys.stderr.write(...)` for handoff text (triggers continuation)\n\n\
            **Allowed imports**: math, json, random, re, datetime, collections, itertools, functools, \
            operator, string, textwrap, copy, types, typing, abc, numbers, decimal, fractions, \
            statistics, hashlib, base64, binascii, html.\n\n\
            {}\n\n\
            Keep code concise and runnable; include prints for results the user should see.",
            tools_section
        );

        // Add custom python_execution prompt if available
        if let Some(custom) = self.custom_tool_prompts.get("builtin::python_execution") {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                prompt.push_str("\n\n**Additional Python Instructions**:\n");
                prompt.push_str(trimmed);
            }
        }

        prompt
    }

    /// Format a list of tables for the prompt.
    fn format_table_list(&self, tables: &[TableInfo]) -> String {
        if tables.is_empty() {
            return "No tables discovered.".to_string();
        }

        tables
            .iter()
            .map(|table| {
                let cols: Vec<String> = table
                    .columns
                    .iter()
                    .take(40) // Show up to 40 columns to give model enough context
                    .map(|c| format!("{} ({})", c.name, c.data_type))
                    .collect();
                let cols_str = if cols.len() < table.columns.len() {
                    format!("{}, ... ({} more)", cols.join(", "), table.columns.len() - cols.len())
                } else {
                    cols.join(", ")
                };
                format!(
                    "- **{}** [{} Syntax] (relevancy: {:.2})\n  Columns: {}",
                    table.fully_qualified_name,
                    table.sql_dialect,
                    table.relevancy,
                    cols_str
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format RAG chunks for the prompt.
    fn format_rag_chunks(&self, chunks: &[RagChunk]) -> String {
        if chunks.is_empty() {
            return "No document chunks available.".to_string();
        }

        chunks
            .iter()
            .map(|chunk| {
                let preview: String = chunk.content.chars().take(500).collect();
                let truncated = if chunk.content.len() > 500 { "..." } else { "" };
                format!(
                    "### {} (relevancy: {:.2})\n\n{}{}",
                    chunk.source_file, chunk.relevancy, preview, truncated
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Format tool schemas for the prompt.
    fn format_tool_schemas(&self, schemas: &[ToolSchema]) -> String {
        if schemas.is_empty() {
            return "".to_string();
        }

        schemas
            .iter()
            .map(|schema| {
                let desc = schema.description.as_deref().unwrap_or("No description");
                format!("- **{}**: {}", schema.name, desc)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ============ State Preview for Settings UI ============

/// A preview of a possible state for the settings UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StatePreview {
    /// State name
    pub name: String,
    /// State description
    pub description: String,
    /// Tools available in this state
    pub available_tools: Vec<String>,
    /// System prompt additions for this state
    pub prompt_additions: Vec<String>,
    /// Whether this state is currently possible given enabled capabilities
    pub is_possible: bool,
}

impl AgenticStateMachine {
    /// Get previews of all possible states for the settings UI.
    pub fn get_possible_states(&self) -> Vec<StatePreview> {
        let mut previews = Vec::new();

        // Conversational
        previews.push(StatePreview {
            name: "Conversational".to_string(),
            description: "Base conversational state - no tools available".to_string(),
            available_tools: vec![],
            prompt_additions: vec!["Base prompt only".to_string()],
            is_possible: true, // Always possible
        });

        // RAG Retrieval
        if self.enabled_capabilities.contains(&Capability::Rag) {
            previews.push(StatePreview {
                name: "RAG Retrieval".to_string(),
                description: "Document Q&A with attached files".to_string(),
                available_tools: vec![],
                prompt_additions: vec![
                    "Answer from attached documents; content already extracted".to_string(),
                ],
                is_possible: true,
            });
        }

        // SQL Retrieval
        if self.enabled_capabilities.contains(&Capability::SqlQuery) {
            previews.push(StatePreview {
                name: "SQL Retrieval".to_string(),
                description: "Database queries with discovered schemas".to_string(),
                available_tools: vec!["sql_select".to_string(), "schema_search".to_string()],
                prompt_additions: vec![
                    "Schema context, SQL format".to_string(),
                    "Execute and return results".to_string(),
                ],
                is_possible: true,
            });
        }

        // Tool Orchestration
        if self.enabled_capabilities.contains(&Capability::McpTools) {
            previews.push(StatePreview {
                name: "Tool Orchestration".to_string(),
                description: "MCP tool usage with discovery".to_string(),
                available_tools: vec!["tool_search".to_string()],
                prompt_additions: vec!["Tool descriptions, format instructions".to_string()],
                is_possible: true,
            });
        }

        // Code Execution
        if self.enabled_capabilities.contains(&Capability::PythonExecution) {
            previews.push(StatePreview {
                name: "Code Execution".to_string(),
                description: "Python sandbox with tool calling".to_string(),
                available_tools: vec!["python_execution".to_string(), "tool_search".to_string()],
                prompt_additions: vec![
                    "Python sandbox rules".to_string(),
                    "Allowed imports".to_string(),
                    "stdout/stderr semantics".to_string(),
                ],
                is_possible: true,
            });
        }

        // SQL Result Commentary (mid-turn)
        if self.enabled_capabilities.contains(&Capability::SqlQuery) {
            previews.push(StatePreview {
                name: "SQL Result Commentary".to_string(),
                description: "After SQL execution - provide interpretation".to_string(),
                available_tools: vec![],
                prompt_additions: vec![
                    "User sees the table".to_string(),
                    "Provide interpretation, insights, and next steps".to_string(),
                    "Do NOT re-display data".to_string(),
                ],
                is_possible: true,
            });
        }

        // Code Execution Handoff (mid-turn)
        if self.enabled_capabilities.contains(&Capability::PythonExecution) {
            previews.push(StatePreview {
                name: "Code Execution Handoff".to_string(),
                description: "After Python with stderr - continue processing".to_string(),
                available_tools: vec!["python_execution".to_string()],
                prompt_additions: vec![
                    "Stderr handoff received".to_string(),
                    "User saw stdout".to_string(),
                    "Continue processing or summarize".to_string(),
                ],
                is_possible: true,
            });
        }

        previews
    }
}

// ============ Tests ============

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic_state::SqlResults;
    use crate::settings::AppSettings;
    use crate::tool_capability::ToolLaunchFilter;

    fn test_settings() -> AppSettings {
        let mut settings = AppSettings::default();
        settings.python_execution_enabled = true;
        settings.tool_search_enabled = true;
        settings.schema_search_enabled = true;
        settings.sql_select_enabled = true;
        settings
    }

    #[test]
    fn test_state_machine_creation() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let machine = AgenticStateMachine::new(&settings, &filter, thresholds, "Test prompt".to_string());

        assert!(machine
            .enabled_capabilities()
            .contains(&Capability::PythonExecution));
        assert!(machine
            .enabled_capabilities()
            .contains(&Capability::SqlQuery));
        assert!(machine
            .enabled_capabilities()
            .contains(&Capability::SchemaSearch));
    }

    #[test]
    fn test_initial_state_rag_dominant() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let mut machine =
            AgenticStateMachine::new(&settings, &filter, thresholds, "Test".to_string());

        // RAG relevancy above dominant threshold
        machine.compute_initial_state(0.7, 0.3, vec![], vec![]);

        match machine.current_state() {
            AgenticState::RagRetrieval {
                max_chunk_relevancy,
                schema_relevancy,
            } => {
                assert!(*max_chunk_relevancy > 0.6);
                assert!(*schema_relevancy < 0.01); // Suppressed
            }
            _ => panic!("Expected RagRetrieval state"),
        }
    }

    #[test]
    fn test_initial_state_sql_only() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let mut machine =
            AgenticStateMachine::new(&settings, &filter, thresholds, "Test".to_string());

        // Only schema relevancy passes
        machine.compute_initial_state(0.1, 0.5, vec![], vec![]);

        match machine.current_state() {
            AgenticState::SqlRetrieval { max_table_relevancy, .. } => {
                assert!(*max_table_relevancy > 0.4);
            }
            _ => panic!("Expected SqlRetrieval state"),
        }
    }

    #[test]
    fn test_tool_allowed_in_sql_state() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let mut machine =
            AgenticStateMachine::new(&settings, &filter, thresholds, "Test".to_string());

        machine.compute_initial_state(0.1, 0.5, vec![], vec![]);

        assert!(machine.is_tool_allowed("sql_select"));
        assert!(machine.is_tool_allowed("schema_search"));
        assert!(!machine.is_tool_allowed("python_execution"));
    }

    #[test]
    fn test_sql_result_commentary_transition() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let mut machine =
            AgenticStateMachine::new(&settings, &filter, thresholds, "Test".to_string());

        machine.compute_initial_state(0.1, 0.5, vec![], vec![]);

        // Execute SQL
        machine.handle_event(StateEvent::SqlExecuted {
            results: SqlResults {
                columns: vec!["col1".to_string()],
                rows: vec![vec!["val1".to_string()]],
                row_count: 1,
                truncated: false,
            },
            row_count: 1,
        });

        match machine.current_state() {
            AgenticState::SqlResultCommentary {
                results_shown_to_user,
                row_count,
                ..
            } => {
                assert!(*results_shown_to_user);
                assert_eq!(*row_count, 1);
            }
            _ => panic!("Expected SqlResultCommentary state"),
        }

        // No tools should be allowed in commentary state
        assert!(!machine.is_tool_allowed("sql_select"));
        assert!(machine.should_continue_loop());
    }

    #[test]
    fn test_python_stderr_handoff() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let mut machine =
            AgenticStateMachine::new(&settings, &filter, thresholds, "Test".to_string());

        // Start in code execution mode
        machine.compute_initial_state(0.0, 0.0, vec![], vec![]);

        // Execute Python with stderr
        machine.handle_event(StateEvent::PythonExecuted {
            stdout: "User output".to_string(),
            stderr: "Handoff content".to_string(),
        });

        match machine.current_state() {
            AgenticState::CodeExecutionHandoff {
                stdout_shown_to_user,
                stderr_for_model,
            } => {
                assert_eq!(stdout_shown_to_user, "User output");
                assert_eq!(stderr_for_model, "Handoff content");
            }
            _ => panic!("Expected CodeExecutionHandoff state"),
        }

        assert!(machine.is_tool_allowed("python_execution"));
        assert!(machine.should_continue_loop());
    }

    #[test]
    fn test_possible_states_preview() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let machine = AgenticStateMachine::new(&settings, &filter, thresholds, "Test".to_string());

        let previews = machine.get_possible_states();

        // Should have multiple states
        assert!(previews.len() >= 4);

        // Check that Conversational is always present
        assert!(previews.iter().any(|p| p.name == "Conversational"));

        // Check that SQL Retrieval is present (since sql_select is enabled)
        assert!(previews.iter().any(|p| p.name == "SQL Retrieval"));
    }
}

