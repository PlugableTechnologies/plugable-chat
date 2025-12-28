//! Schema Vector Store Actor - manages database schema embeddings in LanceDB
//!
//! This actor handles:
//! - Storing cached table and column schemas with embeddings
//! - Searching schemas by embedding similarity
//! - Managing schema cache lifecycle

use arrow_array::types::Float32Type;
use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::StreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, Table};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::is_verbose_logging_enabled;
use crate::settings::{CachedColumnSchema, CachedTableSchema, SupportedDatabaseKind};

/// Embedding dimension (matches fastembed BGE-Base-EN-v1.5)
pub const SCHEMA_EMBEDDING_DIM: i32 = 768;

/// Messages for the Schema Vector Store Actor
#[derive(Debug)]
pub enum SchemaVectorMsg {
    /// Cache a table schema with its embedding
    CacheTableSchema {
        schema: CachedTableSchema,
        table_embedding: Vec<f32>,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Cache a column schema with its embedding
    CacheColumnSchema {
        table_fq_name: String,
        source_id: String,
        column: CachedColumnSchema,
        column_embedding: Vec<f32>,
        chunk_key: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Search for relevant tables by embedding
    SearchTables {
        query_embedding: Vec<f32>,
        limit: usize,
        min_score: f32,
        respond_to: oneshot::Sender<Vec<SchemaSearchResult>>,
    },
    /// Search for relevant columns by embedding
    SearchColumns {
        query_embedding: Vec<f32>,
        table_fq_name: Option<String>,
        limit: usize,
        respond_to: oneshot::Sender<Vec<ColumnSearchResult>>,
    },
    /// Get all cached tables for a source
    GetTablesForSource {
        source_id: String,
        respond_to: oneshot::Sender<Vec<CachedTableSchema>>,
    },
    /// Enable or disable a specific table
    SetTableEnabled {
        table_fq_name: String,
        enabled: bool,
        respond_to: oneshot::Sender<Result<CachedTableSchema, String>>,
    },
    /// Clear all schemas for a source
    ClearSource {
        source_id: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Clear all schemas
    ClearAll {
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Get statistics (table count, etc.)
    GetStats {
        respond_to: oneshot::Sender<SchemaStoreStats>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaStoreStats {
    pub table_count: usize,
    pub column_count: usize,
}

/// Result from table schema search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSearchResult {
    pub table_fq_name: String,
    pub source_id: String,
    pub database_kind: SupportedDatabaseKind,
    pub sql_dialect: String,
    pub relevance_score: f32,
    pub description: Option<String>,
    pub key_columns: Vec<String>,
    pub partition_columns: Vec<String>,
    pub cluster_columns: Vec<String>,
}

/// Result from column search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSearchResult {
    pub table_fq_name: String,
    pub column_name: String,
    pub data_type: String,
    pub relevance_score: f32,
    pub description: Option<String>,
}

/// Schema Vector Store Actor
pub struct SchemaVectorStoreActor {
    rx: mpsc::Receiver<SchemaVectorMsg>,
    tables_table: Table,
    columns_table: Table,
}

impl SchemaVectorStoreActor {
    /// Create a new Schema Vector Store Actor
    pub async fn new(rx: mpsc::Receiver<SchemaVectorMsg>, db_path: &str) -> Self {
        let db_connection = connect(db_path)
            .execute()
            .await
            .expect("Failed to connect to LanceDB for schemas");

        // Ensure tables exist
        let tables_table = ensure_tables_table_schema(&db_connection).await;
        let columns_table = ensure_columns_table_schema(&db_connection).await;

        Self {
            rx,
            tables_table,
            columns_table,
        }
    }

    /// Run the actor's message loop
    pub async fn run(mut self) {

        while let Some(msg) = self.rx.recv().await {
            let tables_table = self.tables_table.clone();
            let columns_table = self.columns_table.clone();

            tokio::spawn(async move {
                match msg {
                    SchemaVectorMsg::CacheTableSchema {
                        schema,
                        table_embedding,
                        respond_to,
                    } => {
                        let result =
                            upsert_table_schema(&tables_table, &schema, table_embedding).await;
                        let _ = respond_to.send(result);
                    }
                    SchemaVectorMsg::CacheColumnSchema {
                        table_fq_name,
                        source_id,
                        column,
                        column_embedding,
                        chunk_key,
                        respond_to,
                    } => {
                        let result = upsert_column_schema(
                            &columns_table,
                            &table_fq_name,
                            &source_id,
                            &column,
                            column_embedding,
                            &chunk_key,
                        )
                        .await;
                        let _ = respond_to.send(result);
                    }
                    SchemaVectorMsg::SearchTables {
                        query_embedding,
                        limit,
                        min_score,
                        respond_to,
                    } => {
                        let results =
                            search_tables(&tables_table, query_embedding, limit, min_score).await;
                        let _ = respond_to.send(results);
                    }
                    SchemaVectorMsg::SearchColumns {
                        query_embedding,
                        table_fq_name,
                        limit,
                        respond_to,
                    } => {
                        let results = search_columns(
                            &columns_table,
                            query_embedding,
                            table_fq_name.as_deref(),
                            limit,
                        )
                        .await;
                        let _ = respond_to.send(results);
                    }
                    SchemaVectorMsg::GetTablesForSource {
                        source_id,
                        respond_to,
                    } => {
                        let results = get_tables_for_source(&tables_table, &source_id).await;
                        let _ = respond_to.send(results);
                    }
                    SchemaVectorMsg::SetTableEnabled {
                        table_fq_name,
                        enabled,
                        respond_to,
                    } => {
                        let result =
                            set_table_enabled(&tables_table, &table_fq_name, enabled).await;
                        let _ = respond_to.send(result);
                    }
                    SchemaVectorMsg::ClearSource {
                        source_id,
                        respond_to,
                    } => {
                        let result =
                            clear_source(&tables_table, &columns_table, &source_id).await;
                        let _ = respond_to.send(result);
                    }
                    SchemaVectorMsg::ClearAll { respond_to } => {
                        let result = clear_all(&tables_table, &columns_table).await;
                        let _ = respond_to.send(result);
                    }
                    SchemaVectorMsg::GetStats { respond_to } => {
                        let table_count = tables_table.count_rows(None).await.unwrap_or(0);
                        let column_count = columns_table.count_rows(None).await.unwrap_or(0);
                        let _ = respond_to.send(SchemaStoreStats {
                            table_count,
                            column_count,
                        });
                    }
                }
            });
        }

        println!("[SchemaVectorActor] Stopped");
    }
}

// ========== Schema Definitions ==========

fn tables_table_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("table_fq_name", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("database_kind", DataType::Utf8, false),
        Field::new("sql_dialect", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, true),
        Field::new("key_columns", DataType::Utf8, false), // JSON array
        Field::new("partition_columns", DataType::Utf8, false), // JSON array
        Field::new("cluster_columns", DataType::Utf8, false), // JSON array
        Field::new("enabled", DataType::Boolean, false),
        Field::new("columns_json", DataType::Utf8, false), // Full column data
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                SCHEMA_EMBEDDING_DIM,
            ),
            true,
        ),
    ]))
}

