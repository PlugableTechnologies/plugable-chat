//! Database schema management Tauri commands.
//!
//! Commands for managing database schema cache, table discovery, and
//! embedding generation for schema search functionality.
//!
//! Note: Due to the complexity and tight coupling with helper functions,
//! many database-related functions remain in lib.rs and are re-exported here.

use crate::actors::database_toolbox_actor::DatabaseToolboxMsg;
use crate::actors::schema_vector_actor::SchemaVectorMsg;
use crate::app_state::{ActorHandles, EmbeddingModelState, SettingsState};
use crate::settings::{
    CachedColumnSchema, CachedTableSchema, DatabaseSourceConfig, DatabaseToolboxConfig,
    SupportedDatabaseKind,
};
use fastembed::TextEmbedding;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::State;
use tokio::sync::oneshot;

/// Status of a table in the schema cache
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaTableStatus {
    pub source_id: String,
    pub source_name: String,
    pub table_fq_name: String,
    pub enabled: bool,
    pub column_count: usize,
    pub description: Option<String>,
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
pub async fn refresh_database_schemas_for_config(
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

    let model_guard = embedding_state.model.read().await;
    let embedding_model = model_guard
        .clone()
        .ok_or_else(|| "Embedding model not initialized".to_string())?;
    drop(model_guard);

    ensure_toolbox_running(&handles.database_toolbox_tx, toolbox_config).await?;

    let mut refreshed_sources = Vec::new();
    let mut errors = Vec::new();

    for source in sources {
        match refresh_schema_cache_for_source(handles, &source, embedding_model.clone()).await {
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

    Ok(SchemaRefreshSummary {
        sources: refreshed_sources,
        errors,
    })
}

/// Refresh database schemas
#[tauri::command]
pub async fn refresh_database_schemas(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<Vec<SchemaSourceStatus>, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    drop(settings_guard);

    let summary =
        refresh_database_schemas_for_config(&handles, &embedding_state, &toolbox_config).await?;

    if !summary.errors.is_empty() {
        println!(
            "[SchemaRefresh] Completed with errors: {}",
            summary.errors.join("; ")
        );
    }

    Ok(summary.sources)
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

/// Set whether a schema table is enabled
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
    let toggle_result = {
        let (tx, rx) = oneshot::channel();
        handles
            .schema_tx
            .send(SchemaVectorMsg::SetTableEnabled {
                table_fq_name: table_fq_name.clone(),
                enabled,
                respond_to: tx,
            })
            .await
            .map_err(|e| e.to_string())?;

        rx.await
            .map_err(|_| "Schema vector actor unavailable".to_string())?
    };

    let table_schema = match toggle_result {
        Ok(schema) => schema,
        Err(_) => {
            // Table not cached, try to fetch and cache it
            let model_guard = embedding_state.model.read().await;
            let embedding_model = model_guard
                .clone()
                .ok_or_else(|| "Embedding model not initialized".to_string())?;
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
                .send(SchemaVectorMsg::SetTableEnabled {
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
    use crate::actors::database_toolbox_actor::ToolboxStatus;

    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::GetStatus { reply_to: tx })
        .await
        .map_err(|e| format!("Failed to send status request: {}", e))?;

    let status = rx
        .await
        .map_err(|_| "DatabaseToolbox actor died".to_string())?;

    match status {
        ToolboxStatus::Running => Ok(()),
        ToolboxStatus::NotStarted | ToolboxStatus::Stopped => {
            let (start_tx, start_rx) = oneshot::channel();
            toolbox_tx
                .send(DatabaseToolboxMsg::Start {
                    config: config.clone(),
                    reply_to: start_tx,
                })
                .await
                .map_err(|e| format!("Failed to send start request: {}", e))?;

            start_rx
                .await
                .map_err(|_| "DatabaseToolbox actor died".to_string())?
        }
        ToolboxStatus::Starting => {
            // Wait a bit for it to start
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(())
        }
        ToolboxStatus::Error(e) => Err(format!("Toolbox in error state: {}", e)),
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
        .send(DatabaseToolboxMsg::GetTableSchema {
            source_id: source_id.to_string(),
            table_fq_name: table_fq_name.to_string(),
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
        SupportedDatabaseKind::BigQuery => {
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
    let mut text = format!("Table: {}", schema.fully_qualified_name);

    if let Some(ref desc) = schema.description {
        text.push_str("\nDescription: ");
        text.push_str(desc);
    }

    text.push_str("\nColumns: ");
    let col_names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
    text.push_str(&col_names.join(", "));

    text
}

/// Build embedding text for a column
pub fn build_column_embedding_text(table_name: &str, column: &CachedColumnSchema) -> String {
    let mut text = format!("Column: {}.{}", table_name, column.name);
    text.push_str(&format!(" ({})", column.data_type));

    if let Some(ref desc) = column.description {
        text.push_str("\nDescription: ");
        text.push_str(desc);
    }

    text
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

    // Embed table
    let table_embeddings = model
        .embed(vec![table_text], None)
        .map_err(|e| format!("Failed to embed table: {}", e))?;

    let table_embedding = table_embeddings
        .into_iter()
        .next()
        .ok_or_else(|| "No table embedding returned".to_string())?;

    // Embed columns
    let column_embeddings = if column_texts.is_empty() {
        Vec::new()
    } else {
        model
            .embed(column_texts, None)
            .map_err(|e| format!("Failed to embed columns: {}", e))?
    };

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
    let mut schema_with_keys = schema;
    for col in &mut schema_with_keys.columns {
        col.is_primary_key = primary_keys.contains(&col.name);
        col.is_partition_key = partition_keys.contains(&col.name);
        col.is_cluster_key = cluster_keys.contains(&col.name);
    }

    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::CacheTable {
            schema: schema_with_keys,
            embedding: table_embedding,
            column_embeddings,
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send cache request: {}", e))?;

    rx.await
        .map_err(|_| "Schema vector actor died".to_string())?
}

/// Clear cached schemas for a source
pub async fn clear_source_cache(
    schema_tx: &tokio::sync::mpsc::Sender<SchemaVectorMsg>,
    source_id: &str,
) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    schema_tx
        .send(SchemaVectorMsg::ClearSourceCache {
            source_id: source_id.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send clear request: {}", e))?;

    rx.await
        .map_err(|_| "Schema vector actor died".to_string())
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
    handles: &State<'_, ActorHandles>,
    source: &DatabaseSourceConfig,
    embedding_model: Arc<TextEmbedding>,
) -> Result<SchemaSourceStatus, String> {
    println!(
        "[SchemaRefresh] Refreshing source: {} ({})",
        source.name, source.id
    );

    // Load existing enabled flags
    let existing_enabled = load_cached_enabled_flags(&handles.schema_tx, &source.id).await?;

    // Clear existing cache for this source
    clear_source_cache(&handles.schema_tx, &source.id).await?;

    // Enumerate schemas/datasets
    let schemas = enumerate_source_schemas(&handles.database_toolbox_tx, &source.id).await?;

    let mut tables = Vec::new();

    for schema_name in schemas {
        // Enumerate tables in this schema
        let table_names =
            enumerate_tables_for_schema(&handles.database_toolbox_tx, &source.id, &schema_name)
                .await?;

        for table_name in table_names {
            let fq_name = build_fully_qualified_table_name(source, &schema_name, &table_name);

            // Fetch table schema
            match fetch_table_schema(&handles.database_toolbox_tx, &source.id, &fq_name).await {
                Ok(mut table_schema) => {
                    // Preserve enabled flag from previous cache
                    table_schema.enabled = existing_enabled.get(&fq_name).copied().unwrap_or(true);

                    // Embed and cache
                    let (table_emb, col_embs) =
                        embed_table_and_columns(embedding_model.clone(), &table_schema).await?;

                    cache_table_and_columns(
                        &handles.schema_tx,
                        table_schema.clone(),
                        table_emb,
                        col_embs,
                        &HashSet::new(),
                        &HashSet::new(),
                        &HashSet::new(),
                    )
                    .await?;

                    tables.push(SchemaTableStatus {
                        source_id: source.id.clone(),
                        source_name: source.name.clone(),
                        table_fq_name: table_schema.fully_qualified_name,
                        enabled: table_schema.enabled,
                        column_count: table_schema.columns.len(),
                        description: table_schema.description,
                    });
                }
                Err(e) => {
                    println!(
                        "[SchemaRefresh] Failed to fetch table {}: {}",
                        fq_name, e
                    );
                }
            }
        }
    }

    Ok(SchemaSourceStatus {
        source_id: source.id.clone(),
        source_name: source.name.clone(),
        database_kind: source.kind,
        tables,
    })
}
