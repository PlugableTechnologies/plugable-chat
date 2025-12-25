//! Mid-Turn State Machine
//!
//! Manages state during tool execution within a single turn.
//! This is Tier 3 of the three-tier state machine hierarchy:
//!
//! 1. SettingsStateMachine - Settings -> OperationalMode
//! 2. AgenticStateMachine - OperationalMode + Context -> AgenticState (turn-start)
//! 3. MidTurnStateMachine (this module) - AgenticState + Events -> MidTurnState
//!
//! The MidTurnStateMachine handles the dynamic state during the agentic loop,
//! tracking tool executions and determining when to continue or complete.

use serde::{Deserialize, Serialize};

use crate::agentic_state::{RagChunk, SqlResults, TableInfo};
use crate::protocol::ToolSchema;

// ============ Mid-Turn State ============

/// State during tool execution within a single turn.
///
/// This captures what happened during tool execution and determines
/// how the loop should proceed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidTurnState {
    /// Waiting for the model to produce a response
    AwaitingModelResponse,

    /// Currently processing a tool call
    ProcessingToolCall {
        /// Name of the tool being called
        tool_name: String,
        /// Server ID (for MCP tools) or "builtin"
        server_id: String,
    },

    /// RAG retrieval completed - context injected
    RagContextReady {
        /// Retrieved chunks above threshold
        chunks: Vec<RagChunk>,
        /// Max relevancy across chunks
        max_relevancy: f32,
        /// Whether user can see source citations
        user_can_see_sources: bool,
    },

    /// Schema search completed - tables discovered
    SchemaContextReady {
        /// Discovered tables above threshold
        tables: Vec<TableInfo>,
        /// Max relevancy across tables
        max_relevancy: f32,
        /// Whether sql_select is enabled
        sql_enabled: bool,
    },

    /// SQL query executed - results ready for commentary
    SqlResultsReturned {
        /// Number of rows returned
        row_count: usize,
        /// Whether results have been shown to user
        results_shown_to_user: bool,
        /// Original query context
        query_context: String,
        /// The full results (for reference)
        results: SqlResults,
    },

    /// Python code executed - check for continuation
    PythonExecuted {
        /// Stdout shown to user
        stdout: String,
        /// Stderr for model (handoff channel)
        stderr: String,
        /// Whether this requires continuation
        needs_continuation: bool,
    },

    /// Tool search discovered new tools
    ToolsDiscovered {
        /// Newly materialized tool names
        newly_materialized: Vec<String>,
        /// Tool schemas available for calling
        available_for_call: Vec<ToolSchema>,
    },

    /// MCP tool executed successfully
    McpToolCompleted {
        /// Tool name that was executed
        tool_name: String,
        /// Server ID
        server_id: String,
        /// Result content
        result: String,
    },

    /// Turn is complete - no more tool calls needed
    TurnComplete,

    /// Error occurred during tool execution
    Error {
        /// Error message
        message: String,
        /// Whether this is recoverable
        recoverable: bool,
    },
}

