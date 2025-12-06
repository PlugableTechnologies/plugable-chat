//! Python Execution Implementation
//!
//! Python code execution in a WASM sandbox.
//! This tool allows models to run Python code that can call other registered tools.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{ToolSchema, ExtendedToolCall, ToolCallCaller, ToolCallKind};

/// Input for the python_execution built-in tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionInput {
    /// Lines of Python code to execute
    pub code: Vec<String>,
    /// Optional context/variables to pass to the code
    #[serde(default)]
    pub context: Option<Value>,
}

/// Output from python_execution
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
                "{}({})  # from python_execution",
                self.tool_name,
                serde_json::to_string(&self.arguments).unwrap_or_default()
            ),
            kind: ToolCallKind::Normal,
            caller: Some(ToolCallCaller {
                caller_type: "python_execution_20251206".to_string(),
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
    
    stubs.push_str("# Auto-generated tool stubs for python_execution\n");
    stubs.push_str("# These functions call back to the Rust runtime via __host_call_tool__\n\n");
    stubs.push_str("import json\n\n");
    
    for tool in tools {
        // Skip built-in tools
        if tool.is_python_execution() || tool.is_tool_search() {
            continue;
        }
        
        // Check if this tool can be called from python_execution
        if !tool.can_be_called_by(Some("python_execution_20251206")) {
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

/// Modules allowed in the Python sandbox (matches python_sandbox::ALLOWED_MODULES)
pub const ALLOWED_MODULES: &[&str] = &[
    "math", "json", "random", "re", "datetime", "collections",
    "itertools", "functools", "operator", "string", "textwrap",
    "copy", "types", "typing", "abc", "numbers", "decimal",
    "fractions", "statistics", "hashlib", "base64", "binascii",
    "html",
];

/// Context for dynamic import validation
/// 
/// Used when tool_search has materialized tool modules that become
/// importable in the sandbox.
#[derive(Debug, Clone, Default)]
pub struct DynamicImportContext {
    /// Tool modules that are available for import (python_name -> server_id)
    pub tool_modules: std::collections::HashMap<String, String>,
}

impl DynamicImportContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self {
            tool_modules: std::collections::HashMap::new(),
        }
    }
    
    /// Add a tool module
    pub fn add_tool_module(&mut self, python_name: String, server_id: String) {
        self.tool_modules.insert(python_name, server_id);
    }
    
    /// Check if a module name is a known tool module
    pub fn is_tool_module(&self, module_name: &str) -> bool {
        self.tool_modules.contains_key(module_name)
    }
    
    /// Get all tool module names
    pub fn get_tool_modules(&self) -> Vec<&String> {
        self.tool_modules.keys().collect()
    }
}

impl CodeExecutionExecutor {
    pub fn new() -> Self {
        Self { _placeholder: () }
    }
    
    /// Validate code input before execution (basic validation without tool modules)
    pub fn validate_input(input: &CodeExecutionInput) -> Result<(), String> {
        Self::validate_input_with_context(input, None)
    }
    
    /// Validate code input with dynamic import context
    /// 
    /// This allows tool modules discovered via tool_search to be imported.
    pub fn validate_input_with_context(
        input: &CodeExecutionInput,
        context: Option<&DynamicImportContext>,
    ) -> Result<(), String> {
        if input.code.is_empty() {
            return Err("Code cannot be empty".to_string());
        }
        
        // Check for obviously problematic patterns
        let code_str = input.code.join("\n");
        
        // These are soft checks - the sandbox provides real security
        let blocked_patterns = [
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
        
        // Check for imports of disallowed modules and provide helpful error message
        if let Some(err) = Self::check_imports_with_context(&code_str, context) {
            return Err(err);
        }
        
        Ok(())
    }
    
    /// Check imports with dynamic import context
    fn check_imports_with_context(code: &str, context: Option<&DynamicImportContext>) -> Option<String> {
        use regex::Regex;
        
        // Match various import patterns:
        // - import foo
        // - import foo, bar
        // - import foo as f
        // - from foo import bar
        // - from foo.bar import baz
        let import_re = Regex::new(r"(?m)^\s*(?:import\s+([a-zA-Z_][a-zA-Z0-9_]*(?:\s*,\s*[a-zA-Z_][a-zA-Z0-9_]*)*)|from\s+([a-zA-Z_][a-zA-Z0-9_]*)(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*\s+import)").ok()?;
        
        let mut disallowed = Vec::new();
        
        for cap in import_re.captures_iter(code) {
            // Check "import x" style
            if let Some(modules) = cap.get(1) {
                for module in modules.as_str().split(',') {
                    let module = module.split_whitespace().next().unwrap_or("").trim();
                    if !module.is_empty() && !Self::is_module_allowed(module, context) && module != "builtins" {
                        disallowed.push(module.to_string());
                    }
                }
            }
            // Check "from x import" style
            if let Some(module) = cap.get(2) {
                let module = module.as_str().trim();
                if !Self::is_module_allowed(module, context) && module != "builtins" {
                    disallowed.push(module.to_string());
                }
            }
        }
        
        if !disallowed.is_empty() {
            disallowed.sort();
            disallowed.dedup();
            
            let mut allowed_list = ALLOWED_MODULES.join(", ");
            
            // Include tool modules in the allowed list if any
            if let Some(ctx) = context {
                if !ctx.tool_modules.is_empty() {
                    let tool_modules: Vec<&String> = ctx.get_tool_modules();
                    let tool_list: Vec<&str> = tool_modules.iter().map(|s| s.as_str()).collect();
                    allowed_list = format!("{}, tool modules: {}", allowed_list, tool_list.join(", "));
                }
            }
            
            return Some(format!(
                "Cannot import '{}' - not available in the sandbox. \
                The sandbox provides a restricted Python environment for safe code execution. \
                Allowed modules: {}. \
                For data analysis, use the built-in math, statistics, collections, and itertools modules instead of pandas/numpy.",
                disallowed.join("', '"),
                allowed_list
            ));
        }
        
        None
    }
    
    /// Check if a module is allowed (either built-in or a tool module)
    fn is_module_allowed(module: &str, context: Option<&DynamicImportContext>) -> bool {
        // Check built-in modules
        if ALLOWED_MODULES.contains(&module) {
            return true;
        }
        
        // Check tool modules from context
        if let Some(ctx) = context {
            if ctx.is_tool_module(module) {
                return true;
            }
        }
        
        false
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
        let err = CodeExecutionExecutor::validate_input(&bad_input).unwrap_err();
        assert!(err.contains("Cannot import 'os'"));
        assert!(err.contains("Allowed modules:"));
    }
    
    #[test]
    fn test_blocked_imports_with_helpful_message() {
        // Test pandas
        let pandas_input = CodeExecutionInput {
            code: vec!["import pandas".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&pandas_input).unwrap_err();
        assert!(err.contains("Cannot import 'pandas'"), "Error should mention pandas: {}", err);
        assert!(err.contains("Allowed modules:"), "Error should list allowed modules: {}", err);
        assert!(err.contains("math"), "Error should list math as allowed: {}", err);
        
        // Test numpy
        let numpy_input = CodeExecutionInput {
            code: vec!["import numpy as np".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&numpy_input).unwrap_err();
        assert!(err.contains("Cannot import 'numpy'"), "Error should mention numpy: {}", err);
        
        // Test from import
        let from_input = CodeExecutionInput {
            code: vec!["from pandas import DataFrame".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&from_input).unwrap_err();
        assert!(err.contains("Cannot import 'pandas'"), "Error should mention pandas: {}", err);
    }
    
    #[test]
    fn test_allowed_imports() {
        // These should all be allowed
        let allowed_imports = vec![
            "import math",
            "import json",
            "from datetime import datetime",
            "import collections",
            "from itertools import chain",
            "import statistics",
        ];
        
        for import_stmt in allowed_imports {
            let input = CodeExecutionInput {
                code: vec![import_stmt.to_string()],
                context: None,
            };
            assert!(
                CodeExecutionExecutor::validate_input(&input).is_ok(),
                "'{}' should be allowed", import_stmt
            );
        }
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
                allowed_callers: Some(vec!["python_execution_20251206".to_string()]),
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
        assert_eq!(extended.caller.unwrap().caller_type, "python_execution_20251206");
    }
    
    // ============ Additional Input Validation Tests ============
    
    #[test]
    fn test_empty_code_rejected() {
        let input = CodeExecutionInput {
            code: vec![],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("empty") || err.contains("Empty"),
            "Error should mention empty code: {}", err);
    }
    
    #[test]
    fn test_eval_pattern_rejected() {
        let input = CodeExecutionInput {
            code: vec!["result = eval('1 + 1')".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("eval("),
            "Error should mention eval: {}", err);
    }
    
    #[test]
    fn test_exec_pattern_rejected() {
        let input = CodeExecutionInput {
            code: vec!["exec('x = 1')".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("exec("),
            "Error should mention exec: {}", err);
    }
    
    #[test]
    fn test_compile_pattern_rejected() {
        let input = CodeExecutionInput {
            code: vec!["code = compile('x = 1', '', 'exec')".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("compile("),
            "Error should mention compile: {}", err);
    }
    
    #[test]
    fn test_dunder_import_pattern_rejected() {
        let input = CodeExecutionInput {
            code: vec!["os = __import__('os')".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("__import__"),
            "Error should mention __import__: {}", err);
    }
    
    #[test]
    fn test_multiple_blocked_imports() {
        // Multiple disallowed imports in one code block
        let input = CodeExecutionInput {
            code: vec![
                "import os".to_string(),
                "import sys".to_string(),
                "import subprocess".to_string(),
            ],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        // Should mention at least one of them
        assert!(err.contains("os") || err.contains("sys") || err.contains("subprocess"),
            "Error should mention blocked imports: {}", err);
    }
    
    #[test]
    fn test_code_with_context() {
        let input: CodeExecutionInput = serde_json::from_value(json!({
            "code": ["print(x + y)"],
            "context": {"x": 10, "y": 20}
        })).unwrap();
        
        assert_eq!(input.code.len(), 1);
        assert!(input.context.is_some());
        let ctx = input.context.unwrap();
        assert_eq!(ctx["x"], 10);
        assert_eq!(ctx["y"], 20);
    }
    
    #[test]
    fn test_allowed_import_with_alias() {
        let input = CodeExecutionInput {
            code: vec!["import math as m".to_string()],
            context: None,
        };
        assert!(CodeExecutionExecutor::validate_input(&input).is_ok(),
            "import math as m should be allowed");
    }
    
    #[test]
    fn test_allowed_multiple_imports() {
        let input = CodeExecutionInput {
            code: vec!["import math, json, random".to_string()],
            context: None,
        };
        // Note: This tests comma-separated imports
        // Our regex may or may not support this - let's verify behavior
        let result = CodeExecutionExecutor::validate_input(&input);
        // All three are allowed, so it should pass
        assert!(result.is_ok(), 
            "Multiple allowed imports should pass: {:?}", result);
    }
    
    #[test]
    fn test_blocked_import_in_middle_of_code() {
        let input = CodeExecutionInput {
            code: vec![
                "x = 1".to_string(),
                "y = 2".to_string(),
                "import os".to_string(),  // Blocked import in the middle
                "z = x + y".to_string(),
            ],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("os") || err.contains("Cannot import"),
            "Should detect blocked import in middle of code: {}", err);
    }
    
    #[test]
    fn test_from_submodule_import() {
        // from datetime.datetime should work (datetime is allowed)
        let input = CodeExecutionInput {
            code: vec!["from datetime import datetime, timedelta".to_string()],
            context: None,
        };
        assert!(CodeExecutionExecutor::validate_input(&input).is_ok(),
            "from datetime import should be allowed");
    }
    
    #[test]
    fn test_blocked_from_submodule_import() {
        // from os.path import join should be blocked (os is not allowed)
        let input = CodeExecutionInput {
            code: vec!["from os.path import join".to_string()],
            context: None,
        };
        let err = CodeExecutionExecutor::validate_input(&input).unwrap_err();
        assert!(err.contains("os") || err.contains("Cannot import"),
            "from os.path import should be blocked: {}", err);
    }
    
    #[test]
    fn test_all_allowed_modules() {
        // Test that all explicitly allowed modules pass validation
        let allowed = vec![
            "import math",
            "import json",
            "import random",
            "import re",
            "import datetime",
            "import collections",
            "import itertools",
            "import functools",
            "import operator",
            "import string",
            "import textwrap",
            "import copy",
            "import types",
            "import typing",
            "import abc",
            "import numbers",
            "import decimal",
            "import fractions",
            "import statistics",
            "import hashlib",
            "import base64",
            "import binascii",
            "import html",
        ];
        
        for import_stmt in allowed {
            let input = CodeExecutionInput {
                code: vec![import_stmt.to_string()],
                context: None,
            };
            assert!(
                CodeExecutionExecutor::validate_input(&input).is_ok(),
                "'{}' should be allowed", import_stmt
            );
        }
    }
    
    #[test]
    fn test_safe_code_patterns() {
        // Various safe code patterns should pass validation
        let safe_patterns = vec![
            vec!["x = 1", "y = 2", "print(x + y)"],
            vec!["def foo(): return 42", "print(foo())"],
            vec!["class Foo: pass", "f = Foo()"],
            vec!["data = [1, 2, 3]", "print(sum(data))"],
            vec!["d = {'a': 1}", "print(d['a'])"],
            vec!["import math", "print(math.pi)"],
            vec!["from collections import Counter", "c = Counter('abcd')"],
        ];
        
        for pattern in safe_patterns {
            let input = CodeExecutionInput {
                code: pattern.iter().map(|s| s.to_string()).collect(),
                context: None,
            };
            assert!(
                CodeExecutionExecutor::validate_input(&input).is_ok(),
                "Pattern {:?} should be allowed", pattern
            );
        }
    }
    
    #[test]
    fn test_output_types() {
        // Verify CodeExecutionOutput serialization
        let output = CodeExecutionOutput {
            stdout: "Hello, world!".to_string(),
            stderr: String::new(),
            result: Some(json!(42)),
            success: true,
            tool_calls_made: 0,
            duration_ms: 100,
        };
        
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["stdout"], "Hello, world!");
        assert_eq!(json["success"], true);
        assert_eq!(json["result"], 42);
        assert_eq!(json["duration_ms"], 100);
    }
    
    #[test]
    fn test_code_execution_output_default() {
        let default = CodeExecutionOutput::default();
        assert_eq!(default.stdout, "");
        assert_eq!(default.stderr, "");
        assert!(default.result.is_none());
        assert!(!default.success);
        assert_eq!(default.tool_calls_made, 0);
        assert_eq!(default.duration_ms, 0);
    }
    
    // ============ Dynamic Import Context Tests ============
    
    #[test]
    fn test_dynamic_import_context_creation() {
        let mut ctx = DynamicImportContext::new();
        ctx.add_tool_module("weather".to_string(), "mcp-weather".to_string());
        ctx.add_tool_module("database".to_string(), "mcp-db".to_string());
        
        assert!(ctx.is_tool_module("weather"));
        assert!(ctx.is_tool_module("database"));
        assert!(!ctx.is_tool_module("unknown"));
        assert_eq!(ctx.get_tool_modules().len(), 2);
    }
    
    #[test]
    fn test_validate_with_tool_modules() {
        let mut ctx = DynamicImportContext::new();
        ctx.add_tool_module("weather_api".to_string(), "mcp-weather".to_string());
        
        // Should allow import of tool module
        let input = CodeExecutionInput {
            code: vec!["from weather_api import get_forecast".to_string()],
            context: None,
        };
        assert!(
            CodeExecutionExecutor::validate_input_with_context(&input, Some(&ctx)).is_ok(),
            "Tool module import should be allowed with context"
        );
        
        // Should still block unknown modules
        let input2 = CodeExecutionInput {
            code: vec!["import unknown_module".to_string()],
            context: None,
        };
        assert!(
            CodeExecutionExecutor::validate_input_with_context(&input2, Some(&ctx)).is_err(),
            "Unknown module should still be blocked"
        );
    }
    
    #[test]
    fn test_tool_module_import_without_context() {
        // Without context, tool modules are blocked
        let input = CodeExecutionInput {
            code: vec!["from weather_api import get_forecast".to_string()],
            context: None,
        };
        assert!(
            CodeExecutionExecutor::validate_input(&input).is_err(),
            "Tool module should be blocked without context"
        );
    }
    
    #[test]
    fn test_mixed_imports_with_context() {
        let mut ctx = DynamicImportContext::new();
        ctx.add_tool_module("my_tools".to_string(), "mcp-tools".to_string());
        
        // Should allow mixing standard modules and tool modules
        let input = CodeExecutionInput {
            code: vec![
                "import math".to_string(),
                "from my_tools import do_something".to_string(),
                "from collections import Counter".to_string(),
            ],
            context: None,
        };
        assert!(
            CodeExecutionExecutor::validate_input_with_context(&input, Some(&ctx)).is_ok(),
            "Mixed standard and tool module imports should be allowed"
        );
    }
}

