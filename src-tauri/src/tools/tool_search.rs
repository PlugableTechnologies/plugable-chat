//! Tool Search Implementation
//!
//! Semantic search over available tools using embeddings.
//! This allows models to discover relevant tools dynamically.

use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use fastembed::TextEmbedding;

use crate::tool_registry::{SharedToolRegistry, ToolSearchResult};

/// Input for the tool_search built-in tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchInput {
    /// Semantic search queries describing what tools are needed
    pub queries: Vec<String>,
    /// Maximum number of tools to return (default: 10)
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    10
}

/// Output from tool_search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchOutput {
    /// Tools found matching the queries
    pub tools: Vec<ToolSearchResult>,
    /// Query that was used
    pub queries_used: Vec<String>,
}

/// Executor for the tool_search built-in tool
pub struct ToolSearchExecutor {
    registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
}

impl ToolSearchExecutor {
    /// Create a new tool search executor
    pub fn new(
        registry: SharedToolRegistry,
        embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    ) -> Self {
        Self {
            registry,
            embedding_model,
        }
    }
    
    /// Execute a tool search
    pub async fn execute(&self, input: ToolSearchInput) -> Result<ToolSearchOutput, String> {
        println!("[ToolSearch] Executing with {} queries, top_k={}", input.queries.len(), input.top_k);
        
        if input.queries.is_empty() {
            return Err("At least one search query is required".to_string());
        }
        
        // Get the embedding model
        let model_guard = self.embedding_model.read().await;
        let embedding_model = model_guard.clone()
            .ok_or_else(|| "Embedding model not initialized".to_string())?;
        drop(model_guard);
        
        // Generate embeddings for all queries
        let query_embeddings = self.embed_queries(&input.queries, &embedding_model).await?;
        
        // Search the registry
        let registry = self.registry.read().await;
        let results = registry.search_tools(&query_embeddings, input.top_k);
        
        println!("[ToolSearch] Found {} matching tools", results.len());
        for result in &results {
            println!("[ToolSearch]   - {} (score: {:.3})", result.name, result.score);
        }
        
        Ok(ToolSearchOutput {
            tools: results,
            queries_used: input.queries,
        })
    }
    
    /// Materialize discovered tools (make them visible to the model)
    pub async fn materialize_results(&self, results: &[ToolSearchResult]) {
        let mut registry = self.registry.write().await;
        for result in results {
            let key = format!("{}___{}", result.server_id, result.name);
            registry.materialize_tool(&key);
        }
    }
    
    /// Generate embeddings for query strings
    async fn embed_queries(
        &self,
        queries: &[String],
        model: &Arc<TextEmbedding>,
    ) -> Result<Vec<Vec<f32>>, String> {
        let queries_clone: Vec<String> = queries.to_vec();
        let model_clone = Arc::clone(model);
        
        let result = tokio::task::spawn_blocking(move || {
            model_clone.embed(queries_clone, None)
        })
        .await
        .map_err(|e| format!("Embedding task panicked: {}", e))?
        .map_err(|e| format!("Embedding generation failed: {}", e))?;
        
        Ok(result)
    }
}

/// Pre-compute embeddings for all tools in the registry
pub async fn precompute_tool_embeddings(
    registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
) -> Result<usize, String> {
    println!("[ToolSearch] Pre-computing tool embeddings...");
    
    // Get the embedding model
    let model_guard = embedding_model.read().await;
    let model = model_guard.clone()
        .ok_or_else(|| "Embedding model not initialized".to_string())?;
    drop(model_guard);
    
    // Get all deferred tools that need embeddings
    let tools_to_embed: Vec<(String, String)> = {
        let registry_guard = registry.read().await;
        registry_guard.get_deferred_tools()
            .iter()
            .map(|(key, schema)| {
                // Create embedding text from name and description
                let text = format!(
                    "{}: {}",
                    schema.name,
                    schema.description.as_deref().unwrap_or("")
                );
                ((*key).clone(), text)
            })
            .collect()
    };
    
    if tools_to_embed.is_empty() {
        println!("[ToolSearch] No deferred tools to embed");
        return Ok(0);
    }
    
    println!("[ToolSearch] Embedding {} tools...", tools_to_embed.len());
    
    // Generate embeddings in batch
    let texts: Vec<String> = tools_to_embed.iter().map(|(_, t)| t.clone()).collect();
    let model_clone = Arc::clone(&model);
    
    let embeddings = tokio::task::spawn_blocking(move || {
        model_clone.embed(texts, None)
    })
    .await
    .map_err(|e| format!("Embedding task panicked: {}", e))?
    .map_err(|e| format!("Embedding generation failed: {}", e))?;
    
    // Store embeddings in the registry
    {
        let mut registry_guard = registry.write().await;
        for ((key, _), embedding) in tools_to_embed.iter().zip(embeddings.into_iter()) {
            registry_guard.set_tool_embedding(key, embedding);
        }
    }
    
    let count = tools_to_embed.len();
    println!("[ToolSearch] Pre-computed {} tool embeddings", count);
    
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_registry::ToolRegistry;
    use crate::actors::mcp_host_actor::McpTool;
    use serde_json::json;
    
    #[test]
    fn test_tool_search_input_parsing() {
        let input: ToolSearchInput = serde_json::from_value(json!({
            "queries": ["weather forecast", "temperature"],
            "top_k": 5
        })).unwrap();
        
        assert_eq!(input.queries.len(), 2);
        assert_eq!(input.top_k, 5);
    }
    
    #[test]
    fn test_tool_search_input_default_top_k() {
        let input: ToolSearchInput = serde_json::from_value(json!({
            "queries": ["test"]
        })).unwrap();
        
        assert_eq!(input.top_k, 10);
    }
}