fn columns_table_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("column_id", DataType::Utf8, false), // table_fq_name::column_name
        Field::new("table_fq_name", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("data_type", DataType::Utf8, false),
        Field::new("nullable", DataType::Boolean, false),
        Field::new("description", DataType::Utf8, true),
        Field::new("chunk_key", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                SCHEMA_EMBEDDING_DIM,
            ),
            true,
        ),
    ]))
}

async fn ensure_tables_table_schema(db_connection: &Connection) -> Table {
    let expected_schema = tables_table_schema();
    let table_name = "schema_tables";

    match db_connection.open_table(table_name).execute().await {
        Ok(table) => {
            // Check schema compatibility
            match table.schema().await {
                Ok(existing) => {
                    let existing_field_count = existing.fields().len();
                    let expected_field_count = expected_schema.fields().len();

                    // Check vector dimension
                    let existing_dim = existing
                        .field_with_name("vector")
                        .ok()
                        .and_then(|f| match f.data_type() {
                            DataType::FixedSizeList(_, dim) => Some(*dim),
                            _ => None,
                        });

                    let expected_dim = Some(SCHEMA_EMBEDDING_DIM);

                    if existing_field_count != expected_field_count || existing_dim != expected_dim {
                        println!(
                            "[SchemaVectorActor] Table '{}' schema mismatch (Dim: {:?} -> {:?}, Fields: {} -> {}), recreating...",
                            table_name,
                            existing_dim,
                            expected_dim,
                            existing_field_count,
                            expected_field_count
                        );
                        let _ = db_connection.drop_table(table_name).await;
                        create_empty_table(db_connection, table_name, expected_schema).await
                    } else {
                        table
                    }
                }
                Err(_) => table,
            }
        }
        Err(_) => create_empty_table(db_connection, table_name, expected_schema).await,
    }
}

