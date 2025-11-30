//! Code Execution Implementation
//!
//! Python/WASP code execution in a WASM sandbox.
//! This tool allows models to run Python code that can call other registered tools.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{ToolSchema, ExtendedToolCall, ToolCallCaller, ToolCallKind};

/// Input for the code_execution built-in tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionInput {
    /// Lines of Python code to execute
    pub code: Vec<String>,
    /// Optional context/variables to pass to the code
    #[serde(default)]
    pub context: Option<Value>,
}

/// Output from code_execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionOutput {
    /// Standard output from the code execution
    pub stdout: String,
    /// Standard error output (if any)
    pub stderr: String,
    /// Return value from the code (if any)
    pub result: Option<Value>,
    /// Whether execution succeeded
    pub success: bool,
    /// Number of tool calls made during execution
    pub tool_calls_made: usize,
    /// Duration of execution in milliseconds
    pub duration_ms: u64,
}

impl Default for CodeExecutionOutput {
    fn default() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            success: false,
            tool_calls_made: 0,
            duration_ms: 0,
        }
    }
}

/// A tool call made from within Python code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InnerToolCall {
    /// The tool being called
    pub tool_name: String,
    /// Server ID for the tool
    pub server_id: String,
    /// Arguments passed to the tool
    pub arguments: Value,
    /// Parent execution ID
    pub parent_exec_id: String,
}

impl InnerToolCall {
    /// Convert to an ExtendedToolCall with caller information
    pub fn to_extended_call(&self, _call_id: &str) -> ExtendedToolCall {
        ExtendedToolCall {
            server: self.server_id.clone(),
            tool: self.tool_name.clone(),
            arguments: self.arguments.clone(),
            raw: format!(
                "{}({})  # from code_execution",
                self.tool_name,
                serde_json::to_string(&self.arguments).unwrap_or_default()
            ),
            kind: ToolCallKind::Normal,
            caller: Some(ToolCallCaller {
                caller_type: "code_execution_20250825".to_string(),
                tool_id: self.parent_exec_id.clone(),
            }),
        }
    }
}

/// Generate Python stub code for available tools
///
/// This creates Python function definitions that can be called from user code.
/// Each function is a wrapper that will trigger a callback to the Rust runtime.
pub fn generate_tool_stubs(tools: &[ToolSchema]) -> String {
    let mut stubs = String::new();
    
    stubs.push_str("# Auto-generated tool stubs for code_execution\n");
    stubs.push_str("# These functions call back to the Rust runtime via __host_call_tool__\n\n");
    stubs.push_str("import json\n\n");
    
    for tool in tools {
        // Skip built-in tools
        if tool.is_code_execution() || tool.is_tool_search() {
            continue;
        }
        
        // Check if this tool can be called from code_execution
        if !tool.can_be_called_by(Some("code_execution_20250825")) {
            continue;
        }
        
        // Generate function signature from parameters
        let params = extract_params_for_stub(&tool.parameters);
        let param_str = if params.is_empty() {
            String::new()
        } else {
            params.join(", ")
        };
        
        // Generate the function
        stubs.push_str(&format!("def {}({}):\n", tool.name, param_str));
        stubs.push_str(&format!("    \"\"\"{}\"\"\"", 
            tool.description.as_deref().unwrap_or(&tool.name)));
        stubs.push('\n');
        
        // Build kwargs dict
        if params.is_empty() {
            stubs.push_str("    kwargs = {}\n");
        } else {
            stubs.push_str("    kwargs = {\n");
            for param in &params {
                // Handle default value params
                let param_name = param.split('=').next().unwrap().trim();
                stubs.push_str(&format!("        \"{}\": {},\n", param_name, param_name));
            }
            stubs.push_str("    }\n");
        }
        
        stubs.push_str(&format!("    return __host_call_tool__(\"{}\", kwargs)\n\n", tool.name));
    }
    
    stubs
}

/// Extract parameter names from a JSON schema for stub generation
fn extract_params_for_stub(schema: &Value) -> Vec<String> {
    let mut params = Vec::new();
    
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        let required: Vec<&str> = schema.get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        
        // Add required params first (no default)
        for (name, _prop) in properties {
            if required.contains(&name.as_str()) {
                params.push(name.clone());
            }
        }
        
        // Add optional params with None default
        for (name, _prop) in properties {
            if !required.contains(&name.as_str()) {
                params.push(format!("{}=None", name));
            }
        }
    }
    
    params
}

