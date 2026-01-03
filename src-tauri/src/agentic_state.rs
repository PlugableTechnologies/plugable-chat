//! Agentic State Machine - State Definitions
//!
//! Defines the states, capabilities, and events for the agentic loop state machine.
//! The state machine controls which tools are available and how the system prompt
//! is constructed based on context (RAG attachments, schema search results, etc.).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;

use crate::protocol::{ToolSchema, ToolFormat};
use crate::settings::ToolCallFormatName;

// ============ MCP Tool Context ============

/// Simplified MCP tool info for state machine (avoids direct dependency on mcp_host_actor)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    /// JSON schema for parameters
    pub parameters_schema: Option<serde_json::Value>,
    /// Optional examples for usage
    pub input_examples: Option<Vec<serde_json::Value>>,
}

/// Simplified MCP server info for state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub id: String,
    pub name: String,
    pub auto_approve_tools: bool,
    /// Environment variables (non-sensitive ones for context)
    pub visible_env: HashMap<String, String>,
}

/// MCP tool context for prompt building
/// Contains all MCP-related information needed to build the system prompt.
#[derive(Debug, Clone, Default)]
pub struct McpToolContext {
    /// Active MCP tools that can be called immediately (server_id -> tools)
    pub active_tools: Vec<(String, Vec<McpToolInfo>)>,
    /// Deferred MCP tools that require tool_search discovery (server_id -> tools)
    pub deferred_tools: Vec<(String, Vec<McpToolInfo>)>,
    /// Server information for context (env vars, auto-approve status)
    pub servers: Vec<McpServerInfo>,
}

impl McpToolInfo {
    /// Create from an external McpTool (from mcp_host_actor)
    pub fn from_mcp_tool(tool: &crate::actors::mcp_host_actor::McpTool) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters_schema: tool.input_schema.clone(),
            input_examples: tool.input_examples.clone(),
        }
    }
}

impl McpToolContext {
    /// Build from active and deferred tool lists (from lib.rs format)
    pub fn from_tool_lists(
        active: &[(String, Vec<crate::actors::mcp_host_actor::McpTool>)],
        deferred: &[(String, Vec<crate::actors::mcp_host_actor::McpTool>)],
        server_configs: &[crate::settings::McpServerConfig],
    ) -> Self {
        let active_tools: Vec<(String, Vec<McpToolInfo>)> = active
            .iter()
            .map(|(server_id, tools)| {
                let infos: Vec<McpToolInfo> = tools.iter().map(McpToolInfo::from_mcp_tool).collect();
                (server_id.clone(), infos)
            })
            .collect();

        let deferred_tools: Vec<(String, Vec<McpToolInfo>)> = deferred
            .iter()
            .map(|(server_id, tools)| {
                let infos: Vec<McpToolInfo> = tools.iter().map(McpToolInfo::from_mcp_tool).collect();
                (server_id.clone(), infos)
            })
            .collect();

        // Build server info (filter out sensitive env vars)
        let servers: Vec<McpServerInfo> = server_configs
            .iter()
            .filter(|c| c.enabled)
            .map(|c| {
                let visible_env: HashMap<String, String> = c.env
                    .iter()
                    .filter(|(k, _)| {
                        let lower = k.to_lowercase();
                        !lower.contains("secret")
                            && !lower.contains("password")
                            && !lower.contains("token")
                            && !lower.contains("key")
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                McpServerInfo {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    auto_approve_tools: c.auto_approve_tools,
                    visible_env,
                }
            })
            .collect();

        Self {
            active_tools,
            deferred_tools,
            servers,
        }
    }

    /// Check if there are any active tools
    pub fn has_active_tools(&self) -> bool {
        self.active_tools.iter().any(|(_, tools)| !tools.is_empty())
    }

    /// Check if there are any deferred tools
    pub fn has_deferred_tools(&self) -> bool {
        self.deferred_tools.iter().any(|(_, tools)| !tools.is_empty())
    }

    /// Check if there are any MCP tools at all
    pub fn has_any_tools(&self) -> bool {
        self.has_active_tools() || self.has_deferred_tools()
    }

    /// Count total active tools
    pub fn active_tool_count(&self) -> usize {
        self.active_tools.iter().map(|(_, t)| t.len()).sum()
    }

    /// Count total deferred tools
    pub fn deferred_tool_count(&self) -> usize {
        self.deferred_tools.iter().map(|(_, t)| t.len()).sum()
    }
}

// ============ Prompt Context ============

/// Full context needed for system prompt building.
/// This is passed to the state machine to enable unified prompt generation.
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// User's base system prompt
    pub base_prompt: String,
    /// Whether user has attached documents
    pub has_attachments: bool,
    /// Per-chat attached database tables
    pub attached_tables: Vec<crate::settings_state_machine::AttachedTableInfo>,
    /// Per-chat attached tools
    pub attached_tools: Vec<String>,
    /// MCP tool context
    pub mcp_context: McpToolContext,
    /// Tool call format to use
    pub tool_call_format: ToolCallFormatName,
    /// Model-specific tool format preference
    pub model_tool_format: Option<ToolFormat>,
    /// Custom prompts per tool (key: "server_id::tool_name")
    pub custom_tool_prompts: HashMap<String, String>,
    /// Whether this is a Python-primary mode (Code Mode)
    pub python_primary: bool,
}

impl Default for PromptContext {
    fn default() -> Self {
        Self {
            base_prompt: String::new(),
            has_attachments: false,
            attached_tables: Vec::new(),
            attached_tools: Vec::new(),
            mcp_context: McpToolContext::default(),
            tool_call_format: ToolCallFormatName::Hermes,
            model_tool_format: None,
            custom_tool_prompts: HashMap::new(),
            python_primary: false,
        }
    }
}

// ============ Relevancy Thresholds ============

/// Relevancy thresholds for context injection and tool gating.
/// These control when RAG chunks and schema tables are injected into the prompt,
/// and when SQL execution is enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevancyThresholds {
    /// Minimum RAG chunk relevancy to inject into context (default: 0.3)
    pub rag_chunk_min: f32,
    /// Minimum schema relevancy to enable sql_select and inject into context (default: 0.4)
    pub schema_relevancy: f32,
    /// RAG relevancy above which SQL context is suppressed (default: 0.6)
    pub rag_dominant_threshold: f32,
}