async fn ensure_columns_table_schema(db_connection: &Connection) -> Table {
    let expected_schema = columns_table_schema();
    let table_name = "schema_columns";

    match db_connection.open_table(table_name).execute().await {
        Ok(table) => {
            match table.schema().await {
                Ok(existing) => {
                    let existing_field_count = existing.fields().len();
                    let expected_field_count = expected_schema.fields().len();

                    // Check vector dimension
                    let existing_dim = existing
                        .field_with_name("vector")
                        .ok()
                        .and_then(|f| match f.data_type() {
                            DataType::FixedSizeList(_, dim) => Some(*dim),
                            _ => None,
                        });

                    let expected_dim = Some(SCHEMA_EMBEDDING_DIM);

                    if existing_field_count != expected_field_count || existing_dim != expected_dim {
                        println!(
                            "[SchemaVectorActor] Table '{}' schema mismatch (Dim: {:?} -> {:?}, Fields: {} -> {}), recreating...",
                            table_name,
                            existing_dim,
                            expected_dim,
                            existing_field_count,
                            expected_field_count
                        );
                        let _ = db_connection.drop_table(table_name).await;
                        create_empty_table(db_connection, table_name, expected_schema).await
                    } else {
                        table
                    }
                }
                Err(_) => table,
            }
        }
        Err(_) => create_empty_table(db_connection, table_name, expected_schema).await,
    }
}

async fn create_empty_table(db_connection: &Connection, name: &str, schema: Arc<Schema>) -> Table {
    println!("[SchemaVectorActor] Creating table '{}'", name);
    let batch = RecordBatch::new_empty(schema.clone());
    db_connection
        .create_table(
            name,
            RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema),
        )
        .execute()
        .await
        .expect(&format!("Failed to create {} table", name))
}

// ========== Upsert Operations ==========

async fn upsert_table_schema(
    table: &Table,
    schema: &CachedTableSchema,
    embedding: Vec<f32>,
) -> Result<(), String> {
    let table_schema = tables_table_schema();

    let fq_name_array = StringArray::from(vec![schema.fully_qualified_name.clone()]);
    let source_id_array = StringArray::from(vec![schema.source_id.clone()]);
    let kind_array = StringArray::from(vec![format!("{:?}", schema.kind).to_lowercase()]);
    let dialect_array = StringArray::from(vec![schema.sql_dialect.clone()]);
    let desc_array = StringArray::from(vec![schema.description.clone().unwrap_or_default()]);
    let key_cols_array =
        StringArray::from(vec![serde_json::to_string(&schema.primary_keys).unwrap_or_default()]);
    let partition_cols_array = StringArray::from(vec![
        serde_json::to_string(&schema.partition_columns).unwrap_or_default()
    ]);
    let cluster_cols_array = StringArray::from(vec![
        serde_json::to_string(&schema.cluster_columns).unwrap_or_default()
    ]);
    let enabled_array = BooleanArray::from(vec![schema.enabled]);
    let columns_json_array =
        StringArray::from(vec![serde_json::to_string(&schema.columns).unwrap_or_default()]);

    let vector_values = Float32Array::from(embedding);
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vec![Some(
            vector_values
                .values()
                .iter()
                .map(|v| Some(*v))
                .collect::<Vec<_>>(),
        )],
        SCHEMA_EMBEDDING_DIM,
    );

    let batch = RecordBatch::try_new(
        table_schema,
        vec![
            Arc::new(fq_name_array),
            Arc::new(source_id_array),
            Arc::new(kind_array),
            Arc::new(dialect_array),
            Arc::new(desc_array),
            Arc::new(key_cols_array),
            Arc::new(partition_cols_array),
            Arc::new(cluster_cols_array),
            Arc::new(enabled_array),
            Arc::new(columns_json_array),
            Arc::new(vector_array),
        ],
    )
    .map_err(|e| format!("Failed to create batch: {}", e))?;

    // Delete existing and insert new
    let filter = format!("table_fq_name = '{}'", schema.fully_qualified_name);
    let _ = table.delete(&filter).await;

    table
        .add(Box::new(RecordBatchIterator::new(
            vec![Ok(batch)],
            tables_table_schema(),
        )))
        .execute()
        .await
        .map_err(|e| format!("Failed to add table schema: {}", e))?;

    println!(
        "[SchemaVectorActor] Cached table schema: {}",
        schema.fully_qualified_name
    );
    Ok(())
}

