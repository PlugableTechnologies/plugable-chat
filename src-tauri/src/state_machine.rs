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
use crate::protocol::{ToolSchema, ToolFormat};
use crate::settings::{AppSettings, ToolCallFormatName};
use crate::settings_state_machine::{OperationalMode, SettingsStateMachine, ChatTurnContext, TurnConfiguration};
use crate::system_prompt;

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
    /// The SettingsStateMachine providing capabilities and thresholds
    settings_sm: SettingsStateMachine,
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
    /// Model-specific tool format preference
    model_tool_format: Option<ToolFormat>,
    /// Custom prompts per tool (key: "server_id::tool_name")
    custom_tool_prompts: HashMap<String, String>,
    /// Whether Python is the primary tool calling mode (Code Mode)
    python_primary: bool,
    /// Whether user has attached documents
    has_attachments: bool,
    /// Per-chat attached database tables
    attached_tables: Vec<crate::settings_state_machine::AttachedTableInfo>,
    /// Per-chat attached tools
    attached_tools: Vec<String>,
    
    /// Turn-specific configuration computed from attachments
    turn_config: Option<TurnConfiguration>,
    
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
        let mut enabled_capabilities = settings_sm.enabled_capabilities().clone();
        let thresholds = RelevancyThresholds {
            rag_chunk_min: settings_sm.relevancy_thresholds().rag_chunk_min,
            schema_relevancy: settings_sm.relevancy_thresholds().schema_relevancy,
            rag_dominant_threshold: settings_sm.relevancy_thresholds().rag_dominant_threshold,
        };
        
        // When RAG documents are attached, disable SQL tools to avoid confusing the model.
        // We don't support simultaneous SQL and RAG - user-attached documents take priority.
        if prompt_context.has_attachments {
            enabled_capabilities.remove(&Capability::SqlQuery);
            enabled_capabilities.remove(&Capability::SchemaSearch);
            println!("[StateMachine] RAG attachments present - SQL tools disabled to focus on attached documents");
        }
        
        // Determine initial state based on operational mode
        let initial_state = Self::compute_initial_state_from_mode(
            settings_sm.operational_mode(),
            &enabled_capabilities,
        );
        
        Self {
            settings_sm: settings_sm.clone(),
            current_state: initial_state,
            enabled_capabilities,
            thresholds,
            state_history: Vec::new(),
            base_prompt: prompt_context.base_prompt,
            mcp_context: prompt_context.mcp_context,
            tool_call_format: prompt_context.tool_call_format,
            model_tool_format: prompt_context.model_tool_format,
            custom_tool_prompts: prompt_context.custom_tool_prompts,
            python_primary: prompt_context.python_primary,
            has_attachments: prompt_context.has_attachments,
            attached_tables: prompt_context.attached_tables,
            attached_tools: prompt_context.attached_tools,
            turn_config: None,
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


    /// Compute the initial state based on context (RAG and schema search results).
    /// 
    /// This is called at the start of each user turn to determine the appropriate
    /// starting state based on relevancy scores.
    pub fn compute_initial_state(
        &mut self,
        rag_relevancy: f32,
        schema_relevancy: f32,
        mut discovered_tables: Vec<TableInfo>,
        _rag_chunks: Vec<RagChunk>,
    ) {
        // If the user has explicitly attached tables, we prioritize them and 
        // suppress auto-discovered tables to avoid prompt noise and duplication.
        if !self.attached_tables.is_empty() {
            discovered_tables.clear();
        }
        let rag_passes = rag_relevancy >= self.thresholds.rag_chunk_min
            && self.enabled_capabilities.contains(&Capability::Rag);
            
        // schema_passes is true if:
        // 1. Relevancy score passes threshold OR
        // 2. User has explicitly attached tables for this chat
        let schema_passes = (schema_relevancy >= self.thresholds.schema_relevancy
            || !self.attached_tables.is_empty())
            && (self.enabled_capabilities.contains(&Capability::SchemaSearch)
                || self.enabled_capabilities.contains(&Capability::SqlQuery));
                
        let sql_enabled = (schema_relevancy >= self.thresholds.schema_relevancy 
            || !self.attached_tables.is_empty())
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
                let sql_enabled = max_relevancy >= self.thresholds.schema_relevancy
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

            // Allow sql_select in commentary state for multi-query scenarios
            // (e.g., "compare nov 2025 and oct 2025 sales" requires 2 queries)
            AgenticState::SqlResultCommentary { .. } => {
                tool_name == "sql_select" || tool_name == "schema_search"
            }

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

            // Allow sql_select in commentary state for multi-query scenarios
            AgenticState::SqlResultCommentary { .. } => {
                vec!["sql_select".to_string(), "schema_search".to_string()]
            }

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

    pub fn compute_turn_config(&mut self, settings: &AppSettings, filter: &crate::tool_capability::ToolLaunchFilter) {
        let turn_context = ChatTurnContext {
            attached_files: Vec::new(), // Not used for mode computation in SM yet
            attached_tables: self.attached_tables.clone(),
            attached_tools: self.attached_tools.clone(),
        };
        
        let config = self.settings_sm.compute_for_turn(settings, filter, &turn_context);
        
        // Update enabled_capabilities based on per-turn attached tools.
        // This ensures that compute_initial_state() will see these capabilities
        // even if the corresponding setting is disabled globally.
        for tool_name in &config.enabled_tools {
            if tool_name == "python_execution" {
                self.enabled_capabilities.insert(Capability::PythonExecution);
            } else if tool_name == "sql_select" {
                self.enabled_capabilities.insert(Capability::SqlQuery);
            } else if tool_name == "schema_search" {
                self.enabled_capabilities.insert(Capability::SchemaSearch);
            } else if tool_name == "tool_search" {
                self.enabled_capabilities.insert(Capability::ToolSearch);
            } else if tool_name.contains("::") && !tool_name.starts_with("builtin::") {
                // MCP tool (format: "server_id::tool_name")
                self.enabled_capabilities.insert(Capability::McpTools);
            }
        }
        
        // If the turn config establishes a specific mode, update the current state
        // to match. This handles the case where user attaches tools that override
        // the default state derived from settings.
        match &config.mode {
            OperationalMode::CodeMode { .. } => {
                // Ensure we're in CodeExecution state for proper prompt generation
                if !matches!(self.current_state, AgenticState::CodeExecution { .. }) {
                    self.transition_to(AgenticState::CodeExecution {
                        available_tools: Vec::new(),
                    });
                }
                // CRITICAL: Set python_primary=true so the tool_call format section
                // is NOT added to the prompt (it conflicts with Python mode).
                self.python_primary = true;
            }
            OperationalMode::SqlMode { .. } => {
                if !matches!(self.current_state, AgenticState::SqlRetrieval { .. }) {
                    self.transition_to(AgenticState::SqlRetrieval {
                        discovered_tables: Vec::new(),
                        max_table_relevancy: 0.0,
                    });
                }
            }
            OperationalMode::ToolMode { .. } => {
                if !matches!(self.current_state, AgenticState::ToolOrchestration { .. }) {
                    self.transition_to(AgenticState::ToolOrchestration {
                        materialized_tools: Vec::new(),
                    });
                }
            }
            OperationalMode::HybridMode { enabled_modes, .. } => {
                use crate::settings_state_machine::SimplifiedMode;
                // For hybrid mode, prefer CodeExecution if Code is enabled
                if enabled_modes.contains(&SimplifiedMode::Code) {
                    if !matches!(self.current_state, AgenticState::CodeExecution { .. }) {
                        self.transition_to(AgenticState::CodeExecution {
                            available_tools: Vec::new(),
                        });
                    }
                }
            }
            OperationalMode::Conversational => {
                // No transition needed - keep current state or let compute_initial_state handle it
            }
        }
        
        self.turn_config = Some(config);
    }

    /// Build the system prompt for the current state.
    /// 
    /// This is the **single source of truth** for system prompt generation.
    /// It ensures alignment between what we tell the model and what we allow:
    /// - Capabilities section reflects `enabled_capabilities`
    /// - Tool sections only show allowed tools
    /// - Format instructions match `tool_call_format`
    pub fn build_system_prompt(&self) -> String {
        self.build_system_prompt_sections().join("\n\n")
    }

    /// Build the system prompt as a list of sections.
    pub fn build_system_prompt_sections(&self) -> Vec<String> {
        let mut sections: Vec<String> = vec![self.base_prompt.clone()];
        let active_capabilities = self.current_state.active_capabilities();

        // 1. Capabilities section (based on active capabilities)
        if let Some(caps) = self.build_capabilities_section(&active_capabilities) {
            sections.push(caps);
        }

        // 2. Factual grounding (only if we have active data retrieval tools)
        if self.has_active_data_retrieval_tools() {
            sections.push(self.build_factual_grounding_section());
        }

        // 3. State-specific context
        if let Some(state_ctx) = self.build_state_context_section() {
            sections.push(state_ctx);
        }

        // 3b. Turn-specific schema context (from attached tables)
        if let Some(ref config) = self.turn_config {
            if let Some(ref schema_ctx) = config.schema_context {
                let mut ctx = schema_ctx.to_string();
                
                // If the current state doesn't ALREADY provide SQL execution guidance,
                // add it here so attached tables are usable.
                if !self.current_state.has_schema_context() {
                    let first_table = self.attached_tables.first().map(|t| t.table_fq_name.as_str());
                    let guidance = system_prompt::build_sql_instructions(
                        self.tool_call_format,
                        self.model_tool_format,
                        first_table,
                    );
                    ctx.push_str("\n\n## SQL Execution Guidance\n\n");
                    ctx.push_str(&guidance);
                }
                
                sections.push(ctx);
            }
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
        if self.python_primary && active_capabilities.contains(&Capability::PythonExecution) {
            sections.push(self.build_python_section());
        }

        sections
    }

    // ============ Prompt Section Builders ============

    /// Build the Capabilities section based on active capabilities.
    fn build_capabilities_section(&self, active_capabilities: &HashSet<Capability>) -> Option<String> {
        system_prompt::build_capabilities_section(active_capabilities, self.has_attachments)
    }

    /// Check if we have active data retrieval tools.
    fn has_active_data_retrieval_tools(&self) -> bool {
        let active = self.current_state.active_capabilities();
        active.contains(&Capability::SqlQuery)
            || active.contains(&Capability::McpTools)
            || active.contains(&Capability::ToolSearch)
            || self.has_attachments
    }

    /// Build the Factual Grounding section based on enabled tools.
    fn build_factual_grounding_section(&self) -> String {
        let active = self.current_state.active_capabilities();
        system_prompt::build_factual_grounding(&active, self.has_attachments)
    }

    /// Build the state-specific context section.
    fn build_state_context_section(&self) -> Option<String> {
        match &self.current_state {
            AgenticState::Conversational => None,

            AgenticState::RagRetrieval { max_chunk_relevancy, .. } => {
                Some(system_prompt::build_document_context_summary(*max_chunk_relevancy))
            }

            AgenticState::SqlRetrieval { discovered_tables, max_table_relevancy } => {
                // Filter out any tables that are already in the attached_tables list
                // to avoid duplication in the system prompt.
                let attached_names: HashSet<_> = self.attached_tables.iter().map(|t| &t.table_fq_name).collect();
                let filtered_tables: Vec<_> = discovered_tables
                    .iter()
                    .filter(|t| !attached_names.contains(&t.fully_qualified_name))
                    .cloned()
                    .collect();

                if filtered_tables.is_empty() {
                    // If all discovered tables were already attached, only provide guidance
                    // if it's not being provided by Step 3b.
                    // Actually, if we're in SqlRetrieval state, Step 3b might not have run yet
                    // or it might run after this. 
                    // To be safe, if we have attached tables, we'll let Step 3b handle the guidance.
                    if self.attached_tables.is_empty() {
                        Some("No database tables discovered.".to_string())
                    } else {
                        None
                    }
                } else {
                    let table_list = self.format_table_list(&filtered_tables);
                    let first_table = filtered_tables.first().map(|t| t.fully_qualified_name.as_str());
                    let base_sql_instructions = system_prompt::build_sql_instructions(
                        self.tool_call_format,
                        self.model_tool_format,
                        first_table,
                    );
                    Some(system_prompt::build_retrieved_sql_context(*max_table_relevancy, &table_list, &base_sql_instructions))
                }
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
                    parts.push(system_prompt::build_document_context_summary(*rag_relevancy));
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
                Some(system_prompt::build_retrieved_document_context(*max_relevancy, &chunks_text))
            }

            AgenticState::SchemaContextInjected { tables, max_relevancy, sql_enabled } => {
                // Filter out any tables that are already in the attached_tables list
                let attached_names: HashSet<_> = self.attached_tables.iter().map(|t| &t.table_fq_name).collect();
                let filtered_tables: Vec<_> = tables
                    .iter()
                    .filter(|t| !attached_names.contains(&t.fully_qualified_name))
                    .cloned()
                    .collect();

                if filtered_tables.is_empty() && !self.attached_tables.is_empty() {
                    return None;
                }

                let table_list = self.format_table_list(&filtered_tables);
                let sql_instructions = if *sql_enabled {
                    let first_table = filtered_tables.first().map(|t| t.fully_qualified_name.as_str());
                    let mut instr = system_prompt::build_sql_instructions(
                        self.tool_call_format,
                        self.model_tool_format,
                        first_table,
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
                Some(system_prompt::build_retrieved_sql_context(*max_relevancy, &table_list, &sql_instructions))
            }

            AgenticState::SqlResultCommentary { query_context, .. } => Some(format!(
                "## SQL Result Analysis\n\n\
                The previous tool execution returned: {}. \
                If additional queries are needed to fully answer the user's question (e.g., for comparisons), \
                you may execute more `sql_select` calls. Otherwise, summarize these results for the user.",
                query_context
            )),

            AgenticState::CodeExecutionHandoff { stderr_for_model, .. } => Some(format!(
                "## Python Handoff Context\n\n\
                The previous execution returned data on stderr for your consideration:\n\n\
                ```\n\
                {}\n\
                ```\n\n\
                Use this information to continue the task or provide a final answer.",
                stderr_for_model
            )),

            AgenticState::ToolsDiscovered { newly_materialized, available_for_call } => {
                let newly_str = if newly_materialized.is_empty() {
                    "No new tools were materialized.".to_string()
                } else {
                    format!("Newly materialized tools: {}", newly_materialized.join(", "))
                };
                let schemas_text = self.format_tool_schemas(available_for_call);
                Some(format!(
                    "## New Tools Discovered\n\n\
                    {}\n\n\
                    You can now use these tools in your next Python execution:\n\n\
                    {}",
                    newly_str,
                    schemas_text
                ))
            }
        }
    }

    /// Build auto-discovery context (tool_search and schema_search results)
    fn build_auto_discovery_section(&self) -> Option<String> {
        let mut sections: Vec<String> = Vec::new();

        // Auto tool search results
        if let Some(ref output) = self.auto_tool_search {
            if let Some(section) = system_prompt::build_auto_tool_search_section(&output.tools) {
                sections.push(section);
            }
        }

        // Auto schema search results (only if state doesn't already have schema context)
        // If the user has explicitly attached tables, we skip auto schema search
        // to avoid duplication and cluttering the prompt with unwanted tables.
        if !self.current_state.has_schema_context() && self.attached_tables.is_empty() {
            if let Some(ref output) = self.auto_schema_search {
                let sql_enabled = self.enabled_capabilities.contains(&Capability::SqlQuery) 
                    && output.tables.iter().map(|t| t.relevance).fold(0.0f32, f32::max) >= self.thresholds.schema_relevancy;
                
                if let Some(section) = system_prompt::build_auto_schema_search_section(
                    &output.tables, 
                    &output.summary, 
                    self.has_attachments,
                    sql_enabled,
                    self.tool_call_format,
                    self.model_tool_format
                ) {
                    sections.push(section);
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

        system_prompt::build_format_instructions(self.tool_call_format, self.model_tool_format)
    }

    /// Build MCP tool section from mcp_context.
    fn build_mcp_tool_section(&self) -> Option<String> {
        if !self.mcp_context.has_any_tools() {
            return None;
        }

        let mut parts = Vec::new();

        // Active tools (can be called immediately)
        if self.mcp_context.has_active_tools() {
            if let Some(mcp_section) = system_prompt::build_mcp_tools_documentation(
                &self.mcp_context.active_tools,
                &self.mcp_context.servers,
                &self.custom_tool_prompts,
            ) {
                parts.push(mcp_section);
            }
        }

        // Deferred tools (require discovery)
        if self.mcp_context.has_deferred_tools() {
            parts.push(system_prompt::build_deferred_mcp_tool_summary(
                self.mcp_context.deferred_tool_count(),
                self.mcp_context.deferred_tools.len()
            ));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
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


    /// Generate the Python execution prompt section.
    fn python_execution_prompt(&self, available_tools: &[String]) -> String {
        let mut prompt = system_prompt::build_python_prompt(available_tools, self.has_attachments);

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

    fn format_table_list(&self, tables: &[TableInfo]) -> String {
        system_prompt::format_table_list(tables)
    }

    fn format_rag_chunks(&self, chunks: &[RagChunk]) -> String {
        system_prompt::format_rag_chunks(chunks)
    }

    fn format_tool_schemas(&self, schemas: &[ToolSchema]) -> String {
        system_prompt::format_tool_schemas(schemas)
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

    fn create_test_machine(settings: &AppSettings, _filter: &ToolLaunchFilter, _thresholds: RelevancyThresholds, prompt: String) -> AgenticStateMachine {
        let settings_sm = SettingsStateMachine::from_settings(settings, &ToolLaunchFilter::default());
        AgenticStateMachine::new_from_settings_sm(
            &settings_sm,
            crate::agentic_state::PromptContext {
                base_prompt: prompt,
                mcp_context: crate::agentic_state::McpToolContext::default(),
                attached_tables: Vec::new(),
                attached_tools: Vec::new(),
                tool_call_format: ToolCallFormatName::Hermes,
                model_tool_format: None,
                custom_tool_prompts: HashMap::new(),
                python_primary: false,
                has_attachments: false,
            },
        )
    }

    #[test]
    fn test_state_machine_creation() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let machine = create_test_machine(&settings, &filter, thresholds, "Test prompt".to_string());

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
            create_test_machine(&settings, &filter, thresholds, "Test".to_string());

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
            create_test_machine(&settings, &filter, thresholds, "Test".to_string());

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
            create_test_machine(&settings, &filter, thresholds, "Test".to_string());

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
            create_test_machine(&settings, &filter, thresholds, "Test".to_string());

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

        // SQL tools should still be allowed in commentary state for multi-query scenarios
        assert!(machine.is_tool_allowed("sql_select"));
        assert!(machine.is_tool_allowed("schema_search"));
        assert!(!machine.is_tool_allowed("python_execution"));
        assert!(machine.should_continue_loop());
    }

    #[test]
    fn test_python_stderr_handoff() {
        let settings = test_settings();
        let filter = ToolLaunchFilter::default();
        let thresholds = RelevancyThresholds::default();

        let mut machine =
            create_test_machine(&settings, &filter, thresholds, "Test".to_string());

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

        let machine = create_test_machine(&settings, &filter, thresholds, "Test".to_string());

        let previews = machine.get_possible_states();

        // Should have multiple states
        assert!(previews.len() >= 4);

        // Check that Conversational is always present
        assert!(previews.iter().any(|p| p.name == "Conversational"));

        // Check that SQL Retrieval is present (since sql_select is enabled)
        assert!(previews.iter().any(|p| p.name == "SQL Retrieval"));
    }

    #[test]
    fn test_turn_attached_python_enables_code_execution() {
        // Scenario: python_execution_enabled=false in settings, but user explicitly
        // attaches python_execution for this turn. The state machine should enable
        // CodeExecution mode and generate Python guidance.
        let mut settings = AppSettings::default();
        settings.python_execution_enabled = false; // Disabled in settings
        settings.sql_select_enabled = true;
        
        let filter = ToolLaunchFilter::default();
        let settings_sm = SettingsStateMachine::from_settings(&settings, &filter);
        
        // Create state machine with python_execution in attached_tools
        let mut machine = AgenticStateMachine::new_from_settings_sm(
            &settings_sm,
            crate::agentic_state::PromptContext {
                base_prompt: "Test".to_string(),
                mcp_context: crate::agentic_state::McpToolContext::default(),
                attached_tables: Vec::new(),
                attached_tools: vec!["builtin::python_execution".to_string()],
                tool_call_format: ToolCallFormatName::Hermes,
                model_tool_format: None,
                custom_tool_prompts: HashMap::new(),
                python_primary: false,
                has_attachments: false,
            },
        );
        
        // Initially, PythonExecution should NOT be in capabilities (setting is disabled)
        assert!(!machine.enabled_capabilities().contains(&Capability::PythonExecution));
        
        // Compute turn config - this should add PythonExecution to capabilities
        machine.compute_turn_config(&settings, &filter);
        
        // Now PythonExecution SHOULD be in capabilities
        assert!(machine.enabled_capabilities().contains(&Capability::PythonExecution));
        
        // The state should be CodeExecution
        assert!(matches!(machine.current_state(), AgenticState::CodeExecution { .. }));
        
        // Python should be allowed
        assert!(machine.is_tool_allowed("python_execution"));
        
        // The system prompt should contain Python guidance
        let prompt = machine.build_system_prompt();
        assert!(prompt.contains("Python"), "System prompt should contain Python guidance");
        assert!(prompt.contains("print"), "System prompt should contain print guidance");
        
        // CRITICAL: The prompt should NOT contain conflicting tool_call format instructions
        // because python_primary should be true after compute_turn_config
        assert!(!prompt.contains("## Tool Calling Format"), 
            "System prompt should NOT contain Tool Calling Format section when Python mode is active");
        assert!(prompt.contains("EXAMPLE"), 
            "System prompt should contain Python example");
    }

    #[test]
    fn test_turn_attached_mcp_tool_enables_tool_orchestration() {
        // Scenario: No MCP servers are enabled in settings, but user explicitly
        // attaches an MCP tool for this turn. The state machine should enable
        // ToolOrchestration mode.
        let settings = AppSettings::default(); // No MCP servers configured
        
        let filter = ToolLaunchFilter::default();
        let settings_sm = SettingsStateMachine::from_settings(&settings, &filter);
        
        // Create state machine with an MCP tool in attached_tools
        let mut machine = AgenticStateMachine::new_from_settings_sm(
            &settings_sm,
            crate::agentic_state::PromptContext {
                base_prompt: "Test".to_string(),
                mcp_context: crate::agentic_state::McpToolContext::default(),
                attached_tables: Vec::new(),
                attached_tools: vec!["my-mcp-server::some_tool".to_string()],
                tool_call_format: ToolCallFormatName::Hermes,
                model_tool_format: None,
                custom_tool_prompts: HashMap::new(),
                python_primary: false,
                has_attachments: false,
            },
        );
        
        // Initially, McpTools should NOT be in capabilities (no MCP servers enabled)
        assert!(!machine.enabled_capabilities().contains(&Capability::McpTools));
        
        // Compute turn config - this should add McpTools to capabilities
        machine.compute_turn_config(&settings, &filter);
        
        // Now McpTools SHOULD be in capabilities
        assert!(machine.enabled_capabilities().contains(&Capability::McpTools));
        
        // The state should be ToolOrchestration
        assert!(matches!(machine.current_state(), AgenticState::ToolOrchestration { .. }));
    }

    #[test]
    fn test_turn_attached_table_enables_sql_mode() {
        // Scenario: sql_select is enabled but no tables attached by default.
        // User explicitly attaches a table. The state machine should enable
        // SqlRetrieval mode and include SQL guidance.
        let mut settings = AppSettings::default();
        settings.sql_select_enabled = true;
        
        let filter = ToolLaunchFilter::default();
        let settings_sm = SettingsStateMachine::from_settings(&settings, &filter);
        
        // Create state machine with an attached table
        let attached_table = crate::settings_state_machine::AttachedTableInfo {
            source_id: "test-source".to_string(),
            table_fq_name: "test_schema.test_table".to_string(),
            column_count: 5,
            schema_text: Some("CREATE TABLE test_schema.test_table (id INT, name TEXT);".to_string()),
        };
        
        let mut machine = AgenticStateMachine::new_from_settings_sm(
            &settings_sm,
            crate::agentic_state::PromptContext {
                base_prompt: "Test".to_string(),
                mcp_context: crate::agentic_state::McpToolContext::default(),
                attached_tables: vec![attached_table],
                attached_tools: Vec::new(),
                tool_call_format: ToolCallFormatName::Hermes,
                model_tool_format: None,
                custom_tool_prompts: HashMap::new(),
                python_primary: false,
                has_attachments: false,
            },
        );
        
        // Compute turn config
        machine.compute_turn_config(&settings, &filter);
        
        // The state should be SqlRetrieval
        assert!(matches!(machine.current_state(), AgenticState::SqlRetrieval { .. }));
        
        // SQL should be allowed
        assert!(machine.is_tool_allowed("sql_select"));
        
        // The system prompt should contain the attached table schema
        let prompt = machine.build_system_prompt();
        assert!(prompt.contains("test_schema.test_table") || prompt.contains("test_table"), 
            "System prompt should contain attached table reference");
        assert!(prompt.contains("sql_select"), "System prompt should contain sql_select guidance");
    }
}

