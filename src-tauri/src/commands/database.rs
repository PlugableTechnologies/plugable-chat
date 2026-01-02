//! Database schema management Tauri commands.
//!
//! Commands for managing database schema cache, table discovery, and
//! embedding generation for schema search functionality.
//!
//! NOTE: Schema *caching* uses the GPU embedding model (bulk indexing operation),
//! while schema *search* during chat uses the CPU model (avoids LLM eviction).

use crate::actors::database_toolbox_actor::DatabaseToolboxMsg;
use crate::actors::schema_vector_actor::SchemaVectorMsg;
use crate::app_state::{ActorHandles, EmbeddingModelState, SettingsState};
use crate::protocol::FoundryMsg;
use crate::settings::{
    CachedTableSchema, DatabaseSourceConfig, DatabaseToolboxConfig, SupportedDatabaseKind,
};
use fastembed::TextEmbedding;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;

/// Status of a table in the schema cache
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchemaTableStatus {
    pub source_id: String,
    pub source_name: String,
    pub table_fq_name: String,
    pub enabled: bool,
    pub column_count: usize,
    pub description: Option<String>,
}

/// Progress of a schema refresh operation
#[derive(Clone, serde::Serialize)]
pub struct SchemaRefreshProgress {
    pub message: String,
    pub source_name: String,
    pub current_table: Option<String>,
    pub tables_done: usize,
    pub tables_total: usize,
    pub is_complete: bool,
    pub error: Option<String>,
}

/// Result of a table search including relevance score
#[derive(Debug, Clone, serde::Serialize)]
pub struct TableSearchResult {
    pub table: SchemaTableStatus,
    pub relevance_score: f32,
}

/// Status of a database source in the schema cache
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaSourceStatus {
    pub source_id: String,
    pub source_name: String,
    pub database_kind: SupportedDatabaseKind,
    pub tables: Vec<SchemaTableStatus>,
}

/// Summary of a schema refresh operation
#[derive(Debug, Clone)]
pub struct SchemaRefreshSummary {
    pub sources: Vec<SchemaSourceStatus>,
    pub errors: Vec<String>,
}

/// Refresh database schemas for a given configuration
///
/// NOTE: Tries to use GPU embedding model for bulk caching, but falls back to CPU
/// if GPU is busy with LLM operations (prevents memory contention).
pub async fn refresh_database_schemas_for_config(
    app_handle: &AppHandle,
    handles: &State<'_, ActorHandles>,
    embedding_state: &State<'_, EmbeddingModelState>,
    toolbox_config: &DatabaseToolboxConfig,
) -> Result<SchemaRefreshSummary, String> {
    let sources: Vec<DatabaseSourceConfig> = toolbox_config
        .sources
        .iter()
        .cloned()
        .filter(|s| s.enabled)
        .collect();

    if sources.is_empty() {
        return Ok(SchemaRefreshSummary {
            sources: Vec::new(),
            errors: Vec::new(),
        });
    }

    // Try to get GPU embedding model, but fall back to CPU if GPU is busy
    // This prevents GPU memory contention with LLM prewarm/chat operations
    let (embedding_model, using_gpu) = match handles.gpu_guard.mutex.try_lock() {
        Ok(_guard) => {
            // GPU is available - request GPU embedding model
            // Note: The guard is dropped here, but GetGpuEmbeddingModel will acquire it again
            drop(_guard);
            
            let (model_tx, model_rx) = oneshot::channel();
            handles
                .foundry_tx
                .send(FoundryMsg::GetGpuEmbeddingModel {
                    respond_to: model_tx,
                })
                .await
                .map_err(|e| format!("Failed to request GPU embedding model: {}", e))?;

            let model = model_rx
                .await
                .map_err(|_| "Foundry actor died while getting GPU embedding model")?
                .map_err(|e| format!("Failed to load GPU embedding model: {}", e))?;
            
            (model, true)
        }
        Err(_) => {
            // GPU is busy - check what operation is running and fall back to CPU
            let current_op = handles.gpu_guard.current_operation.read().await;
            let op_desc = current_op.clone().unwrap_or_else(|| "unknown operation".to_string());
            drop(current_op);
            
            println!("[SchemaRefresh] GPU busy with '{}', falling back to CPU embeddings", op_desc);
            
            let _ = app_handle.emit(
                "schema-refresh-progress",
                SchemaRefreshProgress {
                    message: format!("GPU busy ({}), using CPU for embeddings", op_desc),
                    source_name: "".to_string(),
                    current_table: None,
                    tables_done: 0,
                    tables_total: 0,
                    is_complete: false,
                    error: None,
                },
            );
            
            // Use CPU embedding model instead
            let model_guard = embedding_state.cpu_model.read().await;
            let model = model_guard
                .clone()
                .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
            drop(model_guard);
            
            (model, false)
        }
    };

    ensure_toolbox_running(&handles.database_toolbox_tx, toolbox_config).await?;

    let mut refreshed_sources = Vec::new();
    let mut errors = Vec::new();

    for source in sources {
        match refresh_schema_cache_for_source(app_handle, handles, &source, embedding_model.clone())
            .await
        {
            Ok(status) => refreshed_sources.push(status),
            Err(err) => {
                let msg = format!("{} ({}): {}", source.name, source.id, err);
                println!(
                    "[SchemaRefresh] Failed to refresh source {} ({}): {}",
                    source.name, source.id, err
                );
                errors.push(msg);
            }
        }
    }

    let _ = app_handle.emit(
        "schema-refresh-progress",
        SchemaRefreshProgress {
            message: "Refresh complete".to_string(),
            source_name: "".to_string(),
            current_table: None,
            tables_done: 0,
            tables_total: 0,
            is_complete: true,
            error: None,
        },
    );

    // Only re-warm LLM if we used GPU embeddings (which may have evicted the LLM)
    if using_gpu {
        println!("[SchemaRefresh] Caching complete (GPU), triggering LLM re-warm");
        let (rewarm_tx, _) = oneshot::channel();
        let _ = handles
            .foundry_tx
            .send(FoundryMsg::RewarmCurrentModel {
                respond_to: rewarm_tx,
            })
            .await;
    } else {
        println!("[SchemaRefresh] Caching complete (CPU), no re-warm needed");
    }

    Ok(SchemaRefreshSummary {
        sources: refreshed_sources,
        errors,
    })
}