async fn upsert_column_schema(
    table: &Table,
    table_fq_name: &str,
    source_id: &str,
    column: &CachedColumnSchema,
    embedding: Vec<f32>,
    chunk_key: &str,
) -> Result<(), String> {
    let column_schema = columns_table_schema();
    let column_id = format!("{}::{}", table_fq_name, column.name);

    let id_array = StringArray::from(vec![column_id.clone()]);
    let table_array = StringArray::from(vec![table_fq_name.to_string()]);
    let source_array = StringArray::from(vec![source_id.to_string()]);
    let name_array = StringArray::from(vec![column.name.clone()]);
    let type_array = StringArray::from(vec![column.data_type.clone()]);
    let nullable_array = BooleanArray::from(vec![column.nullable]);
    let desc_array = StringArray::from(vec![column.description.clone().unwrap_or_default()]);
    let chunk_array = StringArray::from(vec![chunk_key.to_string()]);

    let vector_values = Float32Array::from(embedding);
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vec![Some(
            vector_values
                .values()
                .iter()
                .map(|v| Some(*v))
                .collect::<Vec<_>>(),
        )],
        SCHEMA_EMBEDDING_DIM,
    );

    let batch = RecordBatch::try_new(
        column_schema,
        vec![
            Arc::new(id_array),
            Arc::new(table_array),
            Arc::new(source_array),
            Arc::new(name_array),
            Arc::new(type_array),
            Arc::new(nullable_array),
            Arc::new(desc_array),
            Arc::new(chunk_array),
            Arc::new(vector_array),
        ],
    )
    .map_err(|e| format!("Failed to create column batch: {}", e))?;

    // Delete existing and insert new
    let filter = format!("column_id = '{}'", column_id);
    let _ = table.delete(&filter).await;

    table
        .add(Box::new(RecordBatchIterator::new(
            vec![Ok(batch)],
            columns_table_schema(),
        )))
        .execute()
        .await
        .map_err(|e| format!("Failed to add column schema: {}", e))?;

    Ok(())
}

// ========== Search Operations ==========

