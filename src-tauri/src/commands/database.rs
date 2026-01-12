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
use crate::settings::{
    CachedTableSchema, DatabaseSourceConfig, DatabaseToolboxConfig, SupportedDatabaseKind,
};
use fastembed::TextEmbedding;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;

// NOTE: GPU EMBEDDING DISABLED - FoundryMsg import removed as GetGpuEmbeddingModel,
// UnloadCurrentLlm, and RewarmCurrentModel are no longer used in this file.
// To re-enable, add: use crate::protocol::FoundryMsg;

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

/// A conflict between table names across sources
#[derive(Debug, Clone, serde::Serialize)]
pub struct TableNameConflict {
    /// The table name that conflicts (base name, not fully-qualified)
    pub table_name: String,
    /// Sources that have tables with this name
    pub conflicting_sources: Vec<TableConflictSource>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TableConflictSource {
    pub source_id: String,
    pub source_name: String,
    pub fully_qualified_name: String,
}

/// Check for table name conflicts across enabled sources.
/// Returns a list of conflicts where multiple sources have tables with the same base name.
#[tauri::command]
pub async fn check_table_name_conflicts(
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<Vec<TableNameConflict>, String> {
    let settings = settings_state.settings.read().await;
    
    // Get enabled source IDs and names
    let enabled_sources: Vec<(String, String)> = settings
        .database_toolbox
        .sources
        .iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    
    drop(settings); // Release lock early
    
    if enabled_sources.len() < 2 {
        // No conflicts possible with 0 or 1 source
        return Ok(vec![]);
    }
    
    // Collect tables for each source
    let mut source_tables: HashMap<String, Vec<(String, String)>> = HashMap::new(); // source_id -> [(base_name, fq_name)]
    
    for (source_id, _source_name) in &enabled_sources {
        let (tx, rx) = oneshot::channel();
        if handles
            .schema_tx
            .send(SchemaVectorMsg::GetTablesForSource {
                source_id: source_id.to_string(),
                respond_to: tx,
            })
            .await
            .is_err()
        {
            continue;
        }
        
        if let Ok(tables) = rx.await {
            let table_pairs: Vec<(String, String)> = tables
                .iter()
                .filter(|t| t.enabled)
                .map(|t| {
                    // Extract base name (last part of fully-qualified name)
                    let base_name = t.fully_qualified_name
                        .split('.')
                        .last()
                        .unwrap_or(&t.fully_qualified_name)
                        .to_lowercase();
                    (base_name, t.fully_qualified_name.clone())
                })
                .collect();
            source_tables.insert(source_id.to_string(), table_pairs);
        }
    }
    
    // Find conflicts: same base name in multiple sources
    let mut base_name_to_sources: HashMap<String, Vec<TableConflictSource>> = HashMap::new();
    
    for (source_id, source_name) in &enabled_sources {
        if let Some(tables) = source_tables.get(source_id.as_str()) {
            for (base_name, fq_name) in tables {
                base_name_to_sources
                    .entry(base_name.clone())
                    .or_default()
                    .push(TableConflictSource {
                        source_id: source_id.to_string(),
                        source_name: source_name.to_string(),
                        fully_qualified_name: fq_name.clone(),
                    });
            }
        }
    }
    
    // Filter to only conflicts (2+ sources with same base name)
    let conflicts: Vec<TableNameConflict> = base_name_to_sources
        .into_iter()
        .filter(|(_, sources)| sources.len() > 1)
        .map(|(table_name, conflicting_sources)| TableNameConflict {
            table_name,
            conflicting_sources,
        })
        .collect();
    
    if !conflicts.is_empty() {
        println!(
            "[TableConflictCheck] Found {} table name conflict(s) across enabled sources",
            conflicts.len()
        );
        for conflict in &conflicts {
            let sources_str: Vec<String> = conflict
                .conflicting_sources
                .iter()
                .map(|s| format!("{} ({})", s.fully_qualified_name, s.source_name))
                .collect();
            println!(
                "[TableConflictCheck]   '{}' exists in: {}",
                conflict.table_name,
                sources_str.join(", ")
            );
        }
    }
    
    Ok(conflicts)
}

/// Refresh database schemas for a given configuration
///
/// NOTE: GPU EMBEDDING DISABLED - Always uses CPU embedding model.
/// This simplifies the code and avoids GPU memory contention issues.
/// To re-enable GPU embedding, see the commented code in foundry_actor.rs.
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

    println!(
        "[SchemaRefresh] Config has {} total sources, {} enabled (CPU embedding)",
        toolbox_config.sources.len(),
        sources.len()
    );

    for (idx, source) in sources.iter().enumerate() {
        println!(
            "[SchemaRefresh]   {}. '{}' (id={}, kind={:?}, transport={:?}, command={:?})",
            idx + 1,
            source.name,
            source.id,
            source.kind,
            source.transport,
            source.command
        );
    }

    if sources.is_empty() {
        println!("[SchemaRefresh] No enabled sources to refresh");
        return Ok(SchemaRefreshSummary {
            sources: Vec::new(),
            errors: Vec::new(),
        });
    }

    // Always use CPU embedding model (GPU embedding is disabled)
    let model_guard = embedding_state.cpu_model.read().await;
    let embedding_model = model_guard
        .clone()
        .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
    drop(model_guard);

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

    println!("[SchemaRefresh] Caching complete (CPU)");

    Ok(SchemaRefreshSummary {
        sources: refreshed_sources,
        errors,
    })
}

/// Result of schema refresh operation with detailed per-source status
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaRefreshResult {
    pub sources: Vec<SchemaSourceStatus>,
    pub errors: Vec<SchemaRefreshError>,
}

/// Per-source error information for schema refresh
#[derive(Debug, Clone, serde::Serialize)]
pub struct SchemaRefreshError {
    pub source_id: String,
    pub source_name: String,
    pub error: String,
    pub details: Option<String>,
}

/// Refresh database schemas for ALL enabled sources
#[tauri::command]
pub async fn refresh_database_schemas(
    app_handle: AppHandle,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<SchemaRefreshResult, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    drop(settings_guard);

    println!("[SchemaRefresh] Starting refresh for ALL enabled sources");

    let summary =
        refresh_database_schemas_for_config(&app_handle, &handles, &embedding_state, &toolbox_config).await?;

    let errors: Vec<SchemaRefreshError> = summary
        .errors
        .iter()
        .map(|e| {
            // Parse error string format: "Name (id): error message"
            let parts: Vec<&str> = e.splitn(2, ": ").collect();
            let (source_info, error_msg) = if parts.len() == 2 {
                (parts[0], parts[1])
            } else {
                ("Unknown", e.as_str())
            };
            // Try to extract source_id from "Name (id)" format
            let (source_name, source_id) = if let Some(paren_start) = source_info.rfind('(') {
                let name = source_info[..paren_start].trim();
                let id = source_info[paren_start + 1..].trim_end_matches(')');
                (name.to_string(), id.to_string())
            } else {
                (source_info.to_string(), source_info.to_string())
            };
            SchemaRefreshError {
                source_id,
                source_name,
                error: error_msg.to_string(),
                details: None,
            }
        })
        .collect();

    if !errors.is_empty() {
        println!(
            "[SchemaRefresh] Completed with {} errors:",
            errors.len()
        );
        for err in &errors {
            println!(
                "[SchemaRefresh]   - {} ({}): {}",
                err.source_name, err.source_id, err.error
            );
        }
    } else {
        println!("[SchemaRefresh] All sources refreshed successfully");
    }

    Ok(SchemaRefreshResult {
        sources: summary.sources,
        errors,
    })
}

/// Refresh database schema for a SINGLE source by ID
#[tauri::command]
pub async fn refresh_database_schema_for_source(
    app_handle: AppHandle,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
    embedding_state: State<'_, EmbeddingModelState>,
    source_id: String,
) -> Result<SchemaRefreshResult, String> {
    let settings_guard = settings_state.settings.read().await;
    let toolbox_config = settings_guard.database_toolbox.clone();
    drop(settings_guard);

    println!(
        "[SchemaRefresh] Starting refresh for SINGLE source: {}",
        source_id
    );

    // Find the source by ID
    let source = toolbox_config
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .cloned()
        .ok_or_else(|| format!("Source not found: {}", source_id))?;

    if !source.enabled {
        return Err(format!(
            "Source '{}' is disabled. Enable it in settings first.",
            source.name
        ));
    }

    // Create a config with just this one source
    let single_source_config = DatabaseToolboxConfig {
        enabled: toolbox_config.enabled,
        sources: vec![source.clone()],
    };

    let summary = refresh_database_schemas_for_config(
        &app_handle,
        &handles,
        &embedding_state,
        &single_source_config,
    )
    .await?;

    let errors: Vec<SchemaRefreshError> = summary
        .errors
        .iter()
        .map(|e| SchemaRefreshError {
            source_id: source.id.clone(),
            source_name: source.name.clone(),
            error: e.clone(),
            details: None,
        })
        .collect();

    if !errors.is_empty() {
        println!(
            "[SchemaRefresh] Source '{}' refresh failed: {}",
            source.name,
            errors.iter().map(|e| e.error.as_str()).collect::<Vec<_>>().join("; ")
        );
    } else {
        println!(
            "[SchemaRefresh] Source '{}' refreshed successfully",
            source.name
        );
    }

    Ok(SchemaRefreshResult {
        sources: summary.sources,
        errors,
    })
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
    println!(
        "[SchemaRefresh] Ensuring toolbox is running (enabled={}, sources={})",
        config.enabled,
        config.sources.len()
    );

    if !config.enabled {
        println!(
            "[SchemaRefresh] ⚠️ Database toolbox is disabled in settings; attempting to start anyway"
        );
    }

    let (status_tx, status_rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::GetStatus {
            reply_to: status_tx,
        })
        .await
        .map_err(|e| {
            println!("[SchemaRefresh] ❌ Failed to send GetStatus: {}", e);
            e.to_string()
        })?;
    let status = status_rx
        .await
        .map_err(|_| {
            println!("[SchemaRefresh] ❌ Database toolbox actor unavailable (GetStatus)");
            "Database toolbox actor unavailable".to_string()
        })?;

    println!(
        "[SchemaRefresh] Toolbox status: running={}, connected_sources={:?}, error={:?}",
        status.running,
        status.connected_sources,
        status.error
    );

    if status.running {
        // Check if all requested sources are connected
        let missing: Vec<String> = config
            .sources
            .iter()
            .filter(|s| s.enabled && !status.connected_sources.contains(&s.id))
            .map(|s| format!("'{}' ({})", s.name, s.id))
            .collect();
        
        if !missing.is_empty() {
            println!(
                "[SchemaRefresh] ⚠️ Toolbox running but missing connections for: {}",
                missing.join(", ")
            );
            // Force a restart to sync connections
            println!("[SchemaRefresh] Forcing toolbox restart to sync connections...");
        } else {
            println!("[SchemaRefresh] ✓ Toolbox already running with all sources connected");
            return Ok(());
        }
    }

    println!("[SchemaRefresh] Starting toolbox with {} sources...", config.sources.len());
    for source in &config.sources {
        println!(
            "[SchemaRefresh]   Starting: '{}' (id={}, enabled={}, command={:?})",
            source.name, source.id, source.enabled, source.command
        );
    }

    let (tx, rx) = oneshot::channel();
    toolbox_tx
        .send(DatabaseToolboxMsg::Start {
            config: config.clone(),
            reply_to: tx,
        })
        .await
        .map_err(|e| {
            println!("[SchemaRefresh] ❌ Failed to send Start: {}", e);
            e.to_string()
        })?;

    let start_result = rx
        .await
        .map_err(|_| {
            println!("[SchemaRefresh] ❌ Database toolbox actor unavailable (Start)");
            "Database toolbox actor unavailable".to_string()
        })?;

    match start_result {
        Ok(()) => {
            println!("[SchemaRefresh] ✓ Toolbox started successfully");
            Ok(())
        }
        Err(msg) if msg.contains("already running") => {
            println!("[SchemaRefresh] ✓ Toolbox already running");
            Ok(())
        }
        Err(err) => {
            println!("[SchemaRefresh] ❌ Toolbox start failed: {}", err);
            Err(err)
        }
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
    // Add special attributes (e.g., "primary_key") to help with semantic search for joins
    let attrs = if column.special_attributes.is_empty() {
        String::new()
    } else {
        format!(" [{}]", column.special_attributes.join(", "))
    };

    // Add top values for better semantic matching (e.g., finding "crime type" columns)
    let top_vals = if column.top_values.is_empty() {
        String::new()
    } else {
        // Extract just the value names without percentages for cleaner embedding
        let vals: Vec<String> = column
            .top_values
            .iter()
            .take(3)
            .filter_map(|v| v.split(" (").next().map(|s| s.to_string()))
            .collect();
        if vals.is_empty() {
            String::new()
        } else {
            format!("; examples: {}", vals.join(", "))
        }
    };

    format!(
        "column {}.{} type {} {}{}; description: {}{}",
        table_name,
        column.name,
        column.data_type,
        if column.nullable { "nullable" } else { "not null" },
        attrs,
        column
            .description
            .clone()
            .unwrap_or_else(|| "none".to_string()),
        top_vals
    )
}

/// Embed table and its columns
/// 
/// NOTE: For tables with many columns, we batch the embeddings to avoid
/// overwhelming CoreML/GPU memory. This prevents "Context leak" crashes on macOS.
pub async fn embed_table_and_columns(
    model: Arc<TextEmbedding>,
    schema: &CachedTableSchema,
) -> Result<(Vec<f32>, Vec<Vec<f32>>), String> {
    // Batch size for embedding - prevents CoreML context exhaustion
    const EMBEDDING_BATCH_SIZE: usize = 32;
    
    let table_name = &schema.fully_qualified_name;
    let column_count = schema.columns.len();
    
    println!(
        "[SchemaRefresh] Embedding table '{}' ({} columns, {} batches)...",
        table_name,
        column_count,
        (column_count + EMBEDDING_BATCH_SIZE) / EMBEDDING_BATCH_SIZE
    );

    // First, embed just the table (separate batch to isolate errors)
    let table_text = build_table_embedding_text(schema);
    let model_for_table = model.clone();
    let table_text_clone = table_text.clone();
    
    let table_embedding = tokio::task::spawn_blocking(move || {
        model_for_table.embed(vec![table_text_clone], None)
    })
        .await
        .map_err(|e| format!("Table embedding task panicked: {}", e))?
        .map_err(|e| format!("Failed to embed table '{}': {}", table_name, e))?
        .into_iter()
        .next()
        .ok_or_else(|| format!("No embedding returned for table '{}'", table_name))?;

    println!(
        "[SchemaRefresh] ✓ Embedded table '{}', now embedding {} columns...",
        table_name, column_count
    );

    // Build column texts
    let column_texts: Vec<String> = schema
        .columns
        .iter()
        .map(|c| build_column_embedding_text(&schema.fully_qualified_name, c))
        .collect();

    // Embed columns in batches to prevent CoreML context exhaustion
    let mut all_column_embeddings: Vec<Vec<f32>> = Vec::with_capacity(column_count);
    
    for (batch_idx, batch) in column_texts.chunks(EMBEDDING_BATCH_SIZE).enumerate() {
        let batch_start = batch_idx * EMBEDDING_BATCH_SIZE;
        let batch_end = batch_start + batch.len();
        
        println!(
            "[SchemaRefresh]   Embedding columns {}-{} of {} for '{}'",
            batch_start + 1,
            batch_end,
            column_count,
            table_name
        );

        let batch_texts: Vec<String> = batch.to_vec();
        let model_clone = model.clone();
        let table_name_clone = table_name.clone();
        
        let batch_embeddings = tokio::task::spawn_blocking(move || {
            model_clone.embed(batch_texts, None)
        })
            .await
            .map_err(|e| format!(
                "Column embedding task panicked for '{}' batch {}: {}",
                table_name_clone, batch_idx + 1, e
            ))?
            .map_err(|e| format!(
                "Failed to embed columns for '{}' batch {}: {}",
                table_name, batch_idx + 1, e
            ))?;

        all_column_embeddings.extend(batch_embeddings);
    }

    println!(
        "[SchemaRefresh] ✓ Completed embedding for '{}' ({} + {} embeddings)",
        table_name, 1, all_column_embeddings.len()
    );

    Ok((table_embedding, all_column_embeddings))
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
