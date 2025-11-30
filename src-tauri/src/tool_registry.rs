//! Tool Registry - Manages built-in tools and domain tools with deferred loading
//!
//! This module provides a centralized registry for all tools available in Plugable Chat:
//! - Built-in tools: `code_execution` and `tool_search`
//! - Domain tools from MCP servers (can be deferred for semantic discovery)
//!
//! The registry also stores precomputed embeddings for semantic tool search.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde_json::json;

use crate::protocol::ToolSchema;
use crate::actors::mcp_host_actor::McpTool;

// ========== Built-in Tool Definitions ==========

/// Create the code_execution built-in tool schema
pub fn code_execution_tool() -> ToolSchema {
    ToolSchema {
        name: "code_execution".to_string(),
        description: Some(
            "Execute Python/WASP code in a secure sandbox. \
            This tool can call any allowed domain tools as Python functions. \
            Use for complex multi-step computations, data transformations, or orchestrating multiple tool calls."
                .to_string(),
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Lines of Python code to execute. Each line is a string in the array."
                }
            },
            "required": ["code"]
        }),
        tool_type: Some("code_execution_20250825".to_string()),
        allowed_callers: None, // Anyone can call code_execution
        defer_loading: false,
        embedding: None,
    }
}

/// Create the tool_search built-in tool schema
pub fn tool_search_tool() -> ToolSchema {
    ToolSchema {
        name: "tool_search".to_string(),
        description: Some(
            "Semantic search over available tools using embeddings. \
            Use this to discover relevant tools when you're not sure which tools are available. \
            Returns tool names, descriptions, and relevance scores."
                .to_string(),
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "queries": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Semantic search queries describing what you're looking for. E.g., ['get user data', 'weather forecast']"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of tools to return (default: 10)"
                }
            },
            "required": ["queries"]
        }),
        tool_type: Some("tool_search_20251201".to_string()),
        allowed_callers: None, // Anyone can call tool_search
        defer_loading: false,
        embedding: None,
    }
}

// ========== Tool Search Result ==========

/// Result from a tool search operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolSearchResult {
    pub name: String,
    pub description: Option<String>,
    pub score: f32,
    pub server_id: String,
}

// ========== Tool Registry ==========

/// Central registry for all tools in Plugable Chat
pub struct ToolRegistry {
    /// Built-in tools (code_execution, tool_search)
    internal_tools: Vec<ToolSchema>,
    /// Domain tools from MCP servers (indexed by server_id___tool_name)
    domain_tools: HashMap<String, ToolSchema>,
    /// Precomputed embeddings for tool descriptions (for semantic search)
    tool_embeddings: HashMap<String, Vec<f32>>,
    /// Set of tools that have been materialized (made visible after tool_search)
    materialized_tools: std::collections::HashSet<String>,
}

impl ToolRegistry {
    /// Create a new tool registry with built-in tools
    pub fn new() -> Self {
        let internal_tools = vec![code_execution_tool(), tool_search_tool()];
        
        Self {
            internal_tools,
            domain_tools: HashMap::new(),
            tool_embeddings: HashMap::new(),
            materialized_tools: std::collections::HashSet::new(),
        }
    }
    
    /// Register domain tools from an MCP server
    pub fn register_mcp_tools(&mut self, server_id: &str, tools: &[McpTool], defer: bool) {
        for tool in tools {
            let key = format!("{}___{}", server_id, tool.name);
            let schema = ToolSchema {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone().unwrap_or(json!({"type": "object", "properties": {}})),
                tool_type: None,
                allowed_callers: if defer {
                    // Deferred tools can only be called from code_execution
                    Some(vec!["code_execution_20250825".to_string()])
                } else {
                    None
                },
                defer_loading: defer,
                embedding: None,
            };
            
            println!("[ToolRegistry] Registered tool: {} (defer={})", key, defer);
            self.domain_tools.insert(key, schema);
        }
    }
    
    /// Remove all tools from a specific MCP server
    pub fn unregister_mcp_server(&mut self, server_id: &str) {
        let prefix = format!("{}___", server_id);
        let keys_to_remove: Vec<String> = self.domain_tools
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        
        for key in keys_to_remove {
            self.domain_tools.remove(&key);
            self.tool_embeddings.remove(&key);
            self.materialized_tools.remove(&key);
        }
        
        println!("[ToolRegistry] Unregistered all tools from server: {}", server_id);
    }
    
    /// Store a precomputed embedding for a tool
    pub fn set_tool_embedding(&mut self, tool_key: &str, embedding: Vec<f32>) {
        self.tool_embeddings.insert(tool_key.to_string(), embedding);
    }
    
    /// Get all built-in tools
    pub fn get_internal_tools(&self) -> &[ToolSchema] {
        &self.internal_tools
    }
    
    /// Get visible tools (internal + non-deferred domain tools + materialized deferred tools)
    pub fn get_visible_tools(&self) -> Vec<ToolSchema> {
        let mut tools = self.internal_tools.clone();
        
        for (key, schema) in &self.domain_tools {
            // Include if not deferred OR if materialized
            if !schema.defer_loading || self.materialized_tools.contains(key) {
                tools.push(schema.clone());
            }
        }
        
        tools
    }
    
