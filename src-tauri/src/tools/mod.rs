//! Built-in Tool Implementations
//!
//! This module contains implementations for Plugable Chat's built-in tools:
//! - `tool_search`: Semantic search over available tools using embeddings
//! - `python_execution`: Python code execution in a WASM sandbox
//! - `schema_search`: Semantic search over cached database schemas
//! - `sql_select`: Execute SQL queries against configured databases

pub mod code_execution;
pub mod schema_search;
pub mod sql_select;
pub mod tool_search;

pub use code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};
pub use schema_search::{SchemaSearchExecutor, SchemaSearchInput, SchemaSearchOutput};
pub use sql_select::{SqlSelectExecutor, SqlSelectInput, SqlSelectOutput};
pub use tool_search::{ToolSearchExecutor, ToolSearchInput, ToolSearchOutput};
