//! Built-in Tool Implementations
//!
//! This module contains implementations for Plugable Chat's built-in tools:
//! - `tool_search`: Semantic search over available tools using embeddings
//! - `python_execution`: Python code execution in a WASM sandbox

pub mod code_execution;
pub mod tool_search;

pub use code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};
pub use tool_search::{ToolSearchExecutor, ToolSearchInput, ToolSearchOutput};