impl Default for RelevancyThresholds {
    fn default() -> Self {
        Self {
            rag_chunk_min: 0.3,
            schema_relevancy: 0.4,
            rag_dominant_threshold: 0.6,
        }
    }
}

// ============ Capability Enum ============

/// Capabilities that can be enabled/disabled in the state machine.
/// Each capability corresponds to a class of tools or behaviors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// RAG document retrieval
    Rag,
    /// SQL query execution
    SqlQuery,
    /// MCP tool orchestration
    McpTools,
    /// Python code execution
    PythonExecution,
    /// Schema search (database table discovery)
    SchemaSearch,
    /// Tool search (MCP tool discovery)
    ToolSearch,
}

// ============ Context Data Structures ============

/// A retrieved RAG chunk with relevancy score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagChunk {
    /// The text content of the chunk
    pub content: String,
    /// Source file path or identifier
    pub source_file: String,
    /// Relevancy score (0.0 to 1.0)
    pub relevancy: f32,
}

/// Column information for a table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub description: Option<String>,
    /// Special attributes: "primary_key", "foreign_key", "partition", "cluster"
    #[serde(default)]
    pub special_attributes: Vec<String>,
    /// Top 3 most common values with percentage (e.g., "THEFT (23.5%)")
    #[serde(default)]
    pub top_values: Vec<String>,
}

/// Table info with relevancy for state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    /// Fully qualified table name (e.g., project.dataset.table)
    pub fully_qualified_name: String,
    /// Database source ID
    pub source_id: String,
    /// SQL dialect (e.g., "GoogleSQL", "PostgreSQL")
    pub sql_dialect: String,
    /// Relevancy score (0.0 to 1.0)
    pub relevancy: f32,
    /// Column information
    pub columns: Vec<ColumnInfo>,
    /// Optional table description
    pub description: Option<String>,
}

/// SQL query results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlResults {
    /// Column names
    pub columns: Vec<String>,
    /// Rows of data (each row is a vector of string values)
    pub rows: Vec<Vec<String>>,
    /// Number of rows returned
    pub row_count: usize,
    /// Whether results were truncated
    pub truncated: bool,
}

// ============ Agentic State Enum ============

