//! Python Actor - Sandboxed Python code execution
//!
//! This actor manages Python code execution in a secure sandbox using RustPython.
//! It provides:
//! - Isolated Python execution with restricted builtins/imports
//! - Tool calls via the tool_call() Python function that pauses execution
//! - Batch tool call model: execution pauses on tool_call(), host executes, resumes
//! - Memory and output size limits for security
//!
//! The architecture uses a double-sandbox model:
//! - Inner: RustPython with restricted Python environment
//! - Outer: (Optional) WASM sandbox via Wasmtime for additional isolation

use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::code_execution::{
    CodeExecutionInput, CodeExecutionOutput, ExecutionContext, InnerToolCall, InnerCallResult,
};
use crate::tool_registry::SharedToolRegistry;

// Import the python-sandbox crate
use python_sandbox::protocol::{
    ExecutionRequest, ExecutionStatus, ToolInfo, ToolCallResult,
};

/// Maximum output size (in bytes)
const MAX_OUTPUT_SIZE: usize = 1024 * 1024; // 1MB

/// Maximum number of tool call rounds to prevent infinite loops
const MAX_TOOL_CALL_ROUNDS: usize = 10;

/// Message types for the Python actor
pub enum PythonMsg {
    /// Execute Python code
    Execute {
        input: CodeExecutionInput,
        context: ExecutionContext,
        respond_to: oneshot::Sender<Result<CodeExecutionOutput, String>>,
    },
    /// Handle an inner tool call from executing Python code
    InnerToolCall {
        call: InnerToolCall,
        respond_to: oneshot::Sender<InnerCallResult>,
    },
    /// Check if the Python runtime is available
    HealthCheck {
        respond_to: oneshot::Sender<bool>,
    },
}

/// Event emitted when Python code makes a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonToolCallEvent {
    pub exec_id: String,
    pub tool_name: String,
    pub server_id: String,
    pub arguments: Value,
}

/// Result of a Python tool call to be injected back
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonToolCallResult {
    pub success: bool,
    pub result: Value,
    pub error: Option<String>,
}

/// The Python actor that manages code execution
pub struct PythonActor {
    rx: mpsc::Receiver<PythonMsg>,
    tool_registry: SharedToolRegistry,
    /// Channel to send tool calls to the orchestrator for execution
    tool_call_tx: mpsc::Sender<(InnerToolCall, oneshot::Sender<InnerCallResult>)>,
    tool_call_rx: mpsc::Receiver<(InnerToolCall, oneshot::Sender<InnerCallResult>)>,
}

impl PythonActor {
    pub fn new(rx: mpsc::Receiver<PythonMsg>, tool_registry: SharedToolRegistry) -> Self {
        let (tool_call_tx, tool_call_rx) = mpsc::channel(32);
        
        Self {
            rx,
            tool_registry,
            tool_call_tx,
            tool_call_rx,
        }
    }
    
    pub async fn run(mut self) {
        println!("[PythonActor] Starting with RustPython sandbox...");
        
        loop {
            tokio::select! {
                msg = self.rx.recv() => {
                    match msg {
                        Some(PythonMsg::Execute { input, context, respond_to }) => {
                            let result = self.execute_code(input, context).await;
                            let _ = respond_to.send(result);
                        }
                        Some(PythonMsg::InnerToolCall { call, respond_to }) => {
                            // Forward to the orchestrator for execution
                            let _ = self.tool_call_tx.send((call, respond_to)).await;
                        }
                        Some(PythonMsg::HealthCheck { respond_to }) => {
                            // RustPython sandbox is always available
                            let _ = respond_to.send(true);
                        }
                        None => {
                            println!("[PythonActor] Channel closed, shutting down");
                            break;
                        }
                    }
                }
            }
        }
        
        println!("[PythonActor] Shutdown complete");
    }
    
    /// Get the channel for receiving tool calls that need execution
    pub fn get_tool_call_receiver(&mut self) -> mpsc::Receiver<(InnerToolCall, oneshot::Sender<InnerCallResult>)> {
        // Note: This takes ownership, so it can only be called once
        let (new_tx, new_rx) = mpsc::channel(32);
        let old_rx = std::mem::replace(&mut self.tool_call_rx, new_rx);
        self.tool_call_tx = new_tx;
        old_rx
    }
    