impl MidTurnState {
    /// Get the display name for this state
    pub fn name(&self) -> &'static str {
        match self {
            MidTurnState::AwaitingModelResponse => "Awaiting Model Response",
            MidTurnState::ProcessingToolCall { .. } => "Processing Tool Call",
            MidTurnState::RagContextReady { .. } => "RAG Context Ready",
            MidTurnState::SchemaContextReady { .. } => "Schema Context Ready",
            MidTurnState::SqlResultsReturned { .. } => "SQL Results Returned",
            MidTurnState::PythonExecuted { .. } => "Python Executed",
            MidTurnState::ToolsDiscovered { .. } => "Tools Discovered",
            MidTurnState::McpToolCompleted { .. } => "MCP Tool Completed",
            MidTurnState::TurnComplete => "Turn Complete",
            MidTurnState::Error { .. } => "Error",
        }
    }

    /// Check if this state requires the model to continue (another iteration)
    pub fn requires_continuation(&self) -> bool {
        match self {
            MidTurnState::AwaitingModelResponse => false,
            MidTurnState::ProcessingToolCall { .. } => false,
            MidTurnState::RagContextReady { .. } => false, // Context injected, model responds
            MidTurnState::SchemaContextReady { sql_enabled, .. } => *sql_enabled, // May need SQL
            MidTurnState::SqlResultsReturned { .. } => true, // Need commentary
            MidTurnState::PythonExecuted { needs_continuation, .. } => *needs_continuation,
            MidTurnState::ToolsDiscovered { .. } => true, // Need to use discovered tools
            MidTurnState::McpToolCompleted { .. } => true, // May need follow-up
            MidTurnState::TurnComplete => false,
            MidTurnState::Error { recoverable, .. } => *recoverable,
        }
    }

    /// Check if this state indicates the turn is complete
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            MidTurnState::TurnComplete | MidTurnState::Error { recoverable: false, .. }
        )
    }
}

// ============ Mid-Turn Events ============

/// Events that trigger mid-turn state transitions.
#[derive(Debug, Clone)]
pub enum MidTurnEvent {
    /// Model started generating a response
    ModelResponseStarted,

    /// Model response completed (no tool calls)
    ModelResponseFinal,

    /// Model invoked a tool
    ToolCallStarted {
        tool_name: String,
        server_id: String,
    },

    /// RAG retrieval completed
    RagRetrieved {
        chunks: Vec<RagChunk>,
        max_relevancy: f32,
    },

    /// Schema search completed
    SchemaSearched {
        tables: Vec<TableInfo>,
        max_relevancy: f32,
        sql_enabled: bool,
    },

    /// SQL query executed
    SqlExecuted {
        results: SqlResults,
        row_count: usize,
        query_context: String,
    },

    /// Python code executed
    PythonExecuted {
        stdout: String,
        stderr: String,
    },

    /// Tool search discovered tools
    ToolSearchCompleted {
        discovered: Vec<String>,
        schemas: Vec<ToolSchema>,
    },

    /// MCP tool executed
    McpToolExecuted {
        tool_name: String,
        server_id: String,
        result: String,
    },

    /// Error occurred
    ErrorOccurred {
        message: String,
        recoverable: bool,
    },
}

// ============ Mid-Turn State Machine ============

/// Manages state during tool execution within a single turn.
///
/// This is Tier 3 of the hierarchy. It takes the turn-start state from
/// AgenticStateMachine and manages transitions during the agentic loop.
#[derive(Debug, Clone)]
pub struct MidTurnStateMachine {
    /// Current mid-turn state
    current_state: MidTurnState,
    /// History of states for debugging
    state_history: Vec<MidTurnState>,
    /// Number of tool calls executed this turn
    tool_call_count: usize,
    /// Maximum tool calls per turn (safety limit)
    max_tool_calls: usize,
}

impl Default for MidTurnStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl MidTurnStateMachine {
    /// Create a new MidTurnStateMachine
    pub fn new() -> Self {
        Self {
            current_state: MidTurnState::AwaitingModelResponse,
            state_history: Vec::new(),
            tool_call_count: 0,
            max_tool_calls: 10, // Default safety limit
        }
    }

    /// Create with a custom max tool calls limit
    pub fn with_max_tool_calls(max_tool_calls: usize) -> Self {
        Self {
            current_state: MidTurnState::AwaitingModelResponse,
            state_history: Vec::new(),
            tool_call_count: 0,
            max_tool_calls,
        }
    }

    /// Get the current state
    pub fn current_state(&self) -> &MidTurnState {
        &self.current_state
    }

    /// Get the state history
    pub fn state_history(&self) -> &[MidTurnState] {
        &self.state_history
    }