async fn search_tables(
    table: &Table,
    query_embedding: Vec<f32>,
    limit: usize,
    min_score: f32,
) -> Vec<SchemaSearchResult> {
    let mut query_builder = match table.query().nearest_to(query_embedding) {
        Ok(q) => q,
        Err(e) => {
            println!("[SchemaVectorActor] Failed to create vector query: {}", e);
            return vec![];
        }
    };

    query_builder = query_builder.only_if("enabled = true");

    let mut results = vec![];
    let mut stream = match query_builder.limit(limit).execute().await {
        Ok(s) => s,
        Err(e) => {
            println!("[SchemaVectorActor] Failed to execute query: {}", e);
            return vec![];
        }
    };

    while let Some(batch_result) = stream.next().await {
        let batch = match batch_result {
            Ok(b) => b,
            Err(_) => continue,
        };

        let fq_names = batch
            .column_by_name("table_fq_name")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let source_ids = batch
            .column_by_name("source_id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let kinds = batch
            .column_by_name("database_kind")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let dialects = batch
            .column_by_name("sql_dialect")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let descriptions = batch
            .column_by_name("description")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let key_cols = batch
            .column_by_name("key_columns")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let partition_cols = batch
            .column_by_name("partition_columns")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let cluster_cols = batch
            .column_by_name("cluster_columns")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let _enabled_col = batch
            .column_by_name("enabled")
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>());
        let distances = batch
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

        if let (Some(fq), Some(src), Some(k)) = (fq_names, source_ids, kinds) {
            for i in 0..batch.num_rows() {
                let distance = distances.map(|d| d.value(i)).unwrap_or(0.0);
                let score = 1.0 / (1.0 + distance);

                if score < min_score {
                    if is_verbose_logging_enabled() {
                        println!("[SchemaSearch] Skipping result '{}' with low score {:.3} (min={})", fq.value(i), score, min_score);
                    }
                    continue;
                }

                let kind_str = k.value(i);
                let database_kind = match kind_str {
                    "bigquery" => SupportedDatabaseKind::Bigquery,
                    "postgres" => SupportedDatabaseKind::Postgres,
                    "mysql" => SupportedDatabaseKind::Mysql,
                    "sqlite" => SupportedDatabaseKind::Sqlite,
                    "spanner" => SupportedDatabaseKind::Spanner,
                    _ => continue,
                };

                let sql_dialect = if let Some(d) = dialects {
                    d.value(i).to_string()
                } else {
                    database_kind.sql_dialect().to_string()
                };

                results.push(SchemaSearchResult {
                    table_fq_name: fq.value(i).to_string(),
                    source_id: src.value(i).to_string(),
                    database_kind,
                    sql_dialect,
                    relevance_score: score,
                    description: descriptions.map(|d| d.value(i).to_string()).filter(|s| !s.is_empty()),
                    key_columns: key_cols
                        .and_then(|c| serde_json::from_str(c.value(i)).ok())
                        .unwrap_or_default(),
                    partition_columns: partition_cols
                        .and_then(|c| serde_json::from_str(c.value(i)).ok())
                        .unwrap_or_default(),
                    cluster_columns: cluster_cols
                        .and_then(|c| serde_json::from_str(c.value(i)).ok())
                        .unwrap_or_default(),
                });
            }
        }
    }

    results
}

async fn search_columns(
    table: &Table,
    query_embedding: Vec<f32>,
    table_fq_name: Option<&str>,
    limit: usize,
) -> Vec<ColumnSearchResult> {
    let mut query_builder = match table.query().nearest_to(query_embedding) {
        Ok(q) => q,
        Err(e) => {
            println!("[SchemaVectorActor] Failed to create column vector query: {}", e);
            return vec![];
        }
    };

    // Add filter if table specified
    if let Some(fq) = table_fq_name {
        query_builder = query_builder.only_if(format!("table_fq_name = '{}'", fq));
    }

    let mut stream = match query_builder.limit(limit).execute().await {
        Ok(s) => s,
        Err(e) => {
            println!("[SchemaVectorActor] Failed to execute column query: {}", e);
            return vec![];
        }
    };

    let mut results = vec![];

    while let Some(batch_result) = stream.next().await {
        let batch = match batch_result {
            Ok(b) => b,
            Err(_) => continue,
        };

        let tables = batch
            .column_by_name("table_fq_name")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let names = batch
            .column_by_name("column_name")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let types = batch
            .column_by_name("data_type")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let descriptions = batch
            .column_by_name("description")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let distances = batch
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

        if let (Some(t), Some(n), Some(ty)) = (tables, names, types) {
            for i in 0..batch.num_rows() {
                let distance = distances.map(|d| d.value(i)).unwrap_or(0.0);
                let score = 1.0 / (1.0 + distance);

                results.push(ColumnSearchResult {
                    table_fq_name: t.value(i).to_string(),
                    column_name: n.value(i).to_string(),
                    data_type: ty.value(i).to_string(),
                    relevance_score: score,
                    description: descriptions.map(|d| d.value(i).to_string()).filter(|s| !s.is_empty()),
                });
            }
        }
    }

    results
}