    /// Execute Python code with the batch tool call model
    async fn execute_code(
        &mut self,
        input: CodeExecutionInput,
        _context: ExecutionContext,
    ) -> Result<CodeExecutionOutput, String> {
        use std::io::Write;
        
        let start_time = Instant::now();
        
        println!("[PythonActor] ========== EXECUTE CODE START ==========");
        println!("[PythonActor] Executing code ({} lines)", input.code.len());
        for (i, line) in input.code.iter().enumerate() {
            println!("[PythonActor]   {}: {}", i + 1, line);
        }
        let _ = std::io::stdout().flush();
        
        // Validate input
        println!("[PythonActor] Validating input...");
        let _ = std::io::stdout().flush();
        crate::tools::code_execution::CodeExecutionExecutor::validate_input(&input)?;
        println!("[PythonActor] Input validated");
        let _ = std::io::stdout().flush();
        
        // Get available tools from registry
        let available_tools = self.get_available_tools().await;
        
        // Build the initial request
        let mut request = ExecutionRequest {
            code: input.code.clone(),
            context: input.context.clone(),
            tool_results: HashMap::new(),
            available_tools,
            tool_modules: vec![],  // TODO: Populate from materialized modules
        };
        
        let mut output = CodeExecutionOutput::default();
        let mut total_tool_calls = 0;
        let mut round = 0;
        
        // Batch execution loop: run code, execute any pending tool calls, repeat
        loop {
            round += 1;
            if round > MAX_TOOL_CALL_ROUNDS {
                return Err(format!(
                    "Maximum tool call rounds ({}) exceeded - possible infinite loop",
                    MAX_TOOL_CALL_ROUNDS
                ));
            }
            
            println!("[PythonActor] ========== Execution round {} ==========", round);
            println!("[PythonActor] Calling python_sandbox::execute...");
            let _ = std::io::stdout().flush();
            
            // Execute the Python code using the sandbox
            let result = python_sandbox::execute(&request);
            
            println!("[PythonActor] python_sandbox::execute returned");
            println!("[PythonActor] Status: {:?}", result.status);
            println!("[PythonActor] stdout ({} chars): {}", result.stdout.len(), result.stdout);
            if !result.stderr.is_empty() {
                println!("[PythonActor] stderr: {}", result.stderr);
            }
            let _ = std::io::stdout().flush();
            
            // Accumulate stdout/stderr
            output.stdout.push_str(&result.stdout);
            output.stderr.push_str(&result.stderr);
            total_tool_calls += result.tool_calls_made;
            
            match result.status {
                ExecutionStatus::Complete => {
                    // Execution finished successfully
                    output.success = true;
                    output.result = result.result;
                    break;
                }
                ExecutionStatus::ToolCallsPending => {
                    // Need to execute tool calls and continue
                    if result.pending_calls.is_empty() {
                        // Shouldn't happen, but handle it
                        output.success = true;
                        output.result = result.result;
                        break;
                    }
                    
                    println!("[PythonActor] {} tool calls pending", result.pending_calls.len());
                    
                    // Execute each pending tool call
                    let mut tool_results = HashMap::new();
                    for pending_call in result.pending_calls {
                        let call_result = self.execute_tool_call(
                            &pending_call.tool_name,
                            &pending_call.server_id,
                            &pending_call.arguments,
                        ).await;
                        
                        // Use tool_name as key for matching (simpler than full ID tracking)
                        tool_results.insert(pending_call.tool_name.clone(), call_result);
                    }
                    
                    // Update request with tool results for next round
                    request.tool_results = tool_results;
                }
                ExecutionStatus::Error(msg) => {
                    output.success = false;
                    output.stderr.push_str(&format!("\nError: {}", msg));
                    break;
                }
                ExecutionStatus::Timeout => {
                    return Err("Execution timed out".to_string());
                }
                ExecutionStatus::OutOfFuel => {
                    return Err("Execution exceeded resource limits".to_string());
                }
            }
        }
        
        output.tool_calls_made = total_tool_calls;
        output.duration_ms = start_time.elapsed().as_millis() as u64;
        
        // Truncate output if too large
        if output.stdout.len() > MAX_OUTPUT_SIZE {
            output.stdout.truncate(MAX_OUTPUT_SIZE);
            output.stdout.push_str("\n... [output truncated]");
        }
        
        println!("[PythonActor] ========== EXECUTE CODE COMPLETE ==========");
        println!("[PythonActor] Success: {}, Duration: {}ms, Tool calls: {}", 
            output.success, output.duration_ms, output.tool_calls_made);
        println!("[PythonActor] Final stdout ({} chars): {}", output.stdout.len(), &output.stdout);
        let _ = std::io::stdout().flush();
        
        Ok(output)
    }
    
