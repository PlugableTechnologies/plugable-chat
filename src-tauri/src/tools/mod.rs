//! Built-in Tool Implementations
//!
//! This module contains implementations for Plugable Chat's built-in tools:
//! - `tool_search`: Semantic search over available tools using embeddings
//! - `python_execution`: Python code execution in a WASM sandbox
//! - `schema_search`: Semantic search over cached database schemas
//! - `execute_sql`: Execute SQL queries against configured databases

pub mod code_execution;
pub mod execute_sql;
pub mod schema_search;
pub mod tool_search;

pub use code_execution::{CodeExecutionExecutor, CodeExecutionInput, CodeExecutionOutput};
pub use execute_sql::{ExecuteSqlExecutor, ExecuteSqlInput, ExecuteSqlOutput};
pub use schema_search::{SchemaSearchExecutor, SchemaSearchInput, SchemaSearchOutput};
pub use tool_search::{ToolSearchExecutor, ToolSearchInput, ToolSearchOutput};
