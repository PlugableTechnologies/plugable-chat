//! Tauri command modules.
//!
//! This module organizes all Tauri commands by domain:
//! - `model`: Model management (load, unload, download, etc.)
//! - `rag`: RAG document processing and search
//! - `settings`: Application settings and configuration
//! - `mcp`: MCP server management and tool execution
//! - `database`: Database schema cache management
//! - `tool`: Tool call detection, execution, and approval
//! - `chat`: Chat and history management
//! - `startup`: Startup coordination and handshake
//! - `diagnostics`: GPU diagnostics and first-run retries

pub mod chat;
pub mod database;
pub mod diagnostics;
pub mod mcp;
pub mod model;
pub mod rag;
pub mod settings;
pub mod startup;
pub mod tool;
pub mod toolbox;

// Re-export all commands for easy access from lib.rs
pub use chat::*;
pub use database::*;
pub use diagnostics::*;
pub use mcp::*;
pub use model::*;
pub use rag::*;
pub use settings::*;
pub use startup::*;
pub use tool::*;
pub use toolbox::*;