    /// Get available tools from the registry as ToolInfo for the sandbox
    async fn get_available_tools(&self) -> Vec<ToolInfo> {
        let registry = self.tool_registry.read().await;
        registry.get_visible_tools()
            .iter()
            .map(|schema| ToolInfo {
                name: schema.name.clone(),
                server_id: "default".to_string(), // TODO: Get actual server ID
                description: schema.description.clone(),
                parameters: schema.parameters.clone(),
                python_module: None,  // TODO: Get from registry
            })
            .collect()
    }
    
    /// Execute a single tool call via the orchestrator
    async fn execute_tool_call(
        &mut self,
        tool_name: &str,
        server_id: &str,
        arguments: &Value,
    ) -> ToolCallResult {
        println!("[PythonActor] Executing tool: {}::{}", server_id, tool_name);
        
        // Create the inner tool call
        let call = InnerToolCall {
            tool_name: tool_name.to_string(),
            server_id: server_id.to_string(),
            arguments: arguments.clone(),
            parent_exec_id: uuid::Uuid::new_v4().to_string(),
        };
        
        // Send to orchestrator and wait for result
        let (result_tx, result_rx) = oneshot::channel();
        
        if let Err(e) = self.tool_call_tx.send((call, result_tx)).await {
            return ToolCallResult {
                success: false,
                result: Value::Null,
                error: Some(format!("Failed to send tool call: {}", e)),
            };
        }
        
        match result_rx.await {
            Ok(inner_result) => ToolCallResult {
                success: inner_result.success,
                result: inner_result.result,
                error: inner_result.error,
            },
            Err(_) => ToolCallResult {
                success: false,
                result: Value::Null,
                error: Some("Tool call response channel closed".to_string()),
            },
        }
    }
}

/// Create a channel for communicating with the Python actor
pub fn create_python_channel() -> (mpsc::Sender<PythonMsg>, mpsc::Receiver<PythonMsg>) {
    mpsc::channel(32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_registry::ToolRegistry;
    
    #[tokio::test]
    async fn test_simple_execution() {
        // Create a minimal setup for testing
        let (_tx, rx) = create_python_channel();
        let registry = std::sync::Arc::new(tokio::sync::RwLock::new(ToolRegistry::new()));
        
        let mut actor = PythonActor::new(rx, registry);
        
        let input = CodeExecutionInput {
            code: vec!["x = 1 + 2".to_string(), "print(x)".to_string()],
            context: None,
        };
        
        let context = ExecutionContext {
            exec_id: "test".to_string(),
            tool_stubs: String::new(),
            user_context: None,
            available_tools: vec![],
        };
        
        let result = actor.execute_code(input, context).await;
        
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.success);
        assert!(output.stdout.contains("3"));
    }
    
    #[tokio::test]
    async fn test_code_validation() {
        use crate::tools::code_execution::CodeExecutionExecutor;
        
        let good = CodeExecutionInput {
            code: vec!["x = 1".to_string(), "print(x)".to_string()],
            context: None,
        };
        assert!(CodeExecutionExecutor::validate_input(&good).is_ok());
        
        let bad = CodeExecutionInput {
            code: vec!["import os".to_string()],
            context: None,
        };
        assert!(CodeExecutionExecutor::validate_input(&bad).is_err());
    }
}