async fn set_table_enabled(
    tables: &Table,
    table_fq_name: &str,
    enabled: bool,
) -> Result<CachedTableSchema, String> {
    let query = tables
        .query()
        .only_if(format!("table_fq_name = '{}'", table_fq_name));

    let mut stream = query
        .execute()
        .await
        .map_err(|e| format!("Failed to fetch table for toggle: {}", e))?;

    let batch = match stream.next().await {
        Some(Ok(batch)) => batch,
        _ => return Err(format!("Table not found in cache: {}", table_fq_name)),
    };

    let fq_names = batch
        .column_by_name("table_fq_name")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let source_ids = batch
        .column_by_name("source_id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let kinds = batch
        .column_by_name("database_kind")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let dialects = batch
        .column_by_name("sql_dialect")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let descriptions = batch
        .column_by_name("description")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let columns_json = batch
        .column_by_name("columns_json")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let key_cols = batch
        .column_by_name("key_columns")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let partition_cols = batch
        .column_by_name("partition_columns")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let cluster_cols = batch
        .column_by_name("cluster_columns")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let _enabled_col = batch
        .column_by_name("enabled")
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>());

    let vector_col = batch
        .column_by_name("vector")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .ok_or("Missing vector column for table cache")?;

    if batch.num_rows() == 0 {
        return Err(format!("Table not found in cache: {}", table_fq_name));
    }

    let kind_str = kinds
        .and_then(|k| Some(k.value(0)))
        .ok_or("Missing database kind")?;
    let database_kind = match kind_str {
        "bigquery" => SupportedDatabaseKind::Bigquery,
        "postgres" => SupportedDatabaseKind::Postgres,
        "mysql" => SupportedDatabaseKind::Mysql,
        "sqlite" => SupportedDatabaseKind::Sqlite,
        "spanner" => SupportedDatabaseKind::Spanner,
        _ => return Err(format!("Unsupported database kind: {}", kind_str)),
    };

    let sql_dialect = if let Some(d) = dialects {
        d.value(0).to_string()
    } else {
        database_kind.sql_dialect().to_string()
    };

    let vector_values = vector_col.value(0);
    let values = vector_values
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or("Invalid vector column type")?;
    let embedding: Vec<f32> = (0..values.len()).map(|i| values.value(i)).collect();

    let schema = CachedTableSchema {
        fully_qualified_name: fq_names
            .and_then(|c| Some(c.value(0).to_string()))
            .ok_or("Missing table name")?,
        source_id: source_ids
            .and_then(|c| Some(c.value(0).to_string()))
            .ok_or("Missing source id")?,
        kind: database_kind,
        sql_dialect,
        enabled,
        columns: columns_json
            .and_then(|c| serde_json::from_str(c.value(0)).ok())
            .unwrap_or_default(),
        primary_keys: key_cols
            .and_then(|c| serde_json::from_str(c.value(0)).ok())
            .unwrap_or_default(),
        partition_columns: partition_cols
            .and_then(|c| serde_json::from_str(c.value(0)).ok())
            .unwrap_or_default(),
        cluster_columns: cluster_cols
            .and_then(|c| serde_json::from_str(c.value(0)).ok())
            .unwrap_or_default(),
        description: descriptions
            .and_then(|d| Some(d.value(0).to_string()))
            .filter(|s| !s.is_empty()),
    };

    upsert_table_schema(tables, &schema, embedding).await?;
    Ok(schema)
}

