//! Schema Search Implementation
//!
//! Semantic search over cached database schemas using embeddings.
//! This allows models to discover relevant tables and columns dynamically.
//! Returns structured schema information for SQL query construction.

use fastembed::TextEmbedding;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::actors::schema_vector_actor::SchemaVectorMsg;

/// Input for the search_schemas built-in tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSearchInput {
    /// Natural language query describing what data/tables are needed
    pub query: String,
    /// Maximum number of tables to return (default: 5)
    #[serde(default = "default_max_tables")]
    pub max_tables: usize,
    /// Maximum number of columns per table to return (default: 5)
    #[serde(default = "default_max_columns")]
    pub max_columns_per_table: usize,
    /// Minimum relevance score (0.0-1.0) to include a table (default: 0.3)
    #[serde(default = "default_min_score")]
    pub min_relevance: f32,
}

fn default_max_tables() -> usize {
    5
}

fn default_max_columns() -> usize {
    5
}

fn default_min_score() -> f32 {
    0.3
}

/// A table match result for output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMatchOutput {
    /// Fully-qualified table name
    pub table_name: String,
    /// Database source ID
    pub source_id: String,
    /// SQL dialect for this table
    pub sql_dialect: String,
    /// Relevance score (0.0-1.0)
    pub relevance: f32,
    /// Table description if available
    pub description: Option<String>,
    /// Primary key columns
    pub primary_keys: Vec<String>,
    /// Partition columns (for BigQuery, Spanner)
    pub partition_columns: Vec<String>,
    /// Cluster columns (for BigQuery)
    pub cluster_columns: Vec<String>,
    /// Most relevant columns with types
    pub relevant_columns: Vec<ColumnOutput>,
}

/// Column information in output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnOutput {
    pub name: String,
    pub data_type: String,
    pub relevance: f32,
    pub description: Option<String>,
}

/// Output from search_schemas
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSearchOutput {
    /// Matching tables with their schemas
    pub tables: Vec<TableMatchOutput>,
    /// The query that was used
    pub query_used: String,
    /// Summary for the model
    pub summary: String,
}

/// Executor for the search_schemas built-in tool
pub struct SchemaSearchExecutor {
    schema_tx: mpsc::Sender<SchemaVectorMsg>,
    embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
}

impl SchemaSearchExecutor {
    /// Create a new schema search executor
    pub fn new(
        schema_tx: mpsc::Sender<SchemaVectorMsg>,
        embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    ) -> Self {
        Self {
            schema_tx,
            embedding_model,
        }
    }

    /// Execute a schema search
    pub async fn execute(&self, input: SchemaSearchInput) -> Result<SchemaSearchOutput, String> {
        println!(
            "[SchemaSearch] Executing search: '{}' (max_tables={}, min_relevance={})",
            input.query, input.max_tables, input.min_relevance
        );

        if input.query.trim().is_empty() {
            return Err("Search query cannot be empty".to_string());
        }

        // Get the embedding model
        let model_guard = self.embedding_model.read().await;
        let embedding_model = model_guard
            .clone()
            .ok_or_else(|| "Embedding model not initialized".to_string())?;
        drop(model_guard);

        // Generate embedding for the query
        let query_embedding = self.embed_query(&input.query, &embedding_model)?;

        // Search for matching tables
        let (table_tx, table_rx) = oneshot::channel();
        self.schema_tx
            .send(SchemaVectorMsg::SearchTables {
                query_embedding: query_embedding.clone(),
                limit: input.max_tables,
                min_score: input.min_relevance,
                respond_to: table_tx,
            })
            .await
            .map_err(|e| format!("Failed to send search request: {}", e))?;

        let table_results = table_rx
            .await
            .map_err(|_| "Schema vector actor died".to_string())?;

        println!(
            "[SchemaSearch] Found {} tables above threshold",
            table_results.len()
        );

        // For each table, search for relevant columns
        let mut output_tables = Vec::new();

        for table in table_results {
            let (col_tx, col_rx) = oneshot::channel();
            self.schema_tx
                .send(SchemaVectorMsg::SearchColumns {
                    query_embedding: query_embedding.clone(),
                    table_fq_name: Some(table.table_fq_name.clone()),
                    limit: input.max_columns_per_table,
                    respond_to: col_tx,
                })
                .await
                .map_err(|e| format!("Failed to search columns: {}", e))?;

            let column_results = col_rx.await.unwrap_or_default();

            let relevant_columns: Vec<ColumnOutput> = column_results
                .into_iter()
                .map(|c| ColumnOutput {
                    name: c.column_name,
                    data_type: c.data_type,
                    relevance: c.relevance_score,
                    description: c.description,
                })
                .collect();

            output_tables.push(TableMatchOutput {
                table_name: table.table_fq_name.clone(),
                source_id: table.source_id.clone(),
                sql_dialect: table.database_kind.sql_dialect().to_string(),
                relevance: table.relevance_score,
                description: table.description.clone(),
                primary_keys: table.key_columns.clone(),
                partition_columns: table.partition_columns.clone(),
                cluster_columns: table.cluster_columns.clone(),
                relevant_columns,
            });
        }

        // Generate summary
        let summary = self.generate_summary(&output_tables, &input.query);

        Ok(SchemaSearchOutput {
            tables: output_tables,
            query_used: input.query,
            summary,
        })
    }