    /// Get the number of tool calls executed this turn
    pub fn tool_call_count(&self) -> usize {
        self.tool_call_count
    }

    /// Check if the loop should continue (more iterations needed)
    pub fn should_continue(&self) -> bool {
        if self.current_state.is_terminal() {
            return false;
        }

        if self.tool_call_count >= self.max_tool_calls {
            println!(
                "[MidTurnStateMachine] Reached max tool calls ({}), stopping",
                self.max_tool_calls
            );
            return false;
        }

        self.current_state.requires_continuation()
    }

    /// Handle an event and transition to a new state
    pub fn handle_event(&mut self, event: MidTurnEvent) {
        // Record current state in history
        self.state_history.push(self.current_state.clone());

        let new_state = match event {
            MidTurnEvent::ModelResponseStarted => MidTurnState::AwaitingModelResponse,

            MidTurnEvent::ModelResponseFinal => MidTurnState::TurnComplete,

            MidTurnEvent::ToolCallStarted {
                tool_name,
                server_id,
            } => {
                self.tool_call_count += 1;
                MidTurnState::ProcessingToolCall {
                    tool_name,
                    server_id,
                }
            }

            MidTurnEvent::RagRetrieved {
                chunks,
                max_relevancy,
            } => MidTurnState::RagContextReady {
                chunks,
                max_relevancy,
                user_can_see_sources: true,
            },

            MidTurnEvent::SchemaSearched {
                tables,
                max_relevancy,
                sql_enabled,
            } => MidTurnState::SchemaContextReady {
                tables,
                max_relevancy,
                sql_enabled,
            },

            MidTurnEvent::SqlExecuted {
                results,
                row_count,
                query_context,
            } => MidTurnState::SqlResultsReturned {
                row_count,
                results_shown_to_user: true,
                query_context,
                results,
            },

            MidTurnEvent::PythonExecuted { stdout, stderr } => {
                let needs_continuation = !stderr.trim().is_empty();
                MidTurnState::PythonExecuted {
                    stdout,
                    stderr,
                    needs_continuation,
                }
            }

            MidTurnEvent::ToolSearchCompleted {
                discovered,
                schemas,
            } => MidTurnState::ToolsDiscovered {
                newly_materialized: discovered,
                available_for_call: schemas,
            },

            MidTurnEvent::McpToolExecuted {
                tool_name,
                server_id,
                result,
            } => MidTurnState::McpToolCompleted {
                tool_name,
                server_id,
                result,
            },

            MidTurnEvent::ErrorOccurred {
                message,
                recoverable,
            } => MidTurnState::Error {
                message,
                recoverable,
            },
        };

        println!(
            "[MidTurnStateMachine] {} -> {} (tool_calls: {})",
            self.current_state.name(),
            new_state.name(),
            self.tool_call_count
        );

        self.current_state = new_state;
    }

    /// Reset the state machine for a new turn
    pub fn reset(&mut self) {
        self.current_state = MidTurnState::AwaitingModelResponse;
        self.state_history.clear();
        self.tool_call_count = 0;
    }

    /// Force transition to TurnComplete
    pub fn complete(&mut self) {
        self.state_history.push(self.current_state.clone());
        self.current_state = MidTurnState::TurnComplete;
    }

    /// Get a summary of what happened this turn
    pub fn turn_summary(&self) -> TurnSummary {
        TurnSummary {
            total_tool_calls: self.tool_call_count,
            states_visited: self.state_history.len() + 1,
            final_state: self.current_state.name().to_string(),
        }
    }
}

/// Summary of what happened during a turn
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSummary {
    pub total_tool_calls: usize,
    pub states_visited: usize,
    pub final_state: String,
}