/// The current state of the agentic loop.
/// 
/// States are divided into:
/// - **Turn-Start States**: Computed from settings and context at the start of a turn
/// - **Mid-Turn States**: Triggered by auto-discovery or tool execution results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgenticState {
    // === Turn-Start States (computed from settings + context) ===

    /// Base conversational state - no tools available
    Conversational,

    /// RAG-focused state - document Q&A
    RagRetrieval {
        /// Max relevancy across all retrieved chunks
        max_chunk_relevancy: f32,
        /// Schema relevancy (may still have SQL context if not suppressed)
        schema_relevancy: f32,
    },

    /// SQL-focused state - database queries
    SqlRetrieval {
        /// Discovered tables from schema search
        discovered_tables: Vec<TableInfo>,
        /// Max relevancy across discovered tables
        max_table_relevancy: f32,
    },

    /// MCP tool-focused state
    ToolOrchestration {
        /// Tools that have been materialized (made visible)
        materialized_tools: Vec<String>,
    },

    /// Python code execution state
    CodeExecution {
        /// Tools available for calling from Python
        available_tools: Vec<String>,
    },

    /// Hybrid state - multiple capabilities active with relevancy scores
    Hybrid {
        /// Set of active capabilities
        active_capabilities: HashSet<Capability>,
        /// RAG relevancy score
        rag_relevancy: f32,
        /// Schema relevancy score
        schema_relevancy: f32,
    },

    // === Mid-Turn States (after auto-discovery or tool execution) ===

    /// After RAG retrieval finds relevant chunks - context injected
    RagContextInjected {
        /// Retrieved chunks above threshold
        chunks: Vec<RagChunk>,
        /// Max relevancy across chunks
        max_relevancy: f32,
        /// Whether user can see source citations
        user_can_see_sources: bool,
    },

    /// After schema_search finds relevant tables - context injected
    SchemaContextInjected {
        /// Discovered tables above threshold
        tables: Vec<TableInfo>,
        /// Max relevancy across tables
        max_relevancy: f32,
        /// Whether sql_select is enabled (relevancy >= sql_enable_min)
        sql_enabled: bool,
    },

    /// After SQL returns data - model provides commentary
    SqlResultCommentary {
        /// Whether results have been shown to user
        results_shown_to_user: bool,
        /// Number of rows in results
        row_count: usize,
        /// Original query context for reference
        query_context: String,
    },

    /// After Python execution with stderr - handoff for continuation
    CodeExecutionHandoff {
        /// Stdout that was shown to user
        stdout_shown_to_user: String,
        /// Stderr content for model to process
        stderr_for_model: String,
    },

    /// After tool_search discovers tools - ready to use them
    ToolsDiscovered {
        /// Newly materialized tool names
        newly_materialized: Vec<String>,
        /// Tool schemas available for calling
        available_for_call: Vec<ToolSchema>,
    },
}

impl AgenticState {
    /// Get a human-readable name for the state
    pub fn name(&self) -> &'static str {
        match self {
            AgenticState::Conversational => "Conversational",
            AgenticState::RagRetrieval { .. } => "RAG Retrieval",
            AgenticState::SqlRetrieval { .. } => "SQL Retrieval",
            AgenticState::ToolOrchestration { .. } => "Tool Orchestration",
            AgenticState::CodeExecution { .. } => "Code Execution",
            AgenticState::Hybrid { .. } => "Hybrid",
            AgenticState::RagContextInjected { .. } => "RAG Context Injected",
            AgenticState::SchemaContextInjected { .. } => "Schema Context Injected",
            AgenticState::SqlResultCommentary { .. } => "SQL Result Commentary",
            AgenticState::CodeExecutionHandoff { .. } => "Code Execution Handoff",
            AgenticState::ToolsDiscovered { .. } => "Tools Discovered",
        }
    }

    /// Check if this is a turn-start state (vs mid-turn)
    pub fn is_turn_start_state(&self) -> bool {
        matches!(
            self,
            AgenticState::Conversational
                | AgenticState::RagRetrieval { .. }
                | AgenticState::SqlRetrieval { .. }
                | AgenticState::ToolOrchestration { .. }
                | AgenticState::CodeExecution { .. }
                | AgenticState::Hybrid { .. }
        )
    }

    /// Check if this is a mid-turn state
    pub fn is_mid_turn_state(&self) -> bool {
        !self.is_turn_start_state()
    }

    /// Check if this state includes schema context in the system prompt.
    /// Used to avoid duplicating schema info from auto-discovery.
    pub fn has_schema_context(&self) -> bool {
        match self {
            AgenticState::SqlRetrieval { discovered_tables, .. } => !discovered_tables.is_empty(),
            AgenticState::SchemaContextInjected { tables, .. } => !tables.is_empty(),
            AgenticState::Hybrid { active_capabilities, .. } => {
                active_capabilities.contains(&Capability::SqlQuery)
            }
            _ => false,
        }
    }

    /// Get the capabilities active in this state
    pub fn active_capabilities(&self) -> HashSet<Capability> {
        match self {
            AgenticState::Conversational => HashSet::new(),
            
            AgenticState::RagRetrieval { schema_relevancy, .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::Rag);
                if *schema_relevancy > 0.0 {
                    caps.insert(Capability::SchemaSearch);
                }
                caps
            }
            
            AgenticState::SqlRetrieval { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::SqlQuery);
                caps.insert(Capability::SchemaSearch);
                caps
            }
            
            AgenticState::ToolOrchestration { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::McpTools);
                caps.insert(Capability::ToolSearch);
                caps
            }
            
            AgenticState::CodeExecution { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::PythonExecution);
                caps
            }
            
            AgenticState::Hybrid { active_capabilities, .. } => active_capabilities.clone(),
            
            AgenticState::RagContextInjected { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::Rag);
                caps
            }
            
            AgenticState::SchemaContextInjected { sql_enabled, .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::SchemaSearch);
                if *sql_enabled {
                    caps.insert(Capability::SqlQuery);
                }
                caps
            }
            
            // Allow SQL continuation for multi-query scenarios
            AgenticState::SqlResultCommentary { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::SqlQuery);
                caps.insert(Capability::SchemaSearch);
                caps
            }
            
            AgenticState::CodeExecutionHandoff { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::PythonExecution);
                caps
            }
            
            AgenticState::ToolsDiscovered { .. } => {
                let mut caps = HashSet::new();
                caps.insert(Capability::McpTools);
                caps
            }
        }
    }
}

