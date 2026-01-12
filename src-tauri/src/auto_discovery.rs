//! Auto-discovery utilities for the agentic loop.
//!
//! This module provides automatic tool and schema discovery based on user prompts.
//! It searches for relevant MCP tools and database schemas before the first turn,
//! giving the model context about available capabilities.

use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use fastembed::TextEmbedding;

use crate::actors::mcp_host_actor::McpTool;
use crate::actors::schema_vector_actor::SchemaVectorMsg;
use crate::settings::DatabaseToolboxConfig;
use crate::tool_registry::SharedToolRegistry;
use crate::tools::schema_search::{SchemaSearchInput, SchemaSearchOutput};
use crate::tools::tool_search::{ToolSearchExecutor, ToolSearchInput, ToolSearchOutput};
use crate::tools::SchemaSearchExecutor;

/// Context returned from auto-discovery operations.
///
/// Contains the results of tool search and schema search, along with
/// the tool schemas that were discovered.
#[derive(Default)]
pub struct AutoDiscoveryContext {
    /// Results from tool search, if enabled and successful
    pub tool_search_output: Option<ToolSearchOutput>,
    /// Results from schema search, if enabled and successful
    pub schema_search_output: Option<SchemaSearchOutput>,
    /// Tool schemas discovered during search, grouped by server
    pub discovered_tool_schemas: Vec<(String, Vec<McpTool>)>,
}

/// Perform automatic tool search based on the user prompt.
///
/// Searches the tool registry for tools relevant to the user's query,
/// returning matching tools and their schemas for inclusion in the system prompt.
pub async fn auto_tool_search_for_prompt(
    prompt: &str,
    tool_search_enabled: bool,
    tool_search_max_results: usize,
    has_mcp_tools: bool,
    filtered_tool_descriptions: &[(String, Vec<McpTool>)],
    registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    materialize: bool,
) -> (Option<ToolSearchOutput>, Vec<(String, Vec<McpTool>)>) {
    if !tool_search_enabled || !has_mcp_tools {
        return (None, Vec::new());
    }

    if prompt.trim().is_empty() {
        println!("[Chat] Auto tool_search skipped: empty user prompt");
        return (None, Vec::new());
    }

    let executor = ToolSearchExecutor::new(registry, embedding_model);
    let search_input = ToolSearchInput {
        queries: vec![prompt.to_string()],
        top_k: tool_search_max_results,
    };

    match executor.execute(search_input).await {
        Ok(output) => {
            if materialize {
                executor.materialize_results(&output.tools).await;
            }
            println!(
                "[Chat] Auto tool_search discovered {} tools before first turn",
                output.tools.len()
            );
            let schemas = map_tool_search_hits_to_schemas(&output.tools, filtered_tool_descriptions);
            (Some(output), schemas)
        }
        Err(e) => {
            println!(
                "[Chat] Auto tool_search failed (continuing without discoveries): {}",
                e
            );
            (None, Vec::new())
        }
    }
}

/// Perform automatic schema search based on the user prompt.
///
/// Searches the schema vector store for database tables relevant to the user's query,
/// returning matching tables and their column information.
pub async fn auto_schema_search_for_prompt(
    prompt: &str,
    schema_search_enabled: bool,
    min_relevance: f32,
    toolbox_config: &DatabaseToolboxConfig,
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
) -> Option<SchemaSearchOutput> {
    // Use a generous cap so we don't silently drop discovered tables
    const AUTO_SCHEMA_SEARCH_MAX_TABLES: usize = 50;
    
    if !schema_search_enabled {
        return None;
    }

    let has_enabled_sources = toolbox_config.enabled
        && toolbox_config
            .sources
            .iter()
            .any(|source| source.enabled);

    if !has_enabled_sources {
        println!("[Chat] Auto schema_search skipped: no enabled database sources");
        return None;
    }

    if prompt.trim().is_empty() {
        println!("[Chat] Auto schema_search skipped: empty user prompt");
        return None;
    }

    let executor = SchemaSearchExecutor::new(schema_tx, embedding_model);
    
    // Check if any tables are cached
    if let Ok(stats) = executor.get_stats().await {
        if stats.table_count == 0 {
            println!("[Chat] Auto schema_search skipped: No tables cached in LanceDB. User needs to click 'Refresh schemas'.");
            return Some(SchemaSearchOutput {
                tables: vec![],
                query_used: prompt.to_string(),
                summary: "WARNING: No database tables are currently cached. You CANNOT write accurate SQL queries yet. Please ask the user to click 'Refresh schemas' in Settings > Schemas to index their databases.".to_string(),
            });
        }
    }

    let input = SchemaSearchInput {
        query: prompt.to_string(),
        max_tables: AUTO_SCHEMA_SEARCH_MAX_TABLES,
        max_columns_per_table: 25,
        min_relevance, 
    };

    let mut search_result = executor.execute(input.clone()).await;

    // Fallback: If semantic search found nothing but we HAVE tables in the cache,
    // and the total number of tables is small (<= 10), just include all of them.
    // This handles cases where table names are cryptic and embeddings are weak.
    if let Ok(ref output) = search_result {
        if output.tables.is_empty() {
            if let Ok(stats) = executor.get_stats().await {
                if stats.table_count > 0 && stats.table_count <= 10 {
                    println!("[Chat] Auto schema_search fallback: semantic match failed (at 30%), but total tables small ({}). Including all tables.", stats.table_count);
                    let fallback_input = SchemaSearchInput {
                        min_relevance: 0.0, // Get everything
                        ..input
                    };
                    search_result = executor.execute(fallback_input).await;
                }
            }
        }
    }

    match search_result {
        Ok(mut output) => {
            // Filter tables by enabled database sources
            let enabled_sources: std::collections::HashSet<String> = toolbox_config
                .sources
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.id.clone())
                .collect();

            output.tables.retain(|t| enabled_sources.contains(&t.source_id));

            println!(
                "[Chat] Auto schema_search found {} table(s) matching prompt (after filtering)",
                output.tables.len()
            );
            if output.tables.is_empty() {
                println!("[Chat] Tip: If you have database sources enabled but see 0 tables, ensure you have clicked 'Refresh schemas' in Settings > Schemas.");
            }
            Some(output)
        }
        Err(e) => {
            println!(
                "[Chat] Auto schema_search failed (continuing without schema context): {}",
                e
            );
            None
        }
    }
}

