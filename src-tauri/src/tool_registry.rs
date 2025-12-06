//! Tool Registry - Manages built-in tools and domain tools with deferred loading
//!
//! This module provides a centralized registry for all tools available in Plugable Chat:
//! - Built-in tools: `python_execution` and `tool_search`
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

/// Create the python_execution built-in tool schema
pub fn python_execution_tool() -> ToolSchema {
    ToolSchema {
        name: "python_execution".to_string(),
        description: Some(
            "Execute Python code in a secure sandbox. \
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
        tool_type: Some("python_execution_20251206".to_string()),
        allowed_callers: None, // Anyone can call python_execution
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
                    "description": "Maximum number of tools to return (default: 3)"
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
    /// Parameter schema for generating Python function signatures
    #[serde(default)]
    pub parameters: serde_json::Value,
}

// ========== Python Module Mapping ==========

/// Information about a tool module for Python imports
#[derive(Debug, Clone)]
pub struct ToolModuleInfo {
    /// Python module name (e.g., "mcp_weather")
    pub python_name: String,
    /// Original MCP server ID
    pub server_id: String,
    /// Tool functions available in this module
    pub functions: Vec<ToolFunctionInfo>,
}

/// Information about a single tool function
#[derive(Debug, Clone)]
pub struct ToolFunctionInfo {
    /// Function name (same as MCP tool name)
    pub name: String,
    /// Function description
    pub description: Option<String>,
    /// Parameter schema (JSON Schema)
    pub parameters: serde_json::Value,
}

// ========== Tool Registry ==========

/// Central registry for all tools in Plugable Chat
pub struct ToolRegistry {
    /// Built-in tools (python_execution, tool_search)
    internal_tools: Vec<ToolSchema>,
    /// Domain tools from MCP servers (indexed by server_id___tool_name)
    domain_tools: HashMap<String, ToolSchema>,
    /// Precomputed embeddings for tool descriptions (for semantic search)
    tool_embeddings: HashMap<String, Vec<f32>>,
    /// Set of tools that have been materialized (made visible after tool_search)
    materialized_tools: std::collections::HashSet<String>,
    /// Mapping of server_id to python module name
    server_python_names: HashMap<String, String>,
    /// Reverse mapping of python module name to server_id
    python_name_to_server: HashMap<String, String>,
}

impl ToolRegistry {
    /// Create a new tool registry with built-in tools
    pub fn new() -> Self {
        let internal_tools = vec![python_execution_tool(), tool_search_tool()];
        
        Self {
            internal_tools,
            domain_tools: HashMap::new(),
            tool_embeddings: HashMap::new(),
            materialized_tools: std::collections::HashSet::new(),
            server_python_names: HashMap::new(),
            python_name_to_server: HashMap::new(),
        }
    }
    
    /// Register domain tools from an MCP server with its Python module name
    pub fn register_mcp_tools(&mut self, server_id: &str, python_name: &str, tools: &[McpTool], defer: bool) {
        // Store the python_name mapping
        self.server_python_names.insert(server_id.to_string(), python_name.to_string());
        self.python_name_to_server.insert(python_name.to_string(), server_id.to_string());
        
        for tool in tools {
            let key = format!("{}___{}", server_id, tool.name);
            let schema = ToolSchema {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone().unwrap_or(json!({"type": "object", "properties": {}})),
                tool_type: None,
                allowed_callers: if defer {
                    // Deferred tools can only be called from python_execution
                    Some(vec!["python_execution_20251206".to_string()])
                } else {
                    None
                },
                defer_loading: defer,
                embedding: None,
            };
            
            println!("[ToolRegistry] Registered tool: {} (python_module={}, defer={})", key, python_name, defer);
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
        
        // Clean up python name mappings
        if let Some(python_name) = self.server_python_names.remove(server_id) {
            self.python_name_to_server.remove(&python_name);
        }
        
        println!("[ToolRegistry] Unregistered all tools from server: {}", server_id);
    }
    
    /// Get the Python module name for a server
    pub fn get_python_name(&self, server_id: &str) -> Option<&String> {
        self.server_python_names.get(server_id)
    }
    
    /// Get the server ID for a Python module name
    pub fn get_server_for_python_name(&self, python_name: &str) -> Option<&String> {
        self.python_name_to_server.get(python_name)
    }
    
    /// Check if a Python module name is registered (for import validation)
    pub fn is_valid_python_module(&self, module_name: &str) -> bool {
        self.python_name_to_server.contains_key(module_name)
    }
    
    /// Get all registered Python module names
    pub fn get_all_python_modules(&self) -> Vec<&String> {
        self.python_name_to_server.keys().collect()
    }
    
    /// Get materialized tool modules with their function info (for Python docs)
    pub fn get_materialized_tool_modules(&self) -> Vec<ToolModuleInfo> {
        let mut modules: HashMap<String, ToolModuleInfo> = HashMap::new();
        
        for (key, schema) in &self.domain_tools {
            // Only include materialized tools
            if !self.materialized_tools.contains(key) {
                continue;
            }
            
            // Parse server_id from key
            let parts: Vec<&str> = key.splitn(2, "___").collect();
            if parts.len() != 2 {
                continue;
            }
            let server_id = parts[0];
            
            // Get the python name for this server
            let python_name = match self.server_python_names.get(server_id) {
                Some(name) => name.clone(),
                None => continue,
            };
            
            // Get or create the module info
            let module = modules.entry(python_name.clone()).or_insert_with(|| {
                ToolModuleInfo {
                    python_name: python_name.clone(),
                    server_id: server_id.to_string(),
                    functions: Vec::new(),
                }
            });
            
            // Add the function
            module.functions.push(ToolFunctionInfo {
                name: schema.name.clone(),
                description: schema.description.clone(),
                parameters: schema.parameters.clone(),
            });
        }
        
        modules.into_values().collect()
    }
    
    /// Get all tool modules (including non-materialized) for a given set of servers
    pub fn get_all_tool_modules_for_servers(&self, server_ids: &[&str]) -> Vec<ToolModuleInfo> {
        let mut modules: HashMap<String, ToolModuleInfo> = HashMap::new();
        
        for server_id in server_ids {
            let python_name = match self.server_python_names.get(*server_id) {
                Some(name) => name.clone(),
                None => continue,
            };
            
            let prefix = format!("{}___", server_id);
            for (key, schema) in &self.domain_tools {
                if !key.starts_with(&prefix) {
                    continue;
                }
                
                let module = modules.entry(python_name.clone()).or_insert_with(|| {
                    ToolModuleInfo {
                        python_name: python_name.clone(),
                        server_id: (*server_id).to_string(),
                        functions: Vec::new(),
                    }
                });
                
                module.functions.push(ToolFunctionInfo {
                    name: schema.name.clone(),
                    description: schema.description.clone(),
                    parameters: schema.parameters.clone(),
                });
            }
        }
        
        modules.into_values().collect()
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
    
    /// Get all domain tools (for semantic search - includes both deferred and non-deferred)
    pub fn get_all_domain_tools(&self) -> Vec<(&String, &ToolSchema)> {
        self.domain_tools.iter().collect()
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
    
    /// Perform semantic search over all domain tools
    ///
    /// Returns the top-k tools that match the query embeddings, sorted by score.
    /// Searches ALL domain tools (both deferred and non-deferred) so models can
    /// discover relevant tools even if they're already visible.
    pub fn search_tools(
        &self,
        query_embeddings: &[Vec<f32>],
        top_k: usize,
    ) -> Vec<ToolSearchResult> {
        let mut results: Vec<ToolSearchResult> = Vec::new();
        
        for (key, schema) in self.get_all_domain_tools() {
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
                    parameters: schema.parameters.clone(),
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
        assert!(registry.get_internal_tools().iter().any(|t| t.name == "python_execution"));
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
        
        registry.register_mcp_tools("weather_server", "weather", &mcp_tools, false);
        
        assert!(registry.get_tool("weather_server___get_weather").is_some());
        assert_eq!(registry.get_python_name("weather_server"), Some(&"weather".to_string()));
        assert_eq!(registry.get_server_for_python_name("weather"), Some(&"weather_server".to_string()));
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
        
        registry.register_mcp_tools("internal", "internal_tools", &mcp_tools, true);
        
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


