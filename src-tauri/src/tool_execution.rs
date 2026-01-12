//! Tool execution utilities for the agentic loop.
//!
//! This module provides functions for executing different types of tools:
//! - MCP tools via the McpHostActor
//! - Built-in tools like python_execution and tool_search
//! - Server resolution for unknown tool servers

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::actors::python_actor::PythonMsg;
use crate::protocol::{McpHostMsg, ParsedToolCall};
use crate::python_helpers::strip_unsupported_python;
use crate::tool_registry::{self, SharedToolRegistry, ToolSearchResult};
use crate::tools::code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};
use crate::tools::tool_search::{ToolSearchExecutor, ToolSearchInput};
use fastembed::TextEmbedding;

/// Tool type identifier for python_execution - used for allowed_callers filtering.
pub const PYTHON_EXECUTION_TOOL_TYPE: &str = "python_execution_20251206";

/// Execute a tool call via McpHostActor.
///
/// This is the main entry point for executing MCP server tools.
/// The result is returned as a string, with errors wrapped in Result::Err.
pub async fn dispatch_tool_call_to_executor(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    call: &ParsedToolCall,
) -> Result<String, String> {
    let (tx, rx) = oneshot::channel();
    mcp_host_tx
        .send(McpHostMsg::ExecuteTool {
            server_id: call.server.clone(),
            tool_name: call.tool.clone(),
            arguments: call.arguments.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send to MCP Host: {}", e))?;

    let result = rx.await.map_err(|_| "MCP Host actor died".to_string())??;

    // Convert the result to a string
    let result_text = result
        .content
        .iter()
        .filter_map(|c| c.text.as_ref())
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    if result.is_error {
        Err(result_text)
    } else {
        Ok(result_text)
    }
}

/// Try to resolve an unknown server ID by finding which server has the given tool.
///
/// When a model outputs a tool call with server="unknown", this function
/// searches all connected MCP servers to find which one provides the tool.
pub async fn resolve_mcp_server_for_tool(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    tool_name: &str,
) -> Option<String> {
    println!(
        "[resolve_mcp_server_for_tool] Searching for tool '{}' across servers...",
        tool_name
    );

    // Get all tool descriptions from connected servers
    let (tx, rx) = oneshot::channel();
    if mcp_host_tx
        .send(McpHostMsg::GetAllToolDescriptions { respond_to: tx })
        .await
        .is_err()
    {
        return None;
    }

    let tool_descriptions = match rx.await {
        Ok(descriptions) => descriptions,
        Err(_) => return None,
    };

    // Search for the tool in each server
    for (server_id, tools) in tool_descriptions {
        for tool in tools {
            if tool.name == tool_name {
                println!(
                    "[resolve_mcp_server_for_tool] Found tool '{}' on server '{}'",
                    tool_name, server_id
                );
                return Some(server_id);
            }
        }
    }

    println!(
        "[resolve_mcp_server_for_tool] Tool '{}' not found on any connected server",
        tool_name
    );
    None
}

/// Execute the tool_search built-in tool.
///
/// Searches the tool registry for tools matching the given queries,
/// returning formatted results that guide the model to use python_execution.
pub async fn execute_tool_search(
    input: ToolSearchInput,
    tool_registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    max_results: usize,
) -> Result<(String, Vec<ToolSearchResult>), String> {
    let executor = ToolSearchExecutor::new(tool_registry.clone(), embedding_model);
    let mut capped_input = input.clone();
    let top_cap = std::cmp::max(1, max_results);
    capped_input.top_k = std::cmp::max(1, std::cmp::min(capped_input.top_k, top_cap));
    let output = executor.execute(capped_input).await?;

    // Filter out tools that cannot be called from python_execution (respect allowed_callers)
    let filtered_tools: Vec<ToolSearchResult> = {
        let registry_guard = tool_registry.read().await;
        output
            .tools
            .iter()
            .filter(|tool| {
                let key = format!("{}___{}", tool.server_id, tool.name);
                match registry_guard.get_tool(&key) {
                    Some(schema) => schema.can_be_called_by(Some(PYTHON_EXECUTION_TOOL_TYPE)),
                    None => true,
                }
            })
            .cloned()
            .collect()
    };

    // Materialize discovered tools
    executor.materialize_results(&filtered_tools).await;

    // Format result for the model with clear instructions to use python_execution
    let mut result = String::new();
    result.push_str("# Discovered Tools\n\n");
    result.push_str(
        "**YOUR NEXT STEP: Return a single Python program that uses these functions. Do NOT emit <tool_call> tags.**\n\n",
    );

    // Build the python code example
    let mut python_lines: Vec<String> = vec![];
    let mut tool_docs: Vec<String> = vec![];

    for tool in &filtered_tools {
        // Document the tool
        let mut doc = format!("### {}(", tool.name);
        let mut params: Vec<String> = vec![];
        let mut example_params: Vec<String> = vec![];

        if let Some(props) = tool
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
        {
            let required: Vec<&str> = tool
                .parameters
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            for (name, schema) in props {
                let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                let is_required = required.contains(&name.as_str());
                params.push(format!(
                    "{}: {}{}",
                    name,
                    type_str,
                    if is_required { "" } else { " (optional)" }
                ));

                // Build example call with placeholders
                if is_required {
                    let example_val = match type_str {
                        "string" => format!("\"...\""),
                        "integer" => "1".to_string(),
                        "boolean" => "True".to_string(),
                        "array" => "[]".to_string(),
                        _ => "...".to_string(),
                    };
                    example_params.push(format!("{}={}", name, example_val));
                }
            }
        }

        doc.push_str(&params.join(", "));
        doc.push_str(")\n");
        if let Some(ref desc) = tool.description {
            doc.push_str(&format!("{}\n", desc));
        }
        tool_docs.push(doc);

        // Add to example Python code (just the first tool as primary example)
        if python_lines.is_empty() {
            let call = if example_params.is_empty() {
                format!("result = {}()", tool.name)
            } else {
                format!("result = {}({})", tool.name, example_params.join(", "))
            };
            python_lines.push(call);
            python_lines.push("print(result)".to_string());
        }
    }

    // Show available tools
    for doc in tool_docs {
        result.push_str(&doc);
        result.push_str("\n");
    }

    // Show example python_execution program to make
    result.push_str("---\n\n");
    result.push_str("**NOW return exactly this shape (single Python block):**\n");
    result.push_str("```python\n");
    result.push_str("# Use the discovered tools directly\n");
    for line in &python_lines {
        result.push_str(line);
        result.push('\n');
    }
    result.push_str("```\n");

    Ok((result, filtered_tools))
}

/// Execute the python_execution built-in tool.
///
/// Runs Python code in a sandboxed environment with access to tool functions.
pub async fn execute_python_code(
    input: CodeExecutionInput,
    exec_id: String,
    tool_registry: SharedToolRegistry,
    python_tx: &mpsc::Sender<PythonMsg>,
    allow_tool_search: bool,
) -> Result<CodeExecutionOutput, String> {
    // Strip unsupported keywords before execution
    let code = strip_unsupported_python(&input.code);

    // Log the code about to be executed
    println!("[python_execution] exec_id={}", exec_id);
    println!("[python_execution] Code to execute ({} lines):", code.len());
    for (i, line) in code.iter().enumerate() {
        println!("[python_execution]   {}: {}", i + 1, line);
    }
    // Flush stdout to ensure logs appear immediately
    use std::io::Write;
    let _ = std::io::stdout().flush();

    // Get available tools and materialized tool modules for the execution context
    let (available_tools_with_servers, mut tool_modules) = {
        let registry = tool_registry.read().await;
        let tools = registry.get_visible_tools_with_servers();
        let modules = registry.get_materialized_tool_modules();
        let stats = registry.stats();
        println!(
            "[python_execution] Registry stats: {} materialized tools",
            stats.materialized_tools
        );
        (tools, modules)
    };

    // Filter tools: remove python_execution, optionally remove tool_search if disabled
    let mut filtered_tools = Vec::new();
    for (server_id, tool) in available_tools_with_servers {
        if tool.name == "python_execution" {
            continue;
        }
        if !tool.can_be_called_by(Some(PYTHON_EXECUTION_TOOL_TYPE)) {
            continue;
        }
        if tool.name == "tool_search" && !allow_tool_search {
            continue;
        }
        filtered_tools.push((server_id, tool));
    }

    // Inject a builtin module for tool_search if it is allowed (so python can call it directly)
    if allow_tool_search {
        tool_modules.push(tool_registry::ToolModuleInfo {
            python_name: "builtin_tools".to_string(),
            server_id: "builtin".to_string(),
            functions: vec![tool_registry::ToolFunctionInfo {
                name: "tool_search".to_string(),
                description: Some(
                    "Semantic search over available tools. Call with relevant_to string."
                        .to_string(),
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "relevant_to": { "type": "string" }
                    },
                    "required": ["relevant_to"]
                }),
            }],
        });
    }

    println!(
        "[python_execution] Available tools: {}, Tool modules: {}",
        filtered_tools.len(),
        tool_modules.len()
    );
    for module in &tool_modules {
        println!(
            "[python_execution]   Module '{}' (server: {}) with {} functions",
            module.python_name,
            module.server_id,
            module.functions.len()
        );
        for func in &module.functions {
            println!("[python_execution]     - {}", func.name);
        }
    }
    let _ = std::io::stdout().flush();

    // Create execution context
    let context = CodeExecutionExecutor::create_context(
        exec_id.clone(),
        filtered_tools,
        input.context.clone(),
        tool_modules,
    );

    // Create modified input with the cleaned code
    let cleaned_input = CodeExecutionInput {
        code,
        context: input.context,
    };

    // Pre-validate before sending to the Python actor so errors can be surfaced immediately
    let mut import_context = crate::tools::code_execution::DynamicImportContext::new();
    for module in &context.tool_modules {
        import_context.add_tool_module(module.python_name.clone(), module.server_id.clone());
    }
    let validation_context = crate::tools::code_execution::ValidationContext {
        import_context: Some(&import_context),
        allowed_functions: Some(&context.allowed_functions),
    };
    crate::tools::code_execution::CodeExecutionExecutor::validate_input_with_rules(
        &cleaned_input,
        Some(validation_context),
    )?;

    println!("[python_execution] Sending to Python actor...");
    let _ = std::io::stdout().flush();

    // Send to Python actor for execution
    let (respond_to, rx) = oneshot::channel();
    python_tx
        .send(PythonMsg::ExecuteSandboxedCode {
            input: cleaned_input,
            context,
            respond_to,
        })
        .await
        .map_err(|e| format!("Failed to send to Python actor: {}", e))?;

    println!("[python_execution] Waiting for Python actor response...");
    let _ = std::io::stdout().flush();

    let result = rx.await.map_err(|_| "Python actor died".to_string())?;

    println!(
        "[python_execution] Python execution complete: success={}",
        result.as_ref().map(|r| r.success).unwrap_or(false)
    );
    let _ = std::io::stdout().flush();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Most of these functions require actor infrastructure to test properly.
    // Unit tests are limited to pure functions.
    
    #[test]
    fn test_module_compiles() {
        // Basic compilation check
        assert!(true);
    }
}
