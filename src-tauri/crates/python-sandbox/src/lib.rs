//! Python Sandbox - Secure Python execution via RustPython
//!
//! This crate provides a sandboxed Python execution environment that:
//! - Uses RustPython for embedded Python execution
//! - Restricts dangerous builtins and imports
//! - Provides tool_call() for invoking external tools
//! - Captures stdout/stderr
//! - Designed to be compiled to WASM for double-sandbox security

pub mod protocol;
pub mod sandbox;

use protocol::{ExecutionRequest, ExecutionResult, ExecutionStatus};
use sandbox::{
    create_sandboxed_interpreter, reset_execution_state, set_available_tools,
    set_tool_results, get_pending_calls, get_stdout, get_stderr,
    json_to_pyobject, pyobject_to_json, SANDBOX_SETUP_CODE,
};
use rustpython_compiler::Mode;
use std::alloc::{alloc, dealloc, Layout};

/// Execute Python code in the sandbox
///
/// This is the main entry point for code execution.
/// It creates a fresh VM, sets up the sandbox, and executes the code.
pub fn execute(request: &ExecutionRequest) -> ExecutionResult {
    // Reset state for fresh execution
    reset_execution_state();
    
    // Set up available tools and any results from previous round
    set_available_tools(request.available_tools.clone());
    set_tool_results(request.tool_results.clone());
    
    // Create fresh sandboxed interpreter
    let interpreter = create_sandboxed_interpreter();
    
    // Enter the interpreter context
    interpreter.enter(|vm| {
        // Create a scope for execution
        let scope = vm.new_scope_with_builtins();
        
        // First, run sandbox setup code to configure restrictions
        let setup_code = match vm.compile(
            SANDBOX_SETUP_CODE,
            Mode::Exec,
            "<sandbox_setup>".to_string(),
        ) {
            Ok(code) => code,
            Err(e) => {
                let error_msg = format!("Sandbox setup compilation failed: {:?}", e);
                return ExecutionResult {
                    status: ExecutionStatus::Error(error_msg),
                    stderr: get_stderr(),
                    ..Default::default()
                };
            }
        };
        
        if let Err(e) = vm.run_code_obj(setup_code, scope.clone()) {
            let error_msg = format!("Sandbox setup failed: {:?}", e);
            return ExecutionResult {
                status: ExecutionStatus::Error(error_msg),
                stderr: get_stderr(),
                ..Default::default()
            };
        }
        
        // Join code lines and compile user code
        let code_str = request.code.join("\n");
        
        let user_code = match vm.compile(
            &code_str,
            Mode::Exec,
            "<code_execution>".to_string(),
        ) {
            Ok(code) => code,
            Err(e) => {
                let error_msg = format!("Compilation failed: {:?}", e);
                return ExecutionResult {
                    status: ExecutionStatus::Error(error_msg.clone()),
                    stderr: format!("{}\n{}", get_stderr(), error_msg),
                    ..Default::default()
                };
            }
        };
        
        // Inject context variables if provided
        if let Some(ctx) = &request.context {
            if let serde_json::Value::Object(map) = ctx {
                for (key, value) in map {
                    if let Ok(py_value) = json_to_pyobject(value, vm) {
                        let _ = scope.globals.set_item(key.as_str(), py_value, vm);
                    }
                }
            }
        }
        
        // Execute the user code
        let result = vm.run_code_obj(user_code, scope);
        
        // Check for pending tool calls
        let pending_calls = get_pending_calls();
        
        match result {
            Ok(py_result) => {
                let result_value = pyobject_to_json(&py_result, vm).ok();
                let num_pending = pending_calls.len();
                
                if !pending_calls.is_empty() {
                    ExecutionResult {
                        status: ExecutionStatus::ToolCallsPending,
                        stdout: get_stdout(),
                        stderr: get_stderr(),
                        result: result_value,
                        pending_calls,
                        tool_calls_made: num_pending,
                    }
                } else {
                    ExecutionResult {
                        status: ExecutionStatus::Complete,
                        stdout: get_stdout(),
                        stderr: get_stderr(),
                        result: result_value,
                        pending_calls: Vec::new(),
                        tool_calls_made: 0,
                    }
                }
            }
            Err(exc) => {
                let exc_str = format!("{:?}", exc);
                let num_pending = pending_calls.len();
                
                if exc_str.contains("ToolCallPending:") || !pending_calls.is_empty() {
                    ExecutionResult {
                        status: ExecutionStatus::ToolCallsPending,
                        stdout: get_stdout(),
                        stderr: get_stderr(),
                        result: None,
                        pending_calls,
                        tool_calls_made: num_pending,
                    }
                } else {
                    let error_msg = format!("{:?}", exc);
                    ExecutionResult {
                        status: ExecutionStatus::Error(error_msg.clone()),
                        stdout: get_stdout(),
                        stderr: format!("{}\n{}", get_stderr(), error_msg),
                        result: None,
                        pending_calls: Vec::new(),
                        tool_calls_made: 0,
                    }
                }
            }
        }
    })
}