    /// Get all deferred tools (for semantic search)
    pub fn get_deferred_tools(&self) -> Vec<(&String, &ToolSchema)> {
        self.domain_tools
            .iter()
            .filter(|(_, schema)| schema.defer_loading)
            .collect()
    }
    
    /// Get a specific tool by key (server___name)
    pub fn get_tool(&self, key: &str) -> Option<&ToolSchema> {
        // Check internal tools first
        for tool in &self.internal_tools {
            if tool.name == key {
                return Some(tool);
            }
        }
        
        // Check domain tools
        self.domain_tools.get(key)
    }
    
    /// Materialize a deferred tool (make it visible after tool_search discovers it)
    pub fn materialize_tool(&mut self, tool_key: &str) -> bool {
        if self.domain_tools.contains_key(tool_key) {
            self.materialized_tools.insert(tool_key.to_string());
            println!("[ToolRegistry] Materialized tool: {}", tool_key);
            true
        } else {
            false
        }
    }
    
    /// Materialize multiple tools at once
    pub fn materialize_tools(&mut self, tool_keys: &[String]) {
        for key in tool_keys {
            self.materialize_tool(key);
        }
    }
    
    /// Clear all materialized tools (for a new conversation)
    pub fn clear_materialized(&mut self) {
        self.materialized_tools.clear();
    }
    
    /// Perform semantic search over deferred tools
    ///
    /// Returns the top-k tools that match the query embeddings, sorted by score.
    pub fn search_tools(
        &self,
        query_embeddings: &[Vec<f32>],
        top_k: usize,
    ) -> Vec<ToolSearchResult> {
        let mut results: Vec<ToolSearchResult> = Vec::new();
        
        for (key, schema) in self.get_deferred_tools() {
            if let Some(tool_embedding) = self.tool_embeddings.get(key) {
                // Calculate max cosine similarity across all query embeddings
                let max_score = query_embeddings
                    .iter()
                    .map(|q| cosine_similarity(q, tool_embedding))
                    .fold(f32::NEG_INFINITY, f32::max);
                
                // Parse server_id from key
                let parts: Vec<&str> = key.splitn(2, "___").collect();
                let server_id = if parts.len() == 2 { parts[0] } else { "unknown" };
                
                results.push(ToolSearchResult {
                    name: schema.name.clone(),
                    description: schema.description.clone(),
                    score: max_score,
                    server_id: server_id.to_string(),
                });
            }
        }
        
        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Take top-k
        results.truncate(top_k);
        
        results
    }
    
    /// Check if code_mode should be enabled based on available tools
    pub fn should_enable_code_mode(&self) -> bool {
        // Enable code mode if there are any deferred tools
        self.domain_tools.values().any(|t| t.defer_loading)
    }
    
    /// Get statistics about the registry
    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            internal_tools: self.internal_tools.len(),
            domain_tools: self.domain_tools.len(),
            deferred_tools: self.domain_tools.values().filter(|t| t.defer_loading).count(),
            materialized_tools: self.materialized_tools.len(),
            tools_with_embeddings: self.tool_embeddings.len(),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the tool registry
#[derive(Debug, Clone)]
pub struct RegistryStats {
    pub internal_tools: usize,
    pub domain_tools: usize,
    pub deferred_tools: usize,
    pub materialized_tools: usize,
    pub tools_with_embeddings: usize,
}

// ========== Helper Functions ==========

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    
    dot_product / (norm_a * norm_b)
}

// ========== Shared Registry State ==========

/// Shared tool registry state for use across actors
pub type SharedToolRegistry = Arc<RwLock<ToolRegistry>>;

/// Create a new shared tool registry
pub fn create_shared_registry() -> SharedToolRegistry {
    Arc::new(RwLock::new(ToolRegistry::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_registry_creation() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.get_internal_tools().len(), 2);
        assert!(registry.get_internal_tools().iter().any(|t| t.name == "code_execution"));
        assert!(registry.get_internal_tools().iter().any(|t| t.name == "tool_search"));
    }
    
    #[test]
    fn test_tool_registration() {
        let mut registry = ToolRegistry::new();
        
        let mcp_tools = vec![
            McpTool {
                name: "get_weather".to_string(),
                description: Some("Get weather for a city".to_string()),
                input_schema: Some(json!({"type": "object", "properties": {"city": {"type": "string"}}})),
            },
        ];
        
        registry.register_mcp_tools("weather_server", &mcp_tools, false);
        
        assert!(registry.get_tool("weather_server___get_weather").is_some());
    }
    
    #[test]
    fn test_deferred_tools() {
        let mut registry = ToolRegistry::new();
        
        let mcp_tools = vec![
            McpTool {
                name: "internal_api".to_string(),
                description: Some("Internal API call".to_string()),
                input_schema: None,
            },
        ];
        
        registry.register_mcp_tools("internal", &mcp_tools, true);
        
        // Deferred tool should not be in visible tools
        let visible = registry.get_visible_tools();
        assert!(!visible.iter().any(|t| t.name == "internal_api"));
        
        // Materialize it
        registry.materialize_tool("internal___internal_api");
        
        // Now it should be visible
        let visible = registry.get_visible_tools();
        assert!(visible.iter().any(|t| t.name == "internal_api"));
    }
    
    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
        
        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c)).abs() < 0.001);
        
        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) + 1.0).abs() < 0.001);
    }
}

