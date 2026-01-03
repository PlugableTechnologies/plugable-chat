//! Schema Search Implementation
//!
//! Semantic search over cached database schemas using embeddings.
//! This allows models to discover relevant tables and columns dynamically.
//! Returns structured schema information for SQL query construction.

use fastembed::TextEmbedding;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::actors::schema_vector_actor::{SchemaVectorMsg, SchemaStoreStats};
use crate::settings::CachedColumnSchema;

/// Returns true if the SQL data type is numeric.
/// Used to separate columns for the hybrid column selection strategy:
/// - Non-numeric columns are always included
/// - Numeric columns are selected via semantic search
pub fn is_numeric_data_type(data_type: &str) -> bool {
    let upper = data_type.to_uppercase();
    // Check for common numeric type patterns across SQL dialects
    upper.contains("INT") ||       // INTEGER, INT, INT64, BIGINT, SMALLINT, TINYINT
    upper.contains("FLOAT") ||     // FLOAT, FLOAT64, FLOAT32
    upper.contains("DOUBLE") ||    // DOUBLE, DOUBLE PRECISION
    upper.contains("DECIMAL") ||   // DECIMAL
    upper.contains("NUMERIC") ||   // NUMERIC
    upper.contains("NUMBER") ||    // NUMBER (Oracle)
    upper.contains("REAL") ||      // REAL
    upper == "MONEY" ||            // MONEY (SQL Server)
    upper == "SMALLMONEY"          // SMALLMONEY (SQL Server)
}

/// Hybrid column selection: all non-numeric columns + top N numeric columns.
/// 
/// This is the shared column selection strategy used by both:
/// - The `schema_search` tool
/// - Attached table schema formatting
/// 
/// # Arguments
/// * `columns` - All columns from the table schema
/// * `semantic_numeric_names` - Optional set of semantically relevant numeric column names
///   (from vector search). If None, numeric columns are included in order up to the limit.
/// * `max_numeric_columns` - Maximum number of numeric columns to include
/// 
/// # Returns
/// A tuple of (selected_columns, numeric_count, non_numeric_count) where selected_columns
/// contains references to the columns that should be included.
pub fn select_columns_hybrid<'a>(
    columns: &'a [CachedColumnSchema],
    semantic_numeric_names: Option<&std::collections::HashSet<String>>,
    max_numeric_columns: usize,
) -> (Vec<&'a CachedColumnSchema>, usize, usize) {
    use std::collections::HashSet;
    
    // Separate columns into numeric vs non-numeric
    let (numeric_cols, non_numeric_cols): (Vec<_>, Vec<_>) = columns
        .iter()
        .partition(|c| is_numeric_data_type(&c.data_type));
    
    let non_numeric_count = non_numeric_cols.len();
    let mut selected: Vec<&CachedColumnSchema> = non_numeric_cols;
    
    // For numeric columns: use semantic ordering if provided, otherwise use original order
    let numeric_to_add: Vec<&CachedColumnSchema> = if let Some(semantic_names) = semantic_numeric_names {
        // First add semantically relevant numeric columns (in order of appearance)
        let mut ordered: Vec<&CachedColumnSchema> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        
        // Add semantic matches first
        for col in &numeric_cols {
            if semantic_names.contains(&col.name) && !seen.contains(col.name.as_str()) {
                ordered.push(col);
                seen.insert(&col.name);
            }
        }
        
        // Fill remaining slots with other numeric columns
        for col in &numeric_cols {
            if !seen.contains(col.name.as_str()) && ordered.len() < max_numeric_columns {
                ordered.push(col);
                seen.insert(&col.name);
            }
        }
        
        ordered.into_iter().take(max_numeric_columns).collect()
    } else {
        // No semantic filtering - take first N numeric columns
        numeric_cols.into_iter().take(max_numeric_columns).collect()
    };
    
    let numeric_count = numeric_to_add.len();
    selected.extend(numeric_to_add);
    
    (selected, numeric_count, non_numeric_count)
}

/// Input for the schema_search built-in tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSearchInput {
    /// Natural language query describing what data/tables are needed
    #[serde(alias = "queries")]
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
    10
}

fn default_min_score() -> f32 {
    0.4
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
    /// Special attributes: "primary_key", "foreign_key", "partition", "cluster"
    #[serde(default)]
    pub special_attributes: Vec<String>,
    /// Top 3 most common values with percentage (e.g., "THEFT (23.5%)")
    #[serde(default)]
    pub top_values: Vec<String>,
}

impl ColumnOutput {
    /// Create a ColumnOutput from a CachedColumnSchema with a given relevance score
    pub fn from_cached_column_schema(col: &CachedColumnSchema, relevance: f32) -> Self {
        Self {
            name: col.name.clone(),
            data_type: col.data_type.clone(),
            relevance,
            description: col.description.clone(),
            special_attributes: col.special_attributes.clone(),
            top_values: col.top_values.clone(),
        }
    }
}