// ============ Tests ============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let sm = MidTurnStateMachine::new();
        assert!(matches!(
            sm.current_state(),
            MidTurnState::AwaitingModelResponse
        ));
        assert_eq!(sm.tool_call_count(), 0);
    }

    #[test]
    fn test_tool_call_counting() {
        let mut sm = MidTurnStateMachine::new();

        sm.handle_event(MidTurnEvent::ToolCallStarted {
            tool_name: "test_tool".to_string(),
            server_id: "builtin".to_string(),
        });

        assert_eq!(sm.tool_call_count(), 1);
        assert!(matches!(
            sm.current_state(),
            MidTurnState::ProcessingToolCall { .. }
        ));
    }

    #[test]
    fn test_max_tool_calls_limit() {
        let mut sm = MidTurnStateMachine::with_max_tool_calls(2);

        // Simulate hitting the limit
        sm.handle_event(MidTurnEvent::ToolCallStarted {
            tool_name: "tool1".to_string(),
            server_id: "builtin".to_string(),
        });
        sm.handle_event(MidTurnEvent::ToolCallStarted {
            tool_name: "tool2".to_string(),
            server_id: "builtin".to_string(),
        });

        assert_eq!(sm.tool_call_count(), 2);
        assert!(!sm.should_continue());
    }

    #[test]
    fn test_python_continuation() {
        let mut sm = MidTurnStateMachine::new();

        // Python with stderr -> needs continuation
        sm.handle_event(MidTurnEvent::PythonExecuted {
            stdout: "output".to_string(),
            stderr: "handoff data".to_string(),
        });

        match sm.current_state() {
            MidTurnState::PythonExecuted {
                needs_continuation, ..
            } => {
                assert!(*needs_continuation);
            }
            _ => panic!("Expected PythonExecuted state"),
        }
        assert!(sm.should_continue());

        // Python without stderr -> no continuation
        sm.reset();
        sm.handle_event(MidTurnEvent::PythonExecuted {
            stdout: "output".to_string(),
            stderr: "".to_string(),
        });

        match sm.current_state() {
            MidTurnState::PythonExecuted {
                needs_continuation, ..
            } => {
                assert!(!*needs_continuation);
            }
            _ => panic!("Expected PythonExecuted state"),
        }
        assert!(!sm.should_continue());
    }

    #[test]
    fn test_turn_complete() {
        let mut sm = MidTurnStateMachine::new();

        sm.handle_event(MidTurnEvent::ModelResponseFinal);

        assert!(matches!(sm.current_state(), MidTurnState::TurnComplete));
        assert!(!sm.should_continue());
        assert!(sm.current_state().is_terminal());
    }

    #[test]
    fn test_state_history() {
        let mut sm = MidTurnStateMachine::new();

        sm.handle_event(MidTurnEvent::ToolCallStarted {
            tool_name: "test".to_string(),
            server_id: "builtin".to_string(),
        });
        sm.handle_event(MidTurnEvent::ModelResponseFinal);

        assert_eq!(sm.state_history().len(), 2);
    }

    #[test]
    fn test_reset() {
        let mut sm = MidTurnStateMachine::new();

        sm.handle_event(MidTurnEvent::ToolCallStarted {
            tool_name: "test".to_string(),
            server_id: "builtin".to_string(),
        });

        assert_eq!(sm.tool_call_count(), 1);

        sm.reset();

        assert_eq!(sm.tool_call_count(), 0);
        assert!(sm.state_history().is_empty());
        assert!(matches!(
            sm.current_state(),
            MidTurnState::AwaitingModelResponse
        ));
    }

    #[test]
    fn test_turn_summary() {
        let mut sm = MidTurnStateMachine::new();

        sm.handle_event(MidTurnEvent::ToolCallStarted {
            tool_name: "test".to_string(),
            server_id: "builtin".to_string(),
        });
        sm.handle_event(MidTurnEvent::ModelResponseFinal);

        let summary = sm.turn_summary();
        assert_eq!(summary.total_tool_calls, 1);
        assert_eq!(summary.states_visited, 3);
        assert_eq!(summary.final_state, "Turn Complete");
    }
}