/// Perform both tool search and schema search for a prompt.
///
/// This is the main entry point for auto-discovery, combining both
/// tool and schema search in a single call.
pub async fn perform_auto_discovery_for_prompt(
    prompt: &str,
    tool_search_enabled: bool,
    tool_search_max_results: usize,
    has_mcp_tools: bool,
    schema_search_enabled: bool,
    schema_relevancy_threshold: f32,
    toolbox_config: &DatabaseToolboxConfig,
    filtered_tool_descriptions: &[(String, Vec<McpTool>)],
    registry: SharedToolRegistry,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    materialize_tools: bool,
) -> AutoDiscoveryContext {
    let (tool_search_output, discovered_tool_schemas) = auto_tool_search_for_prompt(
        prompt,
        tool_search_enabled,
        tool_search_max_results,
        has_mcp_tools,
        filtered_tool_descriptions,
        registry.clone(),
        embedding_model.clone(),
        materialize_tools,
    )
    .await;

    let schema_search_output = auto_schema_search_for_prompt(
        prompt,
        schema_search_enabled,
        schema_relevancy_threshold,
        toolbox_config,
        schema_tx,
        embedding_model,
    )
    .await;

    AutoDiscoveryContext {
        tool_search_output,
        schema_search_output,
        discovered_tool_schemas,
    }
}

/// Map tool search results to their full schema definitions.
///
/// Given the search results (tool names and relevance scores), looks up
/// the full tool definitions from the filtered tool descriptions.
fn map_tool_search_hits_to_schemas(
    hits: &[crate::tool_registry::ToolSearchResult],
    filtered_tool_descriptions: &[(String, Vec<McpTool>)],
) -> Vec<(String, Vec<McpTool>)> {
    use std::collections::HashMap;

    // Build a lookup: server_id -> {tool_name -> tool}
    let mut lookup: HashMap<String, HashMap<String, &McpTool>> = HashMap::new();
    for (server_id, tools) in filtered_tool_descriptions {
        let server_tools = lookup.entry(server_id.clone()).or_default();
        for tool in tools {
            server_tools.insert(tool.name.clone(), tool);
        }
    }

    // Collect matching tools grouped by server
    let mut result: HashMap<String, Vec<McpTool>> = HashMap::new();
    for hit in hits {
        if let Some(server_tools) = lookup.get(&hit.server_id) {
            if let Some(tool) = server_tools.get(&hit.name) {
                result
                    .entry(hit.server_id.clone())
                    .or_default()
                    .push((*tool).clone());
            }
        }
    }

    result.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_map_tool_search_hits_to_schemas() {
        let tool1 = McpTool {
            name: "get_weather".to_string(),
            description: Some("Get weather info".to_string()),
            input_schema: Some(json!({})),
            input_examples: None,
            allowed_callers: None,
        };
        let tool2 = McpTool {
            name: "search".to_string(),
            description: Some("Search the web".to_string()),
            input_schema: Some(json!({})),
            input_examples: None,
            allowed_callers: None,
        };

        let filtered = vec![
            ("weather-server".to_string(), vec![tool1.clone()]),
            ("search-server".to_string(), vec![tool2.clone()]),
        ];

        let hits = vec![
            crate::tool_registry::ToolSearchResult {
                server_id: "weather-server".to_string(),
                name: "get_weather".to_string(),
                description: Some("Get weather info".to_string()),
                parameters: json!({}),
                score: 0.9,
            },
        ];

        let result = map_tool_search_hits_to_schemas(&hits, &filtered);
        
        assert_eq!(result.len(), 1);
        let (server_id, tools) = &result[0];
        assert_eq!(server_id, "weather-server");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_weather");
    }

    #[test]
    fn test_auto_discovery_context_default() {
        let ctx = AutoDiscoveryContext::default();
        assert!(ctx.tool_search_output.is_none());
        assert!(ctx.schema_search_output.is_none());
        assert!(ctx.discovered_tool_schemas.is_empty());
    }
}