/// Output from schema_search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSearchOutput {
    /// Matching tables with their schemas
    pub tables: Vec<TableMatchOutput>,
    /// The query that was used
    pub query_used: String,
    /// Summary for the model
    pub summary: String,
}

/// Executor for the schema_search built-in tool
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

    /// Check if the schema store is empty
    pub async fn get_stats(&self) -> Result<SchemaStoreStats, String> {
        let (tx, rx) = oneshot::channel();
        self.schema_tx
            .send(SchemaVectorMsg::GetStats { respond_to: tx })
            .await
            .map_err(|e| format!("Failed to send stats request: {}", e))?;

        Ok(rx.await.map_err(|_| "Schema vector actor died".to_string())?)
    }

    /// Execute a schema search
    pub async fn execute(&self, input: SchemaSearchInput) -> Result<SchemaSearchOutput, String> {
        // Use the input min_relevance directly
        let min_relevance = input.min_relevance;

        println!(
            "[SchemaSearch] Executing search: '{}' (max_tables={}, min_relevance={})",
            input.query, input.max_tables, min_relevance
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
                min_score: min_relevance,
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

        // For each table, use hybrid column selection:
        // - Include ALL non-numeric columns (TEXT, DATE, BOOLEAN, etc.)
        // - Semantic search on numeric columns, limited by max_columns_per_table
        let mut output_tables = Vec::new();

        for table in table_results {
            // 1. Get full table schema with all columns
            let (schema_tx, schema_rx) = oneshot::channel();
            self.schema_tx
                .send(SchemaVectorMsg::GetTableSchema {
                    table_fq_name: table.table_fq_name.clone(),
                    respond_to: schema_tx,
                })
                .await
                .map_err(|e| format!("Failed to get table schema: {}", e))?;

            let full_schema = schema_rx.await.unwrap_or(None);

            let relevant_columns = if let Some(schema) = full_schema {
                // 2. Separate columns into numeric vs non-numeric
                let (numeric_cols, non_numeric_cols): (Vec<_>, Vec<_>) = schema
                    .columns
                    .iter()
                    .partition(|c| is_numeric_data_type(&c.data_type));

                // 3. Include ALL non-numeric columns with high relevance (1.0)
                let mut columns: Vec<ColumnOutput> = non_numeric_cols
                    .into_iter()
                    .map(|c| ColumnOutput::from_cached_column_schema(c, 1.0))
                    .collect();

                println!(
                    "[SchemaSearch] Table '{}': {} non-numeric columns included",
                    table.table_fq_name,
                    columns.len()
                );

                // 4. Semantic search on numeric columns, limited by max_columns_per_table
                if !numeric_cols.is_empty() {
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

                    // Filter to only include numeric columns from the search results
                    let numeric_col_names: std::collections::HashSet<_> =
                        numeric_cols.iter().map(|c| c.name.as_str()).collect();

                    let numeric_search_results: Vec<ColumnOutput> = column_results
                        .into_iter()
                        .filter(|c| numeric_col_names.contains(c.column_name.as_str()))
                        .take(input.max_columns_per_table)
                        .map(|c| ColumnOutput {
                            name: c.column_name,
                            data_type: c.data_type,
                            relevance: c.relevance_score,
                            description: c.description,
                            special_attributes: c.special_attributes,
                            top_values: c.top_values,
                        })
                        .collect();

                    println!(
                        "[SchemaSearch] Table '{}': {} numeric columns selected (of {} total numeric)",
                        table.table_fq_name,
                        numeric_search_results.len(),
                        numeric_cols.len()
                    );

                    columns.extend(numeric_search_results);
                }

                columns
            } else {
                // Fallback: if we can't get the full schema, use old behavior
                println!(
                    "[SchemaSearch] WARNING: Could not get full schema for '{}', using fallback",
                    table.table_fq_name
                );
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
                column_results
                    .into_iter()
                    .map(|c| ColumnOutput {
                        name: c.column_name,
                        data_type: c.data_type,
                        relevance: c.relevance_score,
                        description: c.description,
                        special_attributes: c.special_attributes,
                        top_values: c.top_values,
                    })
                    .collect()
            };

            output_tables.push(TableMatchOutput {
                table_name: table.table_fq_name.clone(),
                source_id: table.source_id.clone(),
                sql_dialect: table.sql_dialect.clone(),
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
        assert!((input.min_relevance - 0.4).abs() < 0.01);
    }

    #[test]
    fn test_schema_search_output_serde() {
        let output = SchemaSearchOutput {
            tables: vec![TableMatchOutput {
                table_name: "project.dataset.orders".to_string(),
                source_id: "bq-prod".to_string(),
                sql_dialect: "GoogleSQL".to_string(),
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
                        special_attributes: Vec::new(),
                        top_values: vec!["100.00 (15%)".to_string()],
                    },
                ],
            }],
            query_used: "orders with total amount".to_string(),
            summary: "Found 1 table".to_string(),
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("orders"));
        assert!(json.contains("GoogleSQL"));
    }
}