// ============ State Events ============

/// Events that trigger state transitions mid-turn.
/// These are produced by auto-discovery and tool execution.
#[derive(Debug, Clone)]
pub enum StateEvent {
    /// RAG retrieval completed with chunks above threshold
    RagRetrieved {
        chunks: Vec<RagChunk>,
        max_relevancy: f32,
    },
    
    /// Schema search completed with tables above threshold
    SchemaSearched {
        tables: Vec<TableInfo>,
        max_relevancy: f32,
    },
    
    /// SQL query executed successfully
    SqlExecuted {
        results: SqlResults,
        row_count: usize,
    },
    
    /// Python code executed
    PythonExecuted {
        stdout: String,
        stderr: String,
    },
    
    /// Tool search discovered new tools
    ToolSearchCompleted {
        discovered: Vec<String>,
        schemas: Vec<ToolSchema>,
    },
    
    /// MCP tool executed
    McpToolExecuted {
        tool_name: String,
        result: String,
    },
    
    /// Model produced final response (no tool calls)
    ModelResponseFinal,
}

// ============ Tests ============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_thresholds() {
        let thresholds = RelevancyThresholds::default();
        assert!((thresholds.rag_chunk_min - 0.3).abs() < 0.001);
        assert!((thresholds.schema_relevancy - 0.4).abs() < 0.001);
        assert!((thresholds.rag_dominant_threshold - 0.6).abs() < 0.001);
    }

    #[test]
    fn test_state_names() {
        assert_eq!(AgenticState::Conversational.name(), "Conversational");
        assert_eq!(
            AgenticState::RagRetrieval {
                max_chunk_relevancy: 0.5,
                schema_relevancy: 0.0
            }
            .name(),
            "RAG Retrieval"
        );
    }

    #[test]
    fn test_turn_start_vs_mid_turn() {
        assert!(AgenticState::Conversational.is_turn_start_state());
        assert!(AgenticState::SqlRetrieval {
            discovered_tables: vec![],
            max_table_relevancy: 0.5
        }
        .is_turn_start_state());

        assert!(AgenticState::SqlResultCommentary {
            results_shown_to_user: true,
            row_count: 10,
            query_context: "test".to_string()
        }
        .is_mid_turn_state());
        
        assert!(AgenticState::CodeExecutionHandoff {
            stdout_shown_to_user: "output".to_string(),
            stderr_for_model: "handoff".to_string()
        }
        .is_mid_turn_state());
    }

    #[test]
    fn test_active_capabilities() {
        let conversational = AgenticState::Conversational;
        assert!(conversational.active_capabilities().is_empty());

        let rag_state = AgenticState::RagRetrieval {
            max_chunk_relevancy: 0.5,
            schema_relevancy: 0.3,
        };
        let rag_caps = rag_state.active_capabilities();
        assert!(rag_caps.contains(&Capability::Rag));
        assert!(rag_caps.contains(&Capability::SchemaSearch));

        let sql_state = AgenticState::SqlRetrieval {
            discovered_tables: vec![],
            max_table_relevancy: 0.5,
        };
        let sql_caps = sql_state.active_capabilities();
        assert!(sql_caps.contains(&Capability::SqlQuery));
        assert!(sql_caps.contains(&Capability::SchemaSearch));
    }
}

