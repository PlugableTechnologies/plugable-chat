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

use fastembed::TextEmbedding;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::protocol::McpHostMsg;
use crate::tool_registry::SharedToolRegistry;
use crate::tools::code_execution::{
    CodeExecutionInput, CodeExecutionOutput, ExecutionContext, InnerCallResult, InnerToolCall,
};
use crate::tools::tool_search::{ToolSearchExecutor, ToolSearchInput};

// Import the python-sandbox crate
use python_sandbox::protocol::{ExecutionRequest, ExecutionStatus, ToolCallResult, ToolInfo};

/// Maximum output size (in bytes)
const MAX_OUTPUT_SIZE: usize = 1024 * 1024; // 1MB

/// Maximum number of tool call rounds to prevent infinite loops
const MAX_TOOL_CALL_ROUNDS: usize = 10;

/// Message types for the Python actor
pub enum PythonMsg {
    /// Execute Python code
    ExecuteSandboxedCode {
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
    HealthCheck { respond_to: oneshot::Sender<bool> },
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
pub struct PythonSandboxActor {
    python_msg_rx: mpsc::Receiver<PythonMsg>,
    tool_registry: SharedToolRegistry,
    mcp_host_tx: mpsc::Sender<McpHostMsg>,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    /// Channel to send tool calls to the orchestrator for execution
    tool_call_tx: mpsc::Sender<(InnerToolCall, oneshot::Sender<InnerCallResult>)>,
    tool_call_rx: mpsc::Receiver<(InnerToolCall, oneshot::Sender<InnerCallResult>)>,
}

impl PythonSandboxActor {
    pub fn new(
        python_msg_rx: mpsc::Receiver<PythonMsg>,
        tool_registry: SharedToolRegistry,
        mcp_host_tx: mpsc::Sender<McpHostMsg>,
        embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    ) -> Self {
        let (tool_call_tx, tool_call_rx) = mpsc::channel(32);

        Self {
            python_msg_rx,
            tool_registry,
            mcp_host_tx,
            embedding_model,
            tool_call_tx,
            tool_call_rx,
        }
    }

    pub async fn run(mut self) {
        println!("[PythonActor] Starting with RustPython sandbox...");

        loop {
            tokio::select! {
                msg = self.python_msg_rx.recv() => {
                    match msg {
                        Some(PythonMsg::ExecuteSandboxedCode { input, context, respond_to }) => {
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
    pub fn get_tool_call_receiver(
        &mut self,
    ) -> mpsc::Receiver<(InnerToolCall, oneshot::Sender<InnerCallResult>)> {
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
        context: ExecutionContext,
    ) -> Result<CodeExecutionOutput, String> {
        use std::io::Write;

        let start_time = Instant::now();

        println!("[PythonActor] ========== EXECUTE CODE START ==========");
        println!("[PythonActor] Executing code ({} lines)", input.code.len());
        for (i, line) in input.code.iter().enumerate() {
            println!("[PythonActor]   {}: {}", i + 1, line);
        }
        println!(
            "[PythonActor] Tool modules available: {}",
            context.tool_modules.len()
        );
        let _ = std::io::stdout().flush();

        // Build validation/import context from execution context
        let mut import_context = crate::tools::code_execution::DynamicImportContext::new();
        for module in &context.tool_modules {
            import_context.add_tool_module(module.python_name.clone(), module.server_id.clone());
        }
        let validation_context = crate::tools::code_execution::ValidationContext {
            import_context: Some(&import_context),
            allowed_functions: Some(&context.allowed_functions),
        };

        // Validate input
        println!("[PythonActor] Validating input...");
        let _ = std::io::stdout().flush();
        crate::tools::code_execution::CodeExecutionExecutor::validate_input_with_rules(
            &input,
            Some(validation_context),
        )?;
        println!("[PythonActor] Input validated");
        let _ = std::io::stdout().flush();

        // Convert available tools from context into ToolInfo for sandbox
        let module_by_server: std::collections::HashMap<String, String> = context
            .tool_modules
            .iter()
            .map(|m| (m.server_id.clone(), m.python_name.clone()))
            .collect();
        let available_tools: Vec<ToolInfo> = context
            .available_tools
            .iter()
            .map(|schema| {
                let server_id = context
                    .tool_server_map
                    .get(&schema.name)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                let python_module = module_by_server.get(&server_id).cloned();
                ToolInfo {
                    name: schema.name.clone(),
                    server_id,
                    description: schema.description.clone(),
                    parameters: schema.parameters.clone(),
                    python_module,
                }
            })
            .collect();

        // Build the initial request with tool modules from context
        let mut request = ExecutionRequest {
            code: input.code.clone(),
            context: input.context.clone(),
            tool_results: HashMap::new(),
            available_tools,
            tool_modules: context.tool_modules.clone(),
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

            println!(
                "[PythonActor] ========== Execution round {} ==========",
                round
            );
            println!("[PythonActor] Calling python_sandbox::execute (spawn_blocking)...");
            let _ = std::io::stdout().flush();

            // Execute the Python code using the sandbox on a blocking thread so we don't stall async tasks/UI
            let request_for_exec = request.clone();
            let result =
                tokio::task::spawn_blocking(move || python_sandbox::execute(&request_for_exec))
                    .await
                    .map_err(|e| format!("python_sandbox::execute join error: {}", e))?;

            println!("[PythonActor] python_sandbox::execute returned");
            println!("[PythonActor] Status: {:?}", result.status);
            println!(
                "[PythonActor] stdout ({} chars): {}",
                result.stdout.len(),
                result.stdout
            );
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

                    println!(
                        "[PythonActor] {} tool calls pending",
                        result.pending_calls.len()
                    );

                    // Execute each pending tool call
                    let mut tool_results = HashMap::new();
                    for pending_call in result.pending_calls {
                        let call_result = self
                            .execute_tool_call(
                                &pending_call.tool_name,
                                &pending_call.server_id,
                                &pending_call.arguments,
                            )
                            .await;

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
        println!(
            "[PythonActor] Success: {}, Duration: {}ms, Tool calls: {}",
            output.success, output.duration_ms, output.tool_calls_made
        );
        println!(
            "[PythonActor] Final stdout ({} chars): {}",
            output.stdout.len(),
            &output.stdout
        );
        let _ = std::io::stdout().flush();

        Ok(output)
    }

    /// Execute a single tool call via the orchestrator
    async fn execute_tool_call(
        &mut self,
        tool_name: &str,
        server_id: &str,
        arguments: &Value,
    ) -> ToolCallResult {
        println!("[PythonActor] Executing tool: {}::{}", server_id, tool_name);

        // Built-in: tool_search routed directly through the executor
        if server_id == "builtin" && tool_name == "tool_search" {
            let search_input =
                if let Some(query) = arguments.get("relevant_to").and_then(|v| v.as_str()) {
                    ToolSearchInput {
                        queries: vec![query.to_string()],
                        top_k: 3,
                    }
                } else {
                    serde_json::from_value::<ToolSearchInput>(arguments.clone()).unwrap_or(
                        ToolSearchInput {
                            queries: vec![],
                            top_k: 3,
                        },
                    )
                };

            let executor =
                ToolSearchExecutor::new(self.tool_registry.clone(), self.embedding_model.clone());

            match executor.execute(search_input).await {
                Ok(output) => {
                    executor.materialize_results(&output.tools).await;
                    let payload = serde_json::to_value(&output).unwrap_or(Value::Null);
                    return ToolCallResult {
                        success: true,
                        result: payload,
                        error: None,
                    };
                }
                Err(e) => {
                    return ToolCallResult {
                        success: false,
                        result: Value::Null,
                        error: Some(e),
                    };
                }
            }
        }

        // MCP tool execution
        let (tx, rx) = oneshot::channel();
        if let Err(e) = self
            .mcp_host_tx
            .send(McpHostMsg::ExecuteTool {
                server_id: server_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.clone(),
                respond_to: tx,
            })
            .await
        {
            return ToolCallResult {
                success: false,
                result: Value::Null,
                error: Some(format!("Failed to send tool call: {}", e)),
            };
        }

        match rx.await {
            Ok(Ok(result)) => {
                let text = result
                    .content
                    .iter()
                    .filter_map(|c| c.text.as_ref())
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

                ToolCallResult {
                    success: !result.is_error,
                    result: Value::String(text.clone()),
                    error: if result.is_error { Some(text) } else { None },
                }
            }
            Ok(Err(err)) => ToolCallResult {
                success: false,
                result: Value::Null,
                error: Some(err),
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
    use crate::tools::code_execution::CodeExecutionExecutor;

    #[tokio::test]
    async fn test_simple_execution() {
        // Create a minimal setup for testing
        let (_tx, rx) = create_python_channel();
        let registry = std::sync::Arc::new(tokio::sync::RwLock::new(ToolRegistry::new()));
        let (mcp_tx, _mcp_rx) = mpsc::channel(1);
        let embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>> = Arc::new(RwLock::new(None));

        let mut actor = PythonSandboxActor::new(rx, registry, mcp_tx, embedding_model);

        let input = CodeExecutionInput {
            code: vec!["x = 1 + 2".to_string(), "print(x)".to_string()],
            context: None,
        };

        let context =
            CodeExecutionExecutor::create_context("test".to_string(), vec![], None, vec![]);

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