/// Refresh database schemas
#[tauri::command]
pub async fn refresh_database_schemas(
    app_handle: AppHandle,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<Vec<SchemaSourceStatus>, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    drop(settings_guard);

    let summary =
        refresh_database_schemas_for_config(&app_handle, &handles, &embedding_state, &toolbox_config).await?;

    if !summary.errors.is_empty() {
        println!(
            "[SchemaRefresh] Completed with errors: {}",
            summary.errors.join("; ")
        );
    }

    Ok(summary.sources)
}

/// Search for relevant database tables using embedding similarity.
/// If query is empty, returns all cached tables in alphabetical order.
/// If query is provided, returns tables ordered by semantic relevance.
#[tauri::command]
pub async fn search_database_tables(
    query: String,
    limit: usize,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<Vec<TableSearchResult>, String> {
    // If query is empty, return all tables alphabetically
    if query.trim().is_empty() {
        return get_all_tables_alphabetically(&handles, &settings_state, limit).await;
    }

    // Use CPU model for schema search during chat (avoids evicting LLM from GPU)
    let model_guard = embedding_state.cpu_model.read().await;
    let embedding_model = model_guard
        .clone()
        .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
    drop(model_guard);

    // Embed the query
    let query_embeddings = embedding_model
        .embed(vec![query], None)
        .map_err(|e| format!("Failed to embed query: {}", e))?;
    let query_vector = query_embeddings
        .into_iter()
        .next()
        .ok_or_else(|| "No embedding returned".to_string())?;

    // Search for tables
    let (tx, rx) = oneshot::channel();
    handles
        .schema_tx
        .send(SchemaVectorMsg::SearchTables {
            query_embedding: query_vector,
            limit,
            min_score: 0.0, // Return all results up to limit, let UI filter if needed
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let search_results = rx
        .await
        .map_err(|_| "Schema vector actor unavailable".to_string())?;

    // Map to TableSearchResult
    let mut results = Vec::new();
    for res in search_results {
        // Fetch full table status to get column count
        let (table_tx, table_rx) = oneshot::channel();
        handles
            .schema_tx
            .send(SchemaVectorMsg::GetTablesForSource {
                source_id: res.source_id.clone(),
                respond_to: table_tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        let source_tables = table_rx
            .await
            .map_err(|_| "Schema vector actor unavailable".to_string())?;

        if let Some(table) = source_tables.into_iter().find(|t| t.fully_qualified_name == res.table_fq_name) {
            results.push(TableSearchResult {
                table: SchemaTableStatus {
                    source_id: res.source_id.clone(),
                    source_name: table.source_id.clone(), // Best we can do without source name lookup
                    table_fq_name: res.table_fq_name.clone(),
                    enabled: table.enabled,
                    column_count: table.columns.len(),
                    description: res.description.clone(),
                },
                relevance_score: res.relevance_score,
            });
        }
    }

    Ok(results)
}

/// Helper: Get all tables from all sources, sorted alphabetically
async fn get_all_tables_alphabetically(
    handles: &State<'_, ActorHandles>,
    settings_state: &State<'_, SettingsState>,
    limit: usize,
) -> Result<Vec<TableSearchResult>, String> {
    let settings_guard = settings_state.settings.read().await;
    let sources: Vec<DatabaseSourceConfig> = settings_guard
        .database_toolbox
        .sources
        .iter()
        .cloned()
        .filter(|s| s.enabled)
        .collect();
    drop(settings_guard);

    let mut all_tables = Vec::new();

    for source in sources {
        let (tx, rx) = oneshot::channel();
        handles
            .schema_tx
            .send(SchemaVectorMsg::GetTablesForSource {
                source_id: source.id.clone(),
                respond_to: tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        let cached_tables = rx
            .await
            .map_err(|_| "Schema vector actor unavailable".to_string())?;

        for table in cached_tables {
            all_tables.push(TableSearchResult {
                table: SchemaTableStatus {
                    source_id: source.id.clone(),
                    source_name: source.name.clone(),
                    table_fq_name: table.fully_qualified_name,
                    enabled: table.enabled,
                    column_count: table.columns.len(),
                    description: table.description,
                },
                relevance_score: 0.0, // No relevance when not searching
            });
        }
    }

    // Sort alphabetically by table name
    all_tables.sort_by(|a, b| a.table.table_fq_name.cmp(&b.table.table_fq_name));

    // Apply limit
    all_tables.truncate(limit);

    Ok(all_tables)
}

/// Get cached database schemas
#[tauri::command]
pub async fn get_cached_database_schemas(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<SchemaSourceStatus>, String> {
    let settings_guard = settings_state.settings.read().await;
    let sources: Vec<DatabaseSourceConfig> = settings_guard
        .database_toolbox
        .sources
        .iter()
        .cloned()
        .filter(|s| s.enabled)
        .collect();
    drop(settings_guard);

    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let mut cached_sources = Vec::new();

    for source in sources {
        let (tx, rx) = oneshot::channel();
        handles
            .schema_tx
            .send(SchemaVectorMsg::GetTablesForSource {
                source_id: source.id.clone(),
                respond_to: tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        let cached_tables = rx
            .await
            .map_err(|_| "Schema vector actor unavailable".to_string())?;

        let table_statuses = cached_tables
            .into_iter()
            .map(|table| SchemaTableStatus {
                source_id: source.id.clone(),
                source_name: source.name.clone(),
                table_fq_name: table.fully_qualified_name,
                enabled: table.enabled,
                column_count: table.columns.len(),
                description: table.description,
            })
            .collect();

        cached_sources.push(SchemaSourceStatus {
            source_id: source.id.clone(),
            source_name: source.name.clone(),
            database_kind: source.kind,
            tables: table_statuses,
        });
    }

    Ok(cached_sources)
}

/// Set whether a table is enabled in the cache
#[tauri::command]
pub async fn set_schema_table_enabled(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
    source_id: String,
    table_fq_name: String,
    enabled: bool,
) -> Result<SchemaTableStatus, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    let source = settings_guard
        .database_toolbox
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .cloned()
        .ok_or_else(|| format!("Source not found: {}", source_id))?;
    let source_name = source.name.clone();
    drop(settings_guard);

    // Try to flip the enabled flag on the cached record
    let (tx, rx) = oneshot::channel();
    handles
        .schema_tx
        .send(crate::actors::schema_vector_actor::SchemaVectorMsg::SetTableEnabled {
            table_fq_name: table_fq_name.clone(),
            enabled,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let toggle_result = rx.await.map_err(|_| "Schema vector actor unavailable".to_string())?;

    let table_schema = match toggle_result {
        Ok(schema) => schema,
        Err(_) => {
            // Table not cached, try to fetch and cache it
            // Use CPU model for schema operations during chat (avoids evicting LLM from GPU)
            let model_guard = embedding_state.cpu_model.read().await;
            let embedding_model = model_guard
                .clone()
                .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
            drop(model_guard);

            ensure_toolbox_running(&handles.database_toolbox_tx, &toolbox_config).await?;

            let schema = fetch_table_schema(&handles.database_toolbox_tx, &source_id, &table_fq_name).await?;

            let (table_emb, col_embs) = embed_table_and_columns(embedding_model, &schema).await?;

            cache_table_and_columns(
                &handles.schema_tx,
                schema.clone(),
                table_emb,
                col_embs,
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
            )
            .await?;

            // Now toggle
            let (tx, rx) = oneshot::channel();
            handles
                .schema_tx
                .send(crate::actors::schema_vector_actor::SchemaVectorMsg::SetTableEnabled {
                    table_fq_name: table_fq_name.clone(),
                    enabled,
                    respond_to: tx,
                })
                .await
                .map_err(|e| e.to_string())?;

            rx.await
                .map_err(|_| "Schema vector actor unavailable".to_string())?
                .map_err(|e| e)?
        }
    };

    Ok(SchemaTableStatus {
        source_id,
        source_name,
        table_fq_name: table_schema.fully_qualified_name,
        enabled: table_schema.enabled,
        column_count: table_schema.columns.len(),
        description: table_schema.description,
    })
}

// ============ Helper Functions ============

/// Ensure the database toolbox process is running
pub async fn ensure_toolbox_running(
    toolbox_tx: &tokio::sync::mpsc::Sender<DatabaseToolboxMsg>,
    config: &DatabaseToolboxConfig,
) -> Result<(), String> {
    if !config.enabled {
        println!(
            "[SchemaRefresh] Database toolbox is disabled in settings; attempting to start anyway"
        );
    }

    let (status_tx, status_rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::GetStatus {
            reply_to: status_tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    let status = status_rx
        .await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?;

    if status.running {
        return Ok(());
    }

    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::Start {
            config: config.clone(),
            reply_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let start_result = rx
        .await
        .map_err(|_| "Database toolbox actor unavailable".to_string())?;

    match start_result {
        Ok(()) => Ok(()),
        Err(msg) if msg.contains("already running") => Ok(()),
        Err(err) => Err(err),
    }
}

/// Enumerate schemas for a database source
pub async fn enumerate_source_schemas(
    toolbox_tx: &tokio::sync::mpsc::Sender<DatabaseToolboxMsg>,
    source_id: &str,
) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::EnumerateSchemas {
            source_id: source_id.to_string(),
            reply_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    rx.await
        .map_err(|_| "DatabaseToolbox actor died".to_string())?
}

/// Enumerate tables in a schema
pub async fn enumerate_tables_for_schema(
    toolbox_tx: &tokio::sync::mpsc::Sender<DatabaseToolboxMsg>,
    source_id: &str,
    dataset_or_schema: &str,
) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::EnumerateTables {
            source_id: source_id.to_string(),
            dataset_or_schema: dataset_or_schema.to_string(),
            reply_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    rx.await
        .map_err(|_| "DatabaseToolbox actor died".to_string())?
}

/// Fetch schema for a specific table
pub async fn fetch_table_schema(
    toolbox_tx: &tokio::sync::mpsc::Sender<DatabaseToolboxMsg>,
    source_id: &str,
    table_fq_name: &str,
) -> Result<CachedTableSchema, String> {
    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::GetTableInfo {
            source_id: source_id.to_string(),
            fully_qualified_table: table_fq_name.to_string(),
            reply_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    rx.await
        .map_err(|_| "DatabaseToolbox actor died".to_string())?
}

/// Build fully qualified table name based on database kind
pub fn build_fully_qualified_table_name(
    source: &DatabaseSourceConfig,
    dataset_or_schema: &str,
    table_name: &str,
) -> String {
    match source.kind {
        SupportedDatabaseKind::Bigquery => {
            format!("{}.{}.{}", source.project_id.as_deref().unwrap_or(&source.id), dataset_or_schema, table_name)
        }
        _ => {
            format!("{}.{}", dataset_or_schema, table_name)
        }
    }
}

/// Split a fully qualified table name into parent (schema/dataset) and table name
pub fn split_parent_and_table(fq_name: &str) -> (String, String) {
    if let Some(pos) = fq_name.rfind('.') {
        (fq_name[..pos].to_string(), fq_name[pos + 1..].to_string())
    } else {
        (String::new(), fq_name.to_string())
    }
}

/// Build embedding text for a table
pub fn build_table_embedding_text(schema: &CachedTableSchema) -> String {
    let column_summaries: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{} {}{}", c.name, c.data_type, if c.nullable { " nullable" } else { "" }))
        .collect();

    let primary = if schema.primary_keys.is_empty() {
        "none".to_string()
    } else {
        schema.primary_keys.join(", ")
    };

    let partitions = if schema.partition_columns.is_empty() {
        "none".to_string()
    } else {
        schema.partition_columns.join(", ")
    };

    let clusters = if schema.cluster_columns.is_empty() {
        "none".to_string()
    } else {
        schema.cluster_columns.join(", ")
    };

    format!(
        "table {} ({}) columns [{}]; primary keys: {}; partitions: {}; clusters: {}; description: {}",
        schema.fully_qualified_name,
        schema.kind.display_name(),
        column_summaries.join("; "),
        primary,
        partitions,
        clusters,
        schema
            .description
            .clone()
            .unwrap_or_else(|| "none".to_string())
    )
}

/// Build embedding text for a column
pub fn build_column_embedding_text(table_name: &str, column: &crate::settings::CachedColumnSchema) -> String {
    format!(
        "column {}.{} type {} {}; description: {}",
        table_name,
        column.name,
        column.data_type,
        if column.nullable { "nullable" } else { "not null" },
        column
            .description
            .clone()
            .unwrap_or_else(|| "none".to_string())
    )
}

/// Embed table and its columns
pub async fn embed_table_and_columns(
    model: Arc<TextEmbedding>,
    schema: &CachedTableSchema,
) -> Result<(Vec<f32>, Vec<Vec<f32>>), String> {
    let table_text = build_table_embedding_text(schema);
    let column_texts: Vec<String> = schema
        .columns
        .iter()
        .map(|c| build_column_embedding_text(&schema.fully_qualified_name, c))
        .collect();

    let mut to_embed = Vec::with_capacity(1 + column_texts.len());
    to_embed.push(table_text);
    to_embed.extend(column_texts);

    let model_clone = model.clone();
    let embeddings = tokio::task::spawn_blocking(move || {
        model_clone.embed(to_embed, None)
    })
        .await
        .map_err(|e| format!("Embedding task panicked: {}", e))?
        .map_err(|e| format!("Failed to embed schema: {}", e))?;

    let mut iter = embeddings.into_iter();
    let table_embedding = iter
        .next()
        .ok_or_else(|| "No table embedding returned".to_string())?;
    let column_embeddings: Vec<Vec<f32>> = iter.collect();

    Ok((table_embedding, column_embeddings))
}

/// Cache table and columns in the schema vector store
pub async fn cache_table_and_columns(
    schema_tx: &tokio::sync::mpsc::Sender<SchemaVectorMsg>,
    schema: CachedTableSchema,
    table_embedding: Vec<f32>,
    column_embeddings: Vec<Vec<f32>>,
    primary_keys: &HashSet<String>,
    partition_keys: &HashSet<String>,
    cluster_keys: &HashSet<String>,
) -> Result<(), String> {
    if schema.columns.len() != column_embeddings.len() {
        println!(
            "[SchemaRefresh] Column embedding count mismatch for {} (columns={}, embeddings={})",
            schema.fully_qualified_name,
            schema.columns.len(),
            column_embeddings.len()
        );
    }

    // Cache the table schema
    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::CacheTableSchema {
            schema: schema.clone(),
            table_embedding,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Schema vector actor unavailable".to_string())?
        .map_err(|e| format!("Failed to cache table: {}", e))?;

    // Build base chunk key: table only (to reduce duplication)
    let (_, table_name) = split_parent_and_table(&schema.fully_qualified_name);
    let base_chunk = table_name;

    // Cache each column schema
    for (column, embedding) in schema.columns.iter().zip(column_embeddings.into_iter()) {
        let is_join = primary_keys.contains(&column.name)
            || partition_keys.contains(&column.name)
            || cluster_keys.contains(&column.name);
        let chunk_key = if is_join {
            format!("{}:join", base_chunk)
        } else {
            base_chunk.clone()
        };

        let (col_tx, col_rx) = oneshot::channel();
        schema_tx
            .send(SchemaVectorMsg::CacheColumnSchema {
                table_fq_name: schema.fully_qualified_name.clone(),
                source_id: schema.source_id.clone(),
                column: column.clone(),
                column_embedding: embedding,
                chunk_key,
                respond_to: col_tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        col_rx
            .await
            .map_err(|_| "Schema vector actor unavailable".to_string())?
            .map_err(|e| format!("Failed to cache column: {}", e))?;
    }

    Ok(())
}

/// Clear cached schemas for a source
pub async fn clear_source_cache(
    schema_tx: &tokio::sync::mpsc::Sender<SchemaVectorMsg>,
    source_id: &str,
) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::ClearSource {
            source_id: source_id.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await
        .map_err(|_| "Schema vector actor unavailable".to_string())?
}

/// Load cached enabled flags for a source
pub async fn load_cached_enabled_flags(
    schema_tx: &tokio::sync::mpsc::Sender<SchemaVectorMsg>,
    source_id: &str,
) -> Result<HashMap<String, bool>, String> {
    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::GetTablesForSource {
            source_id: source_id.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let tables = rx
        .await
        .map_err(|_| "Schema vector actor unavailable".to_string())?;

    let mut map = HashMap::new();
    for table in tables {
        map.insert(table.fully_qualified_name.clone(), table.enabled);
    }
    Ok(map)
}

/// Refresh schema cache for a single source
pub async fn refresh_schema_cache_for_source(
    app_handle: &AppHandle,
    handles: &State<'_, ActorHandles>,
    source: &DatabaseSourceConfig,
    embedding_model: Arc<TextEmbedding>,
) -> Result<SchemaSourceStatus, String> {
    let _ = app_handle.emit(
        "schema-refresh-progress",
        SchemaRefreshProgress {
            message: format!("Refreshing source '{}'", source.name),
            source_name: source.name.clone(),
            current_table: None,
            tables_done: 0,
            tables_total: 0,
            is_complete: false,
            error: None,
        },
    );

    println!(
        "[SchemaRefresh] Refreshing source '{}' ({})",
        source.name, source.id
    );

    // Preserve existing enabled flags if present
    let previous_map = match load_cached_enabled_flags(&handles.schema_tx, &source.id).await {
        Ok(m) => m,
        Err(e) => {
            let _ = app_handle.emit(
                "schema-refresh-progress",
                SchemaRefreshProgress {
                    message: format!("Failed to load cached flags for {}", source.name),
                    source_name: source.name.clone(),
                    current_table: None,
                    tables_done: 0,
                    tables_total: 0,
                    is_complete: false,
                    error: Some(e.clone()),
                },
            );
            return Err(e);
        }
    };

    // Remove stale entries so we only keep current enumeration
    let _ = clear_source_cache(&handles.schema_tx, &source.id).await;

    let mut datasets = match enumerate_source_schemas(&handles.database_toolbox_tx, &source.id).await {
        Ok(d) => d,
        Err(e) => {
            let _ = app_handle.emit(
                "schema-refresh-progress",
                SchemaRefreshProgress {
                    message: format!("Failed to enumerate schemas for {}", source.name),
                    source_name: source.name.clone(),
                    current_table: None,
                    tables_done: 0,
                    tables_total: 0,
                    is_complete: false,
                    error: Some(e.clone()),
                },
            );
            return Err(e);
        }
    };
    
    // Apply BigQuery dataset allowlist if provided
    if source.kind == SupportedDatabaseKind::Bigquery {
        if let Some(allow_raw) = source.dataset_allowlist.as_ref() {
            let allow: Vec<String> = allow_raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !allow.is_empty() {
                let allow_set: HashSet<String> = allow.into_iter().collect();
                datasets.retain(|d| allow_set.contains(d));
                println!(
                    "[SchemaRefresh] Applying dataset allowlist for source {}: {} datasets retained",
                    source.id,
                    datasets.len()
                );
            }
        }
    }
    
    let mut tables_status = Vec::new();
    let mut all_tables_to_process = Vec::new();

    // First pass: gather all tables to process across all datasets
    for dataset in &datasets {
        let dataset_clean = dataset.trim().to_string();
        // Skip datasets ending with numeric suffix (commonly sharded/dated tables)
        if dataset_clean
            .chars()
            .last()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            continue;
        }

        let tables = match enumerate_tables_for_schema(
            &handles.database_toolbox_tx,
            &source.id,
            &dataset_clean,
        )
        .await
        {
            Ok(t) => t,
            Err(_) => continue,
        };

        for table_name in tables {
            let table_clean = table_name.trim().to_string();
            // Skip tables ending with numeric suffix
            if table_clean
                .chars()
                .last()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
            {
                continue;
            }

            // Apply BigQuery table allowlist if provided
            if source.kind == SupportedDatabaseKind::Bigquery {
                if let Some(allow_raw) = source.table_allowlist.as_ref() {
                    let allow: Vec<String> = allow_raw
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !allow.is_empty() {
                        let allow_set: HashSet<String> = allow.into_iter().collect();
                        if !allow_set.contains(&table_clean) {
                            continue;
                        }
                    }
                }
            }

            all_tables_to_process.push((dataset_clean.clone(), table_clean));
        }
    }

    let tables_total = all_tables_to_process.len();
    let mut tables_done = 0;

    println!(
        "[SchemaRefresh] Source '{}': found {} tables to process",
        source.name, tables_total
    );

    for (dataset_clean, table_clean) in all_tables_to_process {
        tables_done += 1;
        let fq_name = build_fully_qualified_table_name(source, &dataset_clean, &table_clean);
        println!(
            "[SchemaRefresh] Processing table {}/{}: {}",
            tables_done, tables_total, fq_name
        );
        
        let _ = app_handle.emit(
            "schema-refresh-progress",
            SchemaRefreshProgress {
                message: format!("Processing table {}/{}", tables_done, tables_total),
                source_name: source.name.clone(),
                current_table: Some(fq_name.clone()),
                tables_done,
                tables_total,
                is_complete: false,
                error: None,
            },
        );

        let enabled = previous_map.get(&fq_name).copied().unwrap_or(true);

        match fetch_table_schema(&handles.database_toolbox_tx, &source.id, &fq_name).await {
            Ok(mut table_schema) => {
                table_schema.enabled = enabled;
                // Annotate join-worthy columns for chunk key purposes
                let partition_set: HashSet<String> =
                    table_schema.partition_columns.iter().cloned().collect();
                let cluster_set: HashSet<String> =
                    table_schema.cluster_columns.iter().cloned().collect();
                let primary_set: HashSet<String> =
                    table_schema.primary_keys.iter().cloned().collect();

                let (table_embedding, column_embeddings) =
                    match embed_table_and_columns(embedding_model.clone(), &table_schema).await
                    {
                        Ok(res) => res,
                        Err(err) => {
                            println!(
                                "[SchemaRefresh] Failed to embed table {}: {}",
                                fq_name, err
                            );
                            continue;
                        }
                    };

                if let Err(err) = cache_table_and_columns(
                    &handles.schema_tx,
                    table_schema.clone(),
                    table_embedding,
                    column_embeddings,
                    &primary_set,
                    &partition_set,
                    &cluster_set,
                )
                .await
                {
                    println!(
                        "[SchemaRefresh] Failed to cache table {}: {}",
                        fq_name, err
                    );
                    continue;
                }

                println!(
                    "[SchemaRefresh] ✓ Cached table {} ({} columns)",
                    fq_name, table_schema.columns.len()
                );
                
                tables_status.push(SchemaTableStatus {
                    source_id: source.id.clone(),
                    source_name: source.name.clone(),
                    table_fq_name: fq_name.clone(),
                    enabled,
                    column_count: table_schema.columns.len(),
                    description: table_schema.description.clone(),
                });
            }
            Err(err) => {
                println!(
                    "[SchemaRefresh] Failed to cache table {}: {}",
                    fq_name, err
                );
            }
        }
    }

    println!(
        "[SchemaRefresh] Source '{}' complete: {} tables cached",
        source.name, tables_status.len()
    );

    Ok(SchemaSourceStatus {
        source_id: source.id.clone(),
        source_name: source.name.clone(),
        database_kind: source.kind,
        tables: tables_status,
    })
}