/// Code execution context passed to the WASM runtime
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Unique ID for this execution
    pub exec_id: String,
    /// Tool stubs Python code
    pub tool_stubs: String,
    /// User context/variables
    pub user_context: Option<Value>,
    /// Available tools for inner calls
    pub available_tools: Vec<ToolSchema>,
}

/// Result of resolving an inner tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InnerCallResult {
    pub success: bool,
    pub result: Value,
    pub error: Option<String>,
}

/// The code execution executor
///
/// Note: Full WASM Python execution is implemented in python_actor.rs
/// This module provides the interface and helper functions.
pub struct CodeExecutionExecutor {
    /// Placeholder for WASM engine (implemented in python_actor)
    _placeholder: (),
}

impl CodeExecutionExecutor {
    pub fn new() -> Self {
        Self { _placeholder: () }
    }
    
    /// Validate code input before execution
    pub fn validate_input(input: &CodeExecutionInput) -> Result<(), String> {
        if input.code.is_empty() {
            return Err("Code cannot be empty".to_string());
        }
        
        // Check for obviously problematic patterns
        let code_str = input.code.join("\n");
        
        // These are soft checks - the WASM sandbox provides real security
        let blocked_patterns = [
            "import os",
            "import sys",
            "import subprocess",
            "__import__",
            "eval(",
            "exec(",
            "compile(",
        ];
        
        for pattern in blocked_patterns {
            if code_str.contains(pattern) {
                return Err(format!(
                    "Code contains blocked pattern: '{}'. \
                    Only safe operations are allowed in the sandbox.",
                    pattern
                ));
            }
        }
        
        Ok(())
    }
    
    /// Create an execution context for a code execution request
    pub fn create_context(
        exec_id: String,
        available_tools: Vec<ToolSchema>,
        user_context: Option<Value>,
    ) -> ExecutionContext {
        let tool_stubs = generate_tool_stubs(&available_tools);
        
        ExecutionContext {
            exec_id,
            tool_stubs,
            user_context,
            available_tools,
        }
    }
}

impl Default for CodeExecutionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    
    #[test]
    fn test_code_input_parsing() {
        let input: CodeExecutionInput = serde_json::from_value(json!({
            "code": ["x = 1", "y = 2", "print(x + y)"]
        })).unwrap();
        
        assert_eq!(input.code.len(), 3);
        assert!(input.context.is_none());
    }
    
    #[test]
    fn test_code_validation() {
        let good_input = CodeExecutionInput {
            code: vec!["x = 1".to_string(), "print(x)".to_string()],
            context: None,
        };
        assert!(CodeExecutionExecutor::validate_input(&good_input).is_ok());
        
        let bad_input = CodeExecutionInput {
            code: vec!["import os".to_string()],
            context: None,
        };
        assert!(CodeExecutionExecutor::validate_input(&bad_input).is_err());
    }
    
    #[test]
    fn test_generate_tool_stubs() {
        let tools = vec![
            ToolSchema {
                name: "get_weather".to_string(),
                description: Some("Get weather for a city".to_string()),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"},
                        "units": {"type": "string"}
                    },
                    "required": ["city"]
                }),
                tool_type: None,
                allowed_callers: Some(vec!["code_execution_20250825".to_string()]),
                defer_loading: false,
                embedding: None,
            },
        ];
        
        let stubs = generate_tool_stubs(&tools);
        
        assert!(stubs.contains("def get_weather("));
        assert!(stubs.contains("city"));
        assert!(stubs.contains("units=None"));
        assert!(stubs.contains("__host_call_tool__"));
    }
    
    #[test]
    fn test_inner_tool_call() {
        let call = InnerToolCall {
            tool_name: "get_weather".to_string(),
            server_id: "weather_server".to_string(),
            arguments: json!({"city": "Seattle"}),
            parent_exec_id: "exec_123".to_string(),
        };
        
        let extended = call.to_extended_call("call_456");
        
        assert_eq!(extended.server, "weather_server");
        assert_eq!(extended.tool, "get_weather");
        assert!(extended.caller.is_some());
        assert_eq!(extended.caller.unwrap().caller_type, "code_execution_20250825");
    }
}

