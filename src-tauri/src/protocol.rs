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
/// Supports two formats:
/// 1. Text-based: <tool_call>{"server": "...", "tool": "...", "arguments": {...}}</tool_call>
/// 2. Native (Qwen/OpenAI): <tool_call>{"name": "server___tool", "arguments": {...}}</tool_call>
/// Also handles unclosed tool calls where the model forgets </tool_call>
pub fn parse_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    
    // Match <tool_call> with optional whitespace (use (?s) for DOTALL mode to match newlines)
    let re = Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap();
    
    println!("[parse_tool_calls] Searching for tool calls in content ({} chars)", content.len());
    
    // Also check for unclosed tool calls (model forgot to add </tool_call>)
    let unclosed_re = Regex::new(r"(?s)<tool_call>\s*(\{.*)");
    
    for cap in re.captures_iter(content) {
        if let Some(json_match) = cap.get(1) {
            let json_str = json_match.as_str().trim();
            println!("[parse_tool_calls] Found tool_call block: {}", json_str);
            
            // Try to fix common JSON issues from LLMs
            let fixed_json = fix_llm_json(json_str);
            
            match serde_json::from_str::<serde_json::Value>(&fixed_json) {
                Ok(parsed) => {
                    let raw = cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
                    
                    // Try Format 1: {"server": "...", "tool": "...", "arguments": {...}}
                    if let (Some(server), Some(tool)) = (
                        parsed.get("server").and_then(|v| v.as_str()),
                        parsed.get("tool").and_then(|v| v.as_str()),
                    ) {
                        let arguments = parsed.get("arguments")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        
                        println!("[parse_tool_calls] Parsed format 1: server={}, tool={}", server, tool);
                        
                        calls.push(ParsedToolCall {
                            server: server.to_string(),
                            tool: tool.to_string(),
                            arguments,
                            raw,
                        });
                        continue;
                    }
                    
                // Try Format 2: {"name": "...", "arguments": {...}}
                // This is what Qwen outputs when using native tool calling
                if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                    let arguments = parsed.get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    
                    // Parse server___tool format if present
                    let (server, tool) = if let Some((s, t)) = parse_combined_tool_name(name) {
                        println!("[parse_tool_calls] Parsed format 2 (native with server): server={}, tool={}", s, t);
                        (s, t)
                    } else {
                        // name doesn't contain ___, use "unknown" server - will be resolved later
                        println!("[parse_tool_calls] Parsed format 2 (no server prefix): tool={}, will resolve server later", name);
                        ("unknown".to_string(), name.to_string())
                    };
                    
                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw,
                    });
                } else {
                    println!("[parse_tool_calls] WARNING: JSON parsed but no 'server'/'tool' or 'name' field found");
                }
                }
                Err(e) => {
                    println!("[parse_tool_calls] ERROR: Failed to parse JSON: {}", e);
                    println!("[parse_tool_calls] Original JSON: {}", json_str);
                    println!("[parse_tool_calls] Fixed JSON: {}", fixed_json);
                    
                    // Try fallback parser for malformed JSON (e.g., unescaped quotes in SQL)
                    if let Some((server, tool, arguments)) = parse_tool_call_fallback(json_str) {
                        println!("[parse_tool_calls] Fallback succeeded: server={}, tool={}", server, tool);
                        calls.push(ParsedToolCall {
                            server,
                            tool,
                            arguments,
                            raw: cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
                        });
                    } else {
                        println!("[parse_tool_calls] Fallback also failed");
                    }
                }
            }
        }
    }
    
    // If no tool calls found, check for unclosed tool calls
    if calls.is_empty() {
        if let Ok(unclosed_re) = unclosed_re {
            if let Some(cap) = unclosed_re.captures(content) {
                if let Some(json_match) = cap.get(1) {
                    let json_str = json_match.as_str().trim();
                    println!("[parse_tool_calls] Found UNCLOSED tool call, attempting to parse: {}...", 
                        if json_str.len() > 100 { &json_str[..100] } else { json_str });
                    
                    // Try to extract balanced JSON from the unclosed content
                    if let Some(balanced_json) = extract_balanced_braces(json_str) {
                        println!("[parse_tool_calls] Extracted balanced JSON from unclosed tool call");
                        
                        let fixed_json = fix_llm_json(&balanced_json);
                        
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&fixed_json) {
                            // Try to extract tool call from parsed JSON
                            if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                                let arguments = parsed.get("arguments")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                
                                let (server, tool) = if let Some((s, t)) = parse_combined_tool_name(name) {
                                    (s, t)
                                } else {
                                    ("unknown".to_string(), name.to_string())
                                };
                                
                                println!("[parse_tool_calls] Successfully parsed unclosed tool call: server={}, tool={}", server, tool);
                                
                                calls.push(ParsedToolCall {
                                    server,
                                    tool,
                                    arguments,
                                    raw: format!("<tool_call>{}</tool_call>", balanced_json),
                                });
                            }
                        } else {
                            // Try fallback parser
                            if let Some((server, tool, arguments)) = parse_tool_call_fallback(&balanced_json) {
                                println!("[parse_tool_calls] Fallback parsed unclosed tool call: server={}, tool={}", server, tool);
                                calls.push(ParsedToolCall {
                                    server,
                                    tool,
                                    arguments,
                                    raw: format!("<tool_call>{}</tool_call>", balanced_json),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    
    println!("[parse_tool_calls] Found {} tool call(s)", calls.len());
    calls
}

/// Fix common JSON issues from LLMs:
/// - Trailing commas: {"key": "value",} -> {"key": "value"}
fn fix_llm_json(json_str: &str) -> String {
    let mut result = json_str.to_string();
    
    // Remove trailing commas before } or ]
    // Pattern: ,\s*} or ,\s*]
    let trailing_comma_re = Regex::new(r",(\s*[}\]])").unwrap();
    result = trailing_comma_re.replace_all(&result, "$1").to_string();
    
    result
}

/// Fallback parser for tool calls when JSON is malformed
/// Extracts name and arguments using regex patterns
fn parse_tool_call_fallback(json_str: &str) -> Option<(String, String, serde_json::Value)> {
    println!("[parse_tool_call_fallback] Attempting fallback parsing...");
    
    // Try to extract "name": "value"
    let name_re = Regex::new(r#""name"\s*:\s*"([^"]+)""#).unwrap();
    let name = name_re.captures(json_str)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())?;
    
    println!("[parse_tool_call_fallback] Extracted name: {}", name);
    
    // Try to extract "arguments": {...} or "arguments": "..."
    // Find the start of arguments
    let args_start_re = Regex::new(r#""arguments"\s*:\s*"#).unwrap();
    if let Some(args_match) = args_start_re.find(json_str) {
        let after_args_key = &json_str[args_match.end()..];
        
        // Check if arguments is an object or a string
        if after_args_key.starts_with('{') {
            // It's an object - find matching closing brace
            if let Some(args_json) = extract_balanced_braces(after_args_key) {
                println!("[parse_tool_call_fallback] Extracted arguments object: {}", 
                    if args_json.len() > 100 { format!("{}...", &args_json[..100]) } else { args_json.clone() });
                
                // Try to parse the arguments, but if it fails, wrap the whole thing as a string
                let arguments = match serde_json::from_str::<serde_json::Value>(&args_json) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("[parse_tool_call_fallback] Arguments JSON invalid ({}), extracting raw values...", e);
                        // Try to extract individual key-value pairs
                        extract_arguments_permissive(&args_json)
                    }
                };
                
                // Parse server and tool from name
                if let Some((server, tool)) = parse_combined_tool_name(&name) {
                    return Some((server, tool, arguments));
                } else {
                    // No server prefix - return with empty server (will need handling upstream)
                    println!("[parse_tool_call_fallback] No server prefix in name, using 'unknown' server");
                    return Some(("unknown".to_string(), name, arguments));
                }
            }
        }
    }
    
    println!("[parse_tool_call_fallback] Failed to extract arguments");
    None
}

/// Extract a balanced {} block from the start of a string
fn extract_balanced_braces(s: &str) -> Option<String> {
    if !s.starts_with('{') {
        return None;
    }
    
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;
    
    for (i, c) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        
        match c {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[..=i].to_string());
                }
            }
            _ => {}
        }
    }
    
    None
}

/// Extract arguments permissively, handling malformed JSON
fn extract_arguments_permissive(args_str: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    
    // Try to find key-value pairs like "key": value or "key": "value"
    // This is a simplified extraction that handles common cases
    
    // Pattern for "key": "value" (string value)
    let string_kv_re = Regex::new(r#""([^"]+)"\s*:\s*"((?:[^"\\]|\\.)*)""#).unwrap();
    for cap in string_kv_re.captures_iter(args_str) {
        if let (Some(key), Some(value)) = (cap.get(1), cap.get(2)) {
            let key_str = key.as_str();
            let value_str = value.as_str();
            // Skip if this looks like it's part of nested content
            if key_str != "name" && key_str != "arguments" {
                map.insert(key_str.to_string(), serde_json::Value::String(value_str.to_string()));
            }
        }
    }
    
    // Pattern for "key": true/false/null/number
    let literal_kv_re = Regex::new(r#""([^"]+)"\s*:\s*(true|false|null|-?\d+(?:\.\d+)?)"#).unwrap();
    for cap in literal_kv_re.captures_iter(args_str) {
        if let (Some(key), Some(value)) = (cap.get(1), cap.get(2)) {
            let key_str = key.as_str();
            let value_str = value.as_str();
            if !map.contains_key(key_str) {
                let parsed_value = match value_str {
                    "true" => serde_json::Value::Bool(true),
                    "false" => serde_json::Value::Bool(false),
                    "null" => serde_json::Value::Null,
                    _ => {
                        if let Ok(n) = value_str.parse::<i64>() {
                            serde_json::Value::Number(n.into())
                        } else if let Ok(f) = value_str.parse::<f64>() {
                            serde_json::Number::from_f64(f)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::String(value_str.to_string()))
                        } else {
                            serde_json::Value::String(value_str.to_string())
                        }
                    }
                };
                map.insert(key_str.to_string(), parsed_value);
            }
        }
    }
    
    // Special handling for "sql" field - extract everything between "sql": " and the last "
    // This handles cases where SQL contains unescaped quotes
    if !map.contains_key("sql") {
        let sql_re = Regex::new(r#""sql"\s*:\s*""#).unwrap();
        if let Some(sql_match) = sql_re.find(args_str) {
            let after_sql = &args_str[sql_match.end()..];
            // Find the last quote before }} or end of string
            if let Some(end_pos) = after_sql.rfind('"') {
                let sql_content = &after_sql[..end_pos];
                println!("[extract_arguments_permissive] Extracted SQL: {}", 
                    if sql_content.len() > 100 { format!("{}...", &sql_content[..100]) } else { sql_content.to_string() });
                map.insert("sql".to_string(), serde_json::Value::String(sql_content.to_string()));
            }
        }
    }
    
    serde_json::Value::Object(map)
}

/// Parse a combined "server___tool" name into (server, tool)
fn parse_combined_tool_name(combined: &str) -> Option<(String, String)> {
    // Split on "___" (three underscores) which we use as the separator
    let parts: Vec<&str> = combined.splitn(2, "___").collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModel {
    pub alias: String,
    pub model_id: String,
}

/// Model family for determining response format and capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelFamily {
    /// GPT-OSS models (gpt-oss-20b, gpt-oss-120b) - use channel-based response format
    GptOss,
    /// Google Gemma models - standard response format
    Gemma,
    /// Microsoft Phi models - use <think> tags for reasoning variants
    Phi,
    /// IBM Granite models - use <|thinking|> tags for reasoning
    Granite,
    /// Generic/unknown models - standard OpenAI-compatible format
    #[default]
    Generic,
}

impl ModelFamily {
    /// Detect model family from model ID string
    pub fn from_model_id(model_id: &str) -> Self {
        let lower = model_id.to_lowercase();
        
        if lower.contains("gpt-oss") {
            ModelFamily::GptOss
        } else if lower.contains("gemma") {
            ModelFamily::Gemma
        } else if lower.contains("phi") {
            ModelFamily::Phi
        } else if lower.contains("granite") {
            ModelFamily::Granite
        } else {
            ModelFamily::Generic
        }
    }
}

/// Tool calling format supported by the model
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolFormat {
    /// OpenAI-compatible tool_calls array in response
    #[default]
    OpenAI,
    /// Hermes-style <tool_call> XML format (Phi, Qwen)
    Hermes,
    /// Gemini function_call format
    Gemini,
    /// Granite <function_call> XML format
    Granite,
    /// No native tool calling support - use text-based fallback
    TextBased,
}

/// Reasoning/thinking output format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningFormat {
    /// No reasoning output
    #[default]
    None,
    /// Phi-style <think>...</think> tags
    ThinkTags,
    /// GPT-OSS channel-based: <|channel|>analysis<|message|>...<|end|>
    ChannelBased,
    /// Granite-style <|thinking|>...<|/thinking|> tags
    ThinkingTags,
}

/// Model info from the running Foundry service with capability flags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    /// Model family for format-specific handling
    pub family: ModelFamily,
    pub tool_calling: bool,
    /// Native tool calling format used by this model
    pub tool_format: ToolFormat,
    pub vision: bool,
    pub reasoning: bool,
    /// Format used for reasoning/thinking output
    pub reasoning_format: ReasoningFormat,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
    /// Whether the model supports temperature parameter
    pub supports_temperature: bool,
    /// Whether the model supports top_p parameter
    pub supports_top_p: bool,
    /// Whether the model supports reasoning_effort parameter
    pub supports_reasoning_effort: bool,
}

/// OpenAI-compatible tool definition for native tool calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl OpenAITool {
    /// Create from MCP tool definition
    /// The function name is prefixed with server_id for routing
    pub fn from_mcp(server_id: &str, tool: &crate::actors::mcp_host_actor::McpTool) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: OpenAIFunction {
                // Encode server_id in the function name for routing
                name: format!("{}___{}", server_id, tool.name),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            },
        }
    }
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
        /// Optional OpenAI-format tools for native tool calling
        tools: Option<Vec<OpenAITool>>,
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