    /// Embed a query string
    fn embed_query(&self, query: &str, model: &TextEmbedding) -> Result<Vec<f32>, String> {
        model
            .embed(vec![query], None)
            .map_err(|e| format!("Failed to embed query: {}", e))?
            .into_iter()
            .next()
            .ok_or_else(|| "No embedding returned".to_string())
    }

    /// Generate a summary of the search results
    fn generate_summary(&self, tables: &[TableMatchOutput], query: &str) -> String {
        if tables.is_empty() {
            return format!(
                "No tables found matching '{}'. Try a different search query or check that schemas are cached.",
                query
            );
        }

        let mut summary = format!(
            "Found {} table(s) matching '{}':\n\n",
            tables.len(),
            query
        );

        for table in tables {
            summary.push_str(&format!(
                "• {} ({} - {})\n",
                table.table_name, table.sql_dialect, table.source_id
            ));

            if !table.relevant_columns.is_empty() {
                let col_names: Vec<&str> = table
                    .relevant_columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();
                summary.push_str(&format!("  Key columns: {}\n", col_names.join(", ")));
            }

            if !table.partition_columns.is_empty() {
                summary.push_str(&format!(
                    "  Partitioned by: {}\n",
                    table.partition_columns.join(", ")
                ));
            }
        }

        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_search_input_defaults() {
        let json = r#"{"query": "customer orders"}"#;
        let input: SchemaSearchInput = serde_json::from_str(json).unwrap();

        assert_eq!(input.query, "customer orders");
        assert_eq!(input.max_tables, 5);
        assert_eq!(input.max_columns_per_table, 5);
        assert!((input.min_relevance - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_schema_search_output_serde() {
        let output = SchemaSearchOutput {
            tables: vec![TableMatchOutput {
                table_name: "project.dataset.orders".to_string(),
                source_id: "bq-prod".to_string(),
                sql_dialect: "StandardSQL".to_string(),
                relevance: 0.85,
                description: Some("Customer orders".to_string()),
                primary_keys: vec!["order_id".to_string()],
                partition_columns: vec!["order_date".to_string()],
                cluster_columns: vec!["customer_id".to_string()],
                relevant_columns: vec![
                    ColumnOutput {
                        name: "total_amount".to_string(),
                        data_type: "FLOAT64".to_string(),
                        relevance: 0.75,
                        description: None,
                    },
                ],
            }],
            query_used: "orders with total amount".to_string(),
            summary: "Found 1 table".to_string(),
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("orders"));
        assert!(json.contains("StandardSQL"));
    }
}
