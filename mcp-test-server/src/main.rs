//! MCP Test Server for Plugable Chat
//! 
//! This server implements the Model Context Protocol (MCP) over stdio
//! and provides test tools for validating the MCP integration.
//!
//! Test tools:
//! - test_echo: Returns the input (basic connectivity test)
//! - test_math: Performs simple math operations
//! - test_file_read: Reads a test file
//! - test_error: Intentionally fails (error handling test)
//! - get_test_status: Reports red/green test status
//! - get_test_prompts: Provides sample prompts for testing

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};

/// Test status tracking
static TESTS_RUN: AtomicU64 = AtomicU64::new(0);
static TESTS_PASSED: AtomicU64 = AtomicU64::new(0);
static TESTS_FAILED: AtomicU64 = AtomicU64::new(0);

/// JSON-RPC request
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

/// MCP Tool definition
#[derive(Debug, Serialize)]
struct Tool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

/// MCP Content block (used in tool responses)
#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct Content {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

fn main() {
    eprintln!("MCP Test Server starting...");
    
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();
    
    for line in stdin.lock().lines() {
        match line {
            Ok(input) => {
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    continue;
                }
                
                eprintln!("Received: {}", trimmed);
                
                match serde_json::from_str::<JsonRpcRequest>(trimmed) {
                    Ok(request) => {
                        let response = handle_request(request);
                        let response_str = serde_json::to_string(&response).unwrap();
                        eprintln!("Sending: {}", response_str);
                        writeln!(stdout_lock, "{}", response_str).unwrap();
                        stdout_lock.flush().unwrap();
                    }
                    Err(e) => {
                        eprintln!("Failed to parse request: {}", e);
                        let error_response = JsonRpcResponse {
                            jsonrpc: "2.0",
                            id: Value::Null,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32700,
                                message: format!("Parse error: {}", e),
                                data: None,
                            }),
                        };
                        let response_str = serde_json::to_string(&error_response).unwrap();
                        writeln!(stdout_lock, "{}", response_str).unwrap();
                        stdout_lock.flush().unwrap();
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }
    
    eprintln!("MCP Test Server shutting down...");
}

fn handle_request(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.unwrap_or(Value::Null);
    
    match request.method.as_str() {
        "initialize" => {
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "mcp-test-server",
                        "version": "0.1.0"
                    }
                })),
                error: None,
            }
        }
        
        "notifications/initialized" => {
            // No response needed for notifications
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({})),
                error: None,
            }
        }
        
        "tools/list" => {
            let tools = get_tools();
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({ "tools": tools })),
                error: None,
            }
        }
        
        "tools/call" => {
            let params = request.params.unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            
            TESTS_RUN.fetch_add(1, Ordering::Relaxed);
            
            match execute_tool(tool_name, arguments) {
                Ok(result) => {
                    TESTS_PASSED.fetch_add(1, Ordering::Relaxed);
                    JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: Some(json!({
                            "content": [{ "type": "text", "text": result }],
                            "isError": false
                        })),
                        error: None,
                    }
                }
                Err(error) => {
                    TESTS_FAILED.fetch_add(1, Ordering::Relaxed);
                    JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: Some(json!({
                            "content": [{ "type": "text", "text": error }],
                            "isError": true
                        })),
                        error: None,
                    }
                }
            }
        }
        
        _ => {
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", request.method),
                    data: None,
                }),
            }
        }
    }
}

