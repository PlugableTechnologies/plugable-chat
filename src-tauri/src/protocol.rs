use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use std::sync::Arc;
use fastembed::TextEmbedding;
use regex::Regex;

/// Parsed tool call from assistant response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedToolCall {
    pub server: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub raw: String,
}

// ============ Tool Execution Event Payloads ============

/// Event payload when tool calls are detected and awaiting approval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallsPendingEvent {
    pub approval_key: String,
    pub calls: Vec<ParsedToolCall>,
    pub iteration: usize,
}

/// Event payload when a tool starts executing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutingEvent {
    pub server: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

/// Event payload when a tool finishes executing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultEvent {
    pub server: String,
    pub tool: String,
    pub result: String,
    pub is_error: bool,
}

/// Event payload when the agentic loop completes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLoopFinishedEvent {
    pub iterations: usize,
    pub had_tool_calls: bool,
}

/// Parse tool calls from assistant response
/// Looks for <tool_call>{"server": "...", "tool": "...", "arguments": {...}}</tool_call>
pub fn parse_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    
    // Use lazy_static or just create the regex here
    let re = Regex::new(r"<tool_call>(.*?)</tool_call>").unwrap();
    
    for cap in re.captures_iter(content) {
        if let Some(json_match) = cap.get(1) {
            let json_str = json_match.as_str().trim();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let (Some(server), Some(tool)) = (
                    parsed.get("server").and_then(|v| v.as_str()),
                    parsed.get("tool").and_then(|v| v.as_str()),
                ) {
                    let arguments = parsed.get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    
                    calls.push(ParsedToolCall {
                        server: server.to_string(),
                        tool: tool.to_string(),
                        arguments,
                        raw: cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
                    });
                }
            }
        }
    }
    
    calls
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModel {
    pub alias: String,
    pub model_id: String,
}

/// Model info from the running Foundry service with capability flags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub tool_calling: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
}

/// A chunk of text from a document with its source information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagChunk {
    pub id: String,
    pub content: String,
    pub source_file: String,
    pub chunk_index: usize,
    pub score: f32,
}

/// Result of processing documents for RAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagIndexResult {
    pub total_chunks: usize,
    pub files_processed: usize,
    pub cache_hits: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub score: f32, // Similarity score
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub enum VectorMsg {
    /// Index a new chat or update an existing one
    UpsertChat {
        id: String,
        title: String,
        content: String,
        messages: String, // JSON string of full history
        // The actor will handle embedding generation internally via Foundry
        // or receive a pre-computed vector.
        vector: Option<Vec<f32>>, 
        pinned: bool,
    },
    /// Search for similar chats
    SearchHistory {
        query_vector: Vec<f32>, 
        limit: usize,
        // Channel to send results back to the caller (Orchestrator)
        respond_to: oneshot::Sender<Vec<ChatSummary>>,
    },
    /// Get all chats
    GetAllChats {
        respond_to: oneshot::Sender<Vec<ChatSummary>>,
    },
    /// Get a specific chat's messages
    GetChat {
        id: String,
        respond_to: oneshot::Sender<Option<String>>, // Returns JSON string of messages
    },
    /// Delete a chat
    DeleteChat {
        id: String,
        respond_to: oneshot::Sender<bool>,
    },
    /// Update chat metadata (title, pinned)
    UpdateChatMetadata {
        id: String,
        title: Option<String>,
        pinned: Option<bool>,
        respond_to: oneshot::Sender<bool>,
    },
}

pub enum FoundryMsg {
    /// Generate an embedding for a string
    GetEmbedding {
        text: String,
        respond_to: oneshot::Sender<Vec<f32>>,
    },
    /// Chat with the model (streaming)
    Chat {
        history: Vec<ChatMessage>,
        reasoning_effort: String,
        respond_to: tokio::sync::mpsc::UnboundedSender<String>,
    },
    /// Get available models from running service
    GetModels {
        respond_to: oneshot::Sender<Vec<String>>,
    },
    /// Get model info with capabilities from running service
    GetModelInfo {
        respond_to: oneshot::Sender<Vec<ModelInfo>>,
    },
    /// Get cached models from `foundry cache ls`
    GetCachedModels {
        respond_to: oneshot::Sender<Vec<CachedModel>>,
    },
    /// Set the active model
    SetModel {
        model_id: String,
        respond_to: oneshot::Sender<bool>,
    },
}

pub enum McpMsg {
    ExecuteTool {
        tool_name: String,
        args: serde_json::Value,
    },
}

use crate::settings::McpServerConfig;
use crate::actors::mcp_host_actor::{McpTool, McpToolResult};

/// Messages for the MCP Host Actor
pub enum McpHostMsg {
    /// Connect to an MCP server
    ConnectServer {
        config: McpServerConfig,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Disconnect from an MCP server
    DisconnectServer {
        server_id: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// List tools available from a server
    ListTools {
        server_id: String,
        respond_to: oneshot::Sender<Result<Vec<McpTool>, String>>,
    },
    /// Execute a tool on a server
    ExecuteTool {
        server_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        respond_to: oneshot::Sender<Result<McpToolResult, String>>,
    },
    /// Get all tool descriptions from enabled servers (for system prompt)
    GetAllToolDescriptions {
        respond_to: oneshot::Sender<Vec<(String, Vec<McpTool>)>>,
    },
    /// Check if a server is connected
    GetServerStatus {
        server_id: String,
        respond_to: oneshot::Sender<bool>,
    },
    /// Sync enabled servers - connect enabled ones, disconnect disabled ones
    SyncEnabledServers {
        configs: Vec<McpServerConfig>,
        respond_to: oneshot::Sender<Vec<(String, Result<(), String>)>>,
    },
}

/// Messages for the RAG (Retrieval Augmented Generation) actor
pub enum RagMsg {
    /// Process and index documents for RAG
    ProcessDocuments {
        paths: Vec<String>,
        embedding_model: Arc<TextEmbedding>,
        respond_to: oneshot::Sender<Result<RagIndexResult, String>>,
    },
    /// Search indexed documents for relevant chunks
    SearchDocuments {
        query_vector: Vec<f32>,
        limit: usize,
        respond_to: oneshot::Sender<Vec<RagChunk>>,
    },
    /// Clear all indexed documents (reset context)
    ClearContext {
        respond_to: oneshot::Sender<bool>,
    },
}
