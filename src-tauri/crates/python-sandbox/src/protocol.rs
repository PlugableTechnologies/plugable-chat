//! Protocol types for host-WASM communication
//!
//! These types define the JSON-based protocol between the host (Wasmtime)
//! and the sandboxed Python execution environment.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Information about an available tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Tool name
    pub name: String,
    /// Server ID that provides this tool
    pub server_id: String,
    /// Tool description
    pub description: Option<String>,
    /// Parameter schema (JSON Schema)
    pub parameters: Value,
}

/// Request from host to execute Python code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    /// Lines of Python code to execute
    pub code: Vec<String>,
    /// Optional context/variables to inject
    pub context: Option<Value>,
    /// Results from previous tool calls (for continuation)
    pub tool_results: HashMap<String, ToolCallResult>,
    /// Available tools that can be called
    pub available_tools: Vec<ToolInfo>,
}

/// Result of a tool call from a previous round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Whether the call succeeded
    pub success: bool,
    /// The result value (if successful)
    pub result: Value,
    /// Error message (if failed)
    pub error: Option<String>,
}

/// A pending tool call that needs to be executed by the host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingToolCall {
    /// Unique ID for this call (used to match results)
    pub id: String,
    /// Name of the tool to call
    pub tool_name: String,
    /// Server ID that provides the tool
    pub server_id: String,
    /// Arguments to pass to the tool
    pub arguments: Value,
}

/// Status of execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExecutionStatus {
    /// Execution completed successfully
    Complete,
    /// Execution paused waiting for tool call results
    ToolCallsPending,
    /// Execution failed with an error
    Error(String),
    /// Execution timed out (via epochs)
    Timeout,
    /// Execution ran out of fuel
    OutOfFuel,
}

/// Result returned from WASM to host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Status of execution
    pub status: ExecutionStatus,
    /// Standard output from Python
    pub stdout: String,
    /// Standard error from Python
    pub stderr: String,
    /// Return value from the code (if any)
    pub result: Option<Value>,
    /// Tool calls that need to be executed
    pub pending_calls: Vec<PendingToolCall>,
    /// Number of tool calls made in this execution
    pub tool_calls_made: usize,
}

impl Default for ExecutionResult {
    fn default() -> Self {
        Self {
            status: ExecutionStatus::Complete,
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            pending_calls: Vec::new(),
            tool_calls_made: 0,
        }
    }
}

impl ExecutionResult {
    /// Create a result indicating an error
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: ExecutionStatus::Error(message.into()),
            ..Default::default()
        }
    }
    
    /// Create a result with pending tool calls
    pub fn with_pending_calls(calls: Vec<PendingToolCall>) -> Self {
        Self {
            status: ExecutionStatus::ToolCallsPending,
            pending_calls: calls,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_execution_request_serialization() {
        let request = ExecutionRequest {
            code: vec!["x = 1".to_string(), "print(x)".to_string()],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let json = serde_json::to_string(&request).unwrap();
        let parsed: ExecutionRequest = serde_json::from_str(&json).unwrap();
        
        assert_eq!(parsed.code.len(), 2);
    }
    
    #[test]
    fn test_execution_result_default() {
        let result = ExecutionResult::default();
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.pending_calls.is_empty());
    }
}