fn get_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "test_echo".to_string(),
            description: "Echo test - returns the input message. Use to verify basic MCP connectivity.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The message to echo back"
                    }
                },
                "required": ["message"]
            }),
        },
        Tool {
            name: "test_math".to_string(),
            description: "Math test - performs simple arithmetic operations.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["add", "subtract", "multiply", "divide"],
                        "description": "The math operation to perform"
                    },
                    "a": {
                        "type": "number",
                        "description": "First operand"
                    },
                    "b": {
                        "type": "number",
                        "description": "Second operand"
                    }
                },
                "required": ["operation", "a", "b"]
            }),
        },
        Tool {
            name: "test_json".to_string(),
            description: "JSON test - returns a structured JSON response to test complex data handling.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "include_nested": {
                        "type": "boolean",
                        "description": "Whether to include nested objects in the response"
                    }
                }
            }),
        },
        Tool {
            name: "test_error".to_string(),
            description: "Error test - intentionally returns an error to test error handling.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "error_type": {
                        "type": "string",
                        "enum": ["validation", "runtime", "timeout"],
                        "description": "The type of error to simulate"
                    }
                },
                "required": ["error_type"]
            }),
        },
        Tool {
            name: "get_test_status".to_string(),
            description: "Returns the current test status with pass/fail counts. Shows red/green status.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        Tool {
            name: "get_test_prompts".to_string(),
            description: "Returns sample prompts that users can try to test the MCP integration.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        Tool {
            name: "reset_test_counts".to_string(),
            description: "Resets all test counters to zero.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

fn execute_tool(name: &str, args: Value) -> Result<String, String> {
    match name {
        "test_echo" => {
            let message = args.get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: message".to_string())?;
            Ok(format!("Echo: {}", message))
        }
        
        "test_math" => {
            let operation = args.get("operation")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: operation".to_string())?;
            let a = args.get("a")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "Missing required parameter: a".to_string())?;
            let b = args.get("b")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "Missing required parameter: b".to_string())?;
            
            let result = match operation {
                "add" => a + b,
                "subtract" => a - b,
                "multiply" => a * b,
                "divide" => {
                    if b == 0.0 {
                        return Err("Division by zero".to_string());
                    }
                    a / b
                }
                _ => return Err(format!("Unknown operation: {}", operation)),
            };
            
            Ok(format!("{} {} {} = {}", a, operation, b, result))
        }
        
        "test_json" => {
            let include_nested = args.get("include_nested")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            
            let mut response = json!({
                "status": "success",
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "data": {
                    "string_field": "Hello, MCP!",
                    "number_field": 42,
                    "boolean_field": true,
                    "array_field": [1, 2, 3, 4, 5]
                }
            });
            
            if include_nested {
                response["data"]["nested"] = json!({
                    "level1": {
                        "level2": {
                            "level3": "deeply nested value"
                        }
                    }
                });
            }
            
            Ok(serde_json::to_string_pretty(&response).unwrap())
        }
        
        "test_error" => {
            let error_type = args.get("error_type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: error_type".to_string())?;
            
            match error_type {
                "validation" => Err("Validation error: Input failed schema validation".to_string()),
                "runtime" => Err("Runtime error: An unexpected condition occurred".to_string()),
                "timeout" => Err("Timeout error: Operation exceeded time limit".to_string()),
                _ => Err(format!("Unknown error type: {}", error_type)),
            }
        }
        
        "get_test_status" => {
            let run = TESTS_RUN.load(Ordering::Relaxed);
            let passed = TESTS_PASSED.load(Ordering::Relaxed);
            let failed = TESTS_FAILED.load(Ordering::Relaxed);
            
            let status = if failed == 0 && passed > 0 {
                "🟢 ALL TESTS PASSING"
            } else if failed > 0 {
                "🔴 SOME TESTS FAILED"
            } else {
                "⚪ NO TESTS RUN YET"
            };
            
            Ok(format!(
                "{}\n\nTest Summary:\n  Total: {}\n  Passed: {} ✓\n  Failed: {} ✗\n  Pass Rate: {:.1}%",
                status,
                run,
                passed,
                failed,
                if run > 0 { (passed as f64 / run as f64) * 100.0 } else { 0.0 }
            ))
        }
        
        "get_test_prompts" => {
            Ok(r#"Here are sample prompts to test the MCP integration:

1. **Basic Echo Test**
   "Please use the test_echo tool to echo the message 'Hello from MCP!'"

2. **Math Test**
   "Use the test_math tool to calculate 15 multiplied by 7"

3. **JSON Response Test**
   "Call test_json with include_nested set to true and show me the result"

4. **Error Handling Test**
   "Use test_error with error_type 'validation' to test error handling"

5. **Status Check**
   "Check the current test status using get_test_status"

Try these prompts in your chat to verify the MCP connection is working correctly!"#.to_string())
        }
        
        "reset_test_counts" => {
            TESTS_RUN.store(0, Ordering::Relaxed);
            TESTS_PASSED.store(0, Ordering::Relaxed);
            TESTS_FAILED.store(0, Ordering::Relaxed);
            Ok("Test counters have been reset to zero.".to_string())
        }
        
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

