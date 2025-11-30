//! Built-in Tool Implementations
//!
//! This module contains implementations for Plugable Chat's built-in tools:
//! - `tool_search`: Semantic search over available tools using embeddings
//! - `code_execution`: Python/WASP code execution in a WASM sandbox

pub mod tool_search;
pub mod code_execution;

pub use tool_search::{ToolSearchExecutor, ToolSearchInput, ToolSearchOutput};
pub use code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};