// ============ WASM Exports ============

/// Allocate memory for the host to write into
#[no_mangle]
pub extern "C" fn alloc_memory(size: usize) -> *mut u8 {
    if size == 0 {
        return std::ptr::null_mut();
    }
    
    let layout = Layout::from_size_align(size, 1).unwrap();
    unsafe { alloc(layout) }
}

/// Free memory allocated by alloc_memory
#[no_mangle]
pub extern "C" fn free_memory(ptr: *mut u8, size: usize) {
    if ptr.is_null() || size == 0 {
        return;
    }
    
    let layout = Layout::from_size_align(size, 1).unwrap();
    unsafe { dealloc(ptr, layout) }
}

/// Execute Python code
///
/// # Arguments
/// * `request_ptr` - Pointer to JSON-encoded ExecutionRequest
/// * `request_len` - Length of the request data
///
/// # Returns
/// Pointer to JSON-encoded ExecutionResult (caller must free with free_memory)
/// The first 4 bytes contain the length of the result as a little-endian u32
#[no_mangle]
pub extern "C" fn execute_python(request_ptr: *const u8, request_len: usize) -> *mut u8 {
    let request_bytes = unsafe {
        std::slice::from_raw_parts(request_ptr, request_len)
    };
    
    let request: ExecutionRequest = match serde_json::from_slice(request_bytes) {
        Ok(r) => r,
        Err(e) => {
            return encode_result(&ExecutionResult::error(format!("Invalid request: {}", e)));
        }
    };
    
    let result = execute(&request);
    encode_result(&result)
}