async fn get_tables_for_source(table: &Table, source_id: &str) -> Vec<CachedTableSchema> {
    let query = table
        .query()
        .only_if(format!("source_id = '{}'", source_id));

    let mut stream = match query.execute().await {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let mut results = vec![];

    while let Some(batch_result) = stream.next().await {
        let batch = match batch_result {
            Ok(b) => b,
            Err(_) => continue,
        };

        let fq_names = batch
            .column_by_name("table_fq_name")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let source_ids = batch
            .column_by_name("source_id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let kinds = batch
            .column_by_name("database_kind")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let dialects = batch
            .column_by_name("sql_dialect")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let descriptions = batch
            .column_by_name("description")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let columns_json = batch
            .column_by_name("columns_json")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let key_cols = batch
            .column_by_name("key_columns")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let partition_cols = batch
            .column_by_name("partition_columns")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let cluster_cols = batch
            .column_by_name("cluster_columns")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let enabled_col = batch
            .column_by_name("enabled")
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>());

        if let (Some(fq), Some(src), Some(k), Some(cols)) = (fq_names, source_ids, kinds, columns_json) {
            for i in 0..batch.num_rows() {
                let kind = match k.value(i) {
                    "bigquery" => SupportedDatabaseKind::Bigquery,
                    "postgres" => SupportedDatabaseKind::Postgres,
                    "mysql" => SupportedDatabaseKind::Mysql,
                    "sqlite" => SupportedDatabaseKind::Sqlite,
                    "spanner" => SupportedDatabaseKind::Spanner,
                    _ => continue,
                };

                let columns: Vec<CachedColumnSchema> =
                    serde_json::from_str(cols.value(i)).unwrap_or_default();

                let sql_dialect = if let Some(d) = dialects {
                    d.value(i).to_string()
                } else {
                    kind.sql_dialect().to_string()
                };

                results.push(CachedTableSchema {
                    fully_qualified_name: fq.value(i).to_string(),
                    source_id: src.value(i).to_string(),
                    kind,
                    sql_dialect,
                    columns,
                    primary_keys: key_cols
                        .and_then(|c| serde_json::from_str(c.value(i)).ok())
                        .unwrap_or_default(),
                    partition_columns: partition_cols
                        .and_then(|c| serde_json::from_str(c.value(i)).ok())
                        .unwrap_or_default(),
                    cluster_columns: cluster_cols
                        .and_then(|c| serde_json::from_str(c.value(i)).ok())
                        .unwrap_or_default(),
                    description: descriptions.map(|d| d.value(i).to_string()).filter(|s| !s.is_empty()),
                    enabled: enabled_col.map(|c| c.value(i)).unwrap_or(true),
                });
            }
        }
    }

    results
}

// ========== Clear Operations ==========

async fn clear_source(tables: &Table, columns: &Table, source_id: &str) -> Result<(), String> {
    let filter = format!("source_id = '{}'", source_id);

    tables
        .delete(&filter)
        .await
        .map_err(|e| format!("Failed to clear tables: {}", e))?;

    columns
        .delete(&filter)
        .await
        .map_err(|e| format!("Failed to clear columns: {}", e))?;

    println!(
        "[SchemaVectorActor] Cleared all schemas for source: {}",
        source_id
    );
    Ok(())
}

async fn clear_all(tables: &Table, columns: &Table) -> Result<(), String> {
    // Delete all records (LanceDB filter for "all" is tricky, use always-true filter)
    tables
        .delete("1 = 1")
        .await
        .map_err(|e| format!("Failed to clear tables: {}", e))?;

    columns
        .delete("1 = 1")
        .await
        .map_err(|e| format!("Failed to clear columns: {}", e))?;

    println!("[SchemaVectorActor] Cleared all schema cache");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_search_result_serde() {
        let result = SchemaSearchResult {
            table_fq_name: "project.dataset.orders".to_string(),
            source_id: "bq-prod".to_string(),
            database_kind: SupportedDatabaseKind::Bigquery,
            sql_dialect: "GoogleSQL".to_string(),
            relevance_score: 0.85,
            description: Some("Customer orders".to_string()),
            key_columns: vec!["order_id".to_string()],
            partition_columns: vec!["order_date".to_string()],
            cluster_columns: vec!["customer_id".to_string()],
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: SchemaSearchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(result.table_fq_name, parsed.table_fq_name);
        assert_eq!(result.key_columns, parsed.key_columns);
    }

    #[test]
    fn test_column_search_result_serde() {
        let result = ColumnSearchResult {
            table_fq_name: "project.dataset.orders".to_string(),
            column_name: "total_amount".to_string(),
            data_type: "FLOAT64".to_string(),
            relevance_score: 0.75,
            description: Some("Order total in USD".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ColumnSearchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(result.column_name, parsed.column_name);
    }
}