/// Encode an ExecutionResult to memory, prefixed with length
fn encode_result(result: &ExecutionResult) -> *mut u8 {
    let json = serde_json::to_vec(result).unwrap_or_else(|_| {
        serde_json::to_vec(&ExecutionResult::error("Failed to serialize result")).unwrap()
    });
    
    let total_len = 4 + json.len();
    let ptr = alloc_memory(total_len);
    
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    
    unsafe {
        let len_bytes = (json.len() as u32).to_le_bytes();
        std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), ptr, 4);
        std::ptr::copy_nonoverlapping(json.as_ptr(), ptr.add(4), json.len());
    }
    
    ptr
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ToolInfo;
    use std::collections::HashMap;
    
    #[test]
    fn test_simple_execution() {
        let request = ExecutionRequest {
            code: vec![
                "x = 1 + 2".to_string(),
                "print(x)".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.stdout.contains("3"));
    }
    
    #[test]
    fn test_blocked_import() {
        let request = ExecutionRequest {
            code: vec!["import os".to_string()],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        // Import should fail (either our restriction or missing module)
        // We just verify it doesn't complete successfully
        match result.status {
            ExecutionStatus::Complete => {
                // If it completes, make sure 'os' isn't actually usable
                // This would indicate a security issue
                panic!("Import 'os' should not be allowed to succeed");
            }
            _ => {
                // Any error is acceptable - the import is blocked
            }
        }
    }
    
    #[test]
    fn test_basic_math() {
        // Test basic Python math without imports
        let request = ExecutionRequest {
            code: vec![
                "x = 2 + 3".to_string(),
                "y = x * 4".to_string(),
                "print(y)".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.stdout.contains("20"));
    }
    
    #[test]
    fn test_tool_call_pending() {
        let request = ExecutionRequest {
            code: vec![
                "result = tool_call('get_weather', city='Seattle')".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![ToolInfo {
                name: "get_weather".to_string(),
                server_id: "weather_server".to_string(),
                description: Some("Get weather".to_string()),
                parameters: serde_json::json!({}),
            }],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::ToolCallsPending);
        assert_eq!(result.pending_calls.len(), 1);
        assert_eq!(result.pending_calls[0].tool_name, "get_weather");
    }
    
    // ===== Security Tests =====
    
    #[test]
    fn test_no_file_access() {
        // Attempt to read a file should fail
        let request = ExecutionRequest {
            code: vec![
                "f = open('/etc/passwd', 'r')".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        // Should fail - open is blocked
        match result.status {
            ExecutionStatus::Complete => panic!("File access should be blocked"),
            _ => {} // Any error is acceptable
        }
    }
    
    #[test]
    fn test_no_subprocess() {
        // Attempt to import subprocess should fail
        let request = ExecutionRequest {
            code: vec![
                "import subprocess".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        // Should fail - subprocess is blocked
        match result.status {
            ExecutionStatus::Complete => panic!("subprocess import should be blocked"),
            _ => {} // Any error is acceptable
        }
    }
    
    #[test]
    fn test_no_eval() {
        // Attempt to use eval should fail
        let request = ExecutionRequest {
            code: vec![
                "eval('1 + 1')".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        // Should fail - eval is blocked
        match result.status {
            ExecutionStatus::Complete => panic!("eval should be blocked"),
            _ => {} // Any error is acceptable
        }
    }
    
    #[test]
    fn test_data_structures() {
        // Test that basic Python data structures work
        let request = ExecutionRequest {
            code: vec![
                "data = {'name': 'test', 'numbers': [1, 2, 3]}".to_string(),
                "data['numbers'].append(4)".to_string(),
                "print(len(data['numbers']))".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.stdout.contains("4"));
    }
    
    #[test]
    fn test_string_operations() {
        // Test string manipulation
        let request = ExecutionRequest {
            code: vec![
                "text = 'hello world'".to_string(),
                "print(text.upper())".to_string(),
                "print(text.split())".to_string(),
            ],
            context: None,
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.stdout.contains("HELLO WORLD"));
    }
    
    #[test]
    fn test_context_injection() {
        // Test that context variables are properly injected
        let request = ExecutionRequest {
            code: vec![
                "print(x + y)".to_string(),
            ],
            context: Some(serde_json::json!({
                "x": 10,
                "y": 20
            })),
            tool_results: HashMap::new(),
            available_tools: vec![],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.stdout.contains("30"));
    }
    
    #[test]
    fn test_tool_call_with_result() {
        // Test tool call with pre-existing result (continuation scenario)
        let mut tool_results = HashMap::new();
        tool_results.insert("get_time".to_string(), protocol::ToolCallResult {
            success: true,
            result: serde_json::json!("2024-01-15T10:30:00Z"),
            error: None,
        });
        
        let request = ExecutionRequest {
            code: vec![
                "time = tool_call('get_time')".to_string(),
                "print(time)".to_string(),
            ],
            context: None,
            tool_results,
            available_tools: vec![ToolInfo {
                name: "get_time".to_string(),
                server_id: "time_server".to_string(),
                description: Some("Get current time".to_string()),
                parameters: serde_json::json!({}),
            }],
        };
        
        let result = execute(&request);
        
        assert_eq!(result.status, ExecutionStatus::Complete);
        assert!(result.stdout.contains("2024-01-15"));
    }
}
