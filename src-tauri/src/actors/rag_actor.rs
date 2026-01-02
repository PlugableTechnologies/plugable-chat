use crate::protocol::{FileError, RagChunk, RagIndexResult, RagMsg, RagProgressEvent, RemoveFileResult};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use fastembed::TextEmbedding;
use futures::StreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::{connect, Connection, Table};
use lru::LruCache;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tauri::AppHandle;
use tauri::Emitter;
use tokio::sync::mpsc;

// ============================================================================
// CONSTANTS
// ============================================================================

/// Soft limit for chunk size in characters (target size)
const CHUNK_SOFT_LIMIT: usize = 400;

/// Hard limit for chunk size in characters (never exceed)
const CHUNK_HARD_LIMIT: usize = 800;

/// The name of the table in LanceDB for RAG chunks
const RAG_CHUNKS_TABLE: &str = "rag_chunks";

/// The name of the table in LanceDB for file cache
const RAG_FILE_CACHE_TABLE: &str = "rag_file_cache";

/// LRU cache capacity for embeddings (~150MB for 384-dim vectors at 10k entries)
const EMBEDDING_LRU_CAPACITY: usize = 10_000;

/// Central cache directory name under ~/.plugable-chat/
const CENTRAL_RAG_CACHE_DIR: &str = "rag-cache";

// ============================================================================
// DOCUMENT ELEMENT TYPES (for hierarchical parsing)
// ============================================================================

/// Represents a parsed element from a document
#[derive(Debug, Clone)]
enum DocumentElement {
    /// A heading with level (1-6) and text
    Heading { level: u8, text: String },
    /// A paragraph of text
    Paragraph(String),
    /// A list item with indent level and text
    ListItem { #[allow(dead_code)] indent: u8, text: String },
    /// A code block
    CodeBlock(String),
}

// ============================================================================
// HEADING STACK MANAGER
// ============================================================================

/// Maintains the current heading hierarchy as document is parsed
struct HeadingStackManager {
    stack: Vec<(u8, String)>, // (level, heading_text)
}

impl HeadingStackManager {
    fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a heading, popping any headings at same or lower level
    fn push_heading(&mut self, level: u8, text: String) {
        // Pop all headings at same or higher level number (lower priority)
        while self.stack.last().map_or(false, |(l, _)| *l >= level) {
            self.stack.pop();
        }
        self.stack.push((level, text));
    }

    /// Get the current heading context as a formatted string
    fn get_context(&self) -> String {
        if self.stack.is_empty() {
            return String::new();
        }
        self.stack
            .iter()
            .map(|(l, t)| format!("H{}: {}", l, t))
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

// ============================================================================
// FILE CACHE ENTRY
// ============================================================================

/// Represents a cached file entry
#[derive(Clone)]
struct FileCacheEntry {
    file_path: String,
    crc32: u32,
    chunk_count: usize,
    indexed_at: i64,
}

// ============================================================================
// INDEXED CHUNK
// ============================================================================

/// A document chunk with its embedding
#[derive(Clone)]
struct IndexedChunk {
    id: String,
    hash: String,
    file_crc32: u32,
    content: String,
    heading_context: String,
    source_file: String,
    chunk_index: usize,
    vector: Vec<f32>,
}

// ============================================================================
// RAG RETRIEVAL ACTOR
// ============================================================================

/// The RAG Actor handles document processing and retrieval
pub struct RagRetrievalActor {
    rx: mpsc::Receiver<RagMsg>,
    /// Active connections to per-directory sidecar databases
    connections: HashMap<PathBuf, DirectoryConnection>,
    /// App handle for emitting events
    app_handle: Option<AppHandle>,
    /// Persistent LRU cache for chunk embeddings (hash -> vector)
    embedding_lru_cache: LruCache<String, Vec<f32>>,
}

/// Represents a connection to a specific directory's sidecar database
struct DirectoryConnection {
    /// LanceDB connection (kept alive for table handles)
    #[allow(dead_code)]
    db: Connection,
    /// Table handle for RAG chunks
    chunks_table: Table,
    /// Table handle for file cache
    file_cache_table: Table,
    /// The root path this connection serves
    #[allow(dead_code)]
    root_path: PathBuf,
}

impl RagRetrievalActor {
    pub fn new(rx: mpsc::Receiver<RagMsg>, app_handle: Option<AppHandle>) -> Self {
        Self {
            rx,
            connections: HashMap::new(),
            app_handle,
            embedding_lru_cache: LruCache::new(NonZeroUsize::new(EMBEDDING_LRU_CAPACITY).unwrap()),
        }
    }

    // ========================================================================
    // DATABASE INITIALIZATION & SIDE CAR MANAGEMENT
    // ========================================================================

    const SIDECAR_CACHE_DIR: &'static str = ".plugable-rag-cache";

    /// Helper to derive the sidecar cache path from a document path
    fn get_cache_dir_for_file(&self, file_path: &Path) -> PathBuf {
        // For a file like /Volumes/USB/docs/report.pdf
        // Returns /Volumes/USB/docs/.plugable-rag-cache/
        file_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(Self::SIDECAR_CACHE_DIR)
    }

    /// On-demand connection creation for a specific path
    async fn ensure_connection_for_path(
        &mut self,
        file_path: &Path,
    ) -> Result<&mut DirectoryConnection, String> {
        let cache_dir = self.get_cache_dir_for_file(file_path);

        if !self.connections.contains_key(&cache_dir) {
            println!("RagActor: Initializing sidecar cache at {:?}", cache_dir);

            // Try to create .plugable-rag-cache directory
            let is_readonly = if let Err(e) = tokio::fs::create_dir_all(&cache_dir).await {
                println!(
                    "RagActor WARNING: Could not create sidecar directory {:?}. Falling back to central cache: {}",
                    cache_dir, e
                );
                true
            } else {
                false
            };

            let db_path_str = if is_readonly {
                // Fallback to central cache instead of volatile memory
                let central_cache = dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".plugable-chat")
                    .join(CENTRAL_RAG_CACHE_DIR)
                    .join(Self::hash_path(&cache_dir)); // Hash to avoid collisions

                if let Err(e) = std::fs::create_dir_all(&central_cache) {
                    // If even central cache fails, then use memory as last resort
                    println!("RagActor WARNING: Central cache also failed: {}. Using memory://", e);
                    "memory://".to_string()
                } else {
                    println!("RagActor: Using central cache at {:?}", central_cache);
                    central_cache.to_string_lossy().to_string()
                }
            } else {
                cache_dir.to_string_lossy().to_string()
            };

            let db = connect(&db_path_str)
                .execute()
                .await
                .map_err(|e| format!("Failed to connect to LanceDB at {}: {}", db_path_str, e))?;

            // Initialize chunks table
            let chunks_schema = self.chunks_schema();
            let chunks_table = self
                .ensure_table_exists(&db, RAG_CHUNKS_TABLE, chunks_schema.clone())
                .await?;

            // Create indexes for chunks table
            let _ = chunks_table
                .create_index(&["id"], Index::Auto)
                .execute()
                .await;
            let _ = chunks_table
                .create_index(&["hash"], Index::Auto)
                .execute()
                .await;
            let _ = chunks_table
                .create_index(&["source_file"], Index::Auto)
                .execute()
                .await;

            // Initialize file cache table
            let file_cache_schema = self.file_cache_schema();
            let file_cache_table = self
                .ensure_table_exists(&db, RAG_FILE_CACHE_TABLE, file_cache_schema.clone())
                .await?;

            // Create index for file cache
            let _ = file_cache_table
                .create_index(&["file_path"], Index::Auto)
                .execute()
                .await;

            self.connections.insert(
                cache_dir.clone(),
                DirectoryConnection {
                    db,
                    chunks_table,
                    file_cache_table,
                    root_path: file_path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .to_path_buf(),
                },
            );
        }

        Ok(self.connections.get_mut(&cache_dir).unwrap())
    }

    async fn ensure_table_exists(
        &self,
        db: &Connection,
        table_name: &str,
        schema: Arc<Schema>,
    ) -> Result<Table, String> {
        let table_names = db.table_names().execute().await.map_err(|e| e.to_string())?;
        
        if table_names.contains(&table_name.to_string()) {
            let table = db.open_table(table_name).execute().await.map_err(|e| e.to_string())?;
            
            // Check schema
            let existing_schema = table.schema().await.map_err(|e| e.to_string())?;
            let existing_field_count = existing_schema.fields().len();
            let expected_field_count = schema.fields().len();

            // Check vector dimension if it exists
            let existing_dim = existing_schema
                .field_with_name("vector")
                .ok()
                .and_then(|f| match f.data_type() {
                    DataType::FixedSizeList(_, dim) => Some(*dim),
                    _ => None,
                });

            let expected_dim = schema
                .field_with_name("vector")
                .ok()
                .and_then(|f| match f.data_type() {
                    DataType::FixedSizeList(_, dim) => Some(*dim),
                    _ => None,
                });

            if existing_field_count != expected_field_count || existing_dim != expected_dim {
                println!(
                    "RagActor: Schema mismatch for {}. Dim: {:?} -> {:?}, Fields: {} -> {}. Recreating table...",
                    table_name,
                    existing_dim,
                    expected_dim,
                    existing_field_count,
                    expected_field_count
                );
                let _ = db.drop_table(table_name).await;
                let batch = RecordBatch::new_empty(schema.clone());
                db.create_table(
                    table_name,
                    RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema),
                )
                .execute()
                .await
                .map_err(|e| e.to_string())
            } else {
                Ok(table)
            }
        } else {
            let batch = RecordBatch::new_empty(schema.clone());
            db.create_table(
                table_name,
                RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema),
            )
            .execute()
            .await
            .map_err(|e| e.to_string())
        }
    }

    fn chunks_schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("hash", DataType::Utf8, false),
            Field::new("file_crc32", DataType::UInt32, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("heading_context", DataType::Utf8, false),
            Field::new("source_file", DataType::Utf8, false),
            Field::new("chunk_index", DataType::Int64, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 768),
                true,
            ),
        ]))
    }

    fn file_cache_schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("file_path", DataType::Utf8, false),
            Field::new("crc32", DataType::UInt32, false),
            Field::new("chunk_count", DataType::Int64, false),
            Field::new("indexed_at", DataType::Int64, false),
        ]))
    }

    // ========================================================================
    // MAIN RUN LOOP
    // ========================================================================

    pub async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                RagMsg::IndexRagDocuments {
                    paths,
                    embedding_model,
                    use_gpu,
                    respond_to,
                } => {
                    println!("RagActor: Processing {} paths ({})", paths.len(), if use_gpu { "GPU" } else { "CPU" });
                    let result = self.process_documents(paths, embedding_model, use_gpu).await;
                    let _ = respond_to.send(result);
                }
                RagMsg::SearchRagChunksByEmbedding {
                    query_vector,
                    limit,
                    respond_to,
                } => {
                    println!("RagActor: Searching with limit {}", limit);
                    let results = self.search_documents(query_vector, limit).await;
                    let _ = respond_to.send(results);
                }
                RagMsg::ClearContext { respond_to } => {
                    println!("RagActor: Clearing context");
                    let result = self.clear_all_tables().await;
                    let _ = respond_to.send(result);
                }
                RagMsg::RemoveFile {
                    source_file,
                    respond_to,
                } => {
                    println!("RagActor: Removing file from index: {}", source_file);
                    let result = self.remove_file(&source_file).await;
                    let _ = respond_to.send(result);
                }
                RagMsg::GetIndexedFiles { respond_to } => {
                    let files = self.get_indexed_files().await;
                    println!("RagActor: Returning {} indexed files", files.len());
                    let _ = respond_to.send(files);
                }
            }
        }

        println!("RagActor: Shutting down");
    }

    async fn clear_all_tables(&self) -> bool {
        let mut success = true;

        for (cache_dir, conn) in &self.connections {
            if let Err(e) = conn.chunks_table.delete("1=1").await {
                println!("RagActor ERROR: Failed to clear chunks in {:?}: {}", cache_dir, e);
                success = false;
            }
            if let Err(e) = conn.file_cache_table.delete("1=1").await {
                println!("RagActor ERROR: Failed to clear file cache in {:?}: {}", cache_dir, e);
                success = false;
            }
        }

        success
    }

    async fn remove_file(&self, source_file: &str) -> RemoveFileResult {
        let escaped_file = source_file.replace("'", "''");
        let cache_dir = self.get_cache_dir_for_file(Path::new(source_file));

        if let Some(conn) = self.connections.get(&cache_dir) {
            // Remove from chunks table
            let filter = format!("source_file = '{}'", escaped_file);
            if let Err(e) = conn.chunks_table.delete(&filter).await {
                println!("RagActor ERROR: Failed to remove file chunks: {}", e);
            }

            // Remove from file cache table
            let filter = format!("file_path = '{}'", escaped_file);
            if let Err(e) = conn.file_cache_table.delete(&filter).await {
                println!("RagActor ERROR: Failed to remove file cache entry: {}", e);
            }
        }

        RemoveFileResult {
            chunks_removed: 0,
            remaining_chunks: self.get_total_chunks().await,
        }
    }

    async fn get_total_chunks(&self) -> usize {
        let mut total = 0;
        for conn in self.connections.values() {
            if let Ok(count) = conn.chunks_table.count_rows(None).await {
                total += count;
            }
        }
        total
    }

    async fn get_indexed_files(&self) -> Vec<String> {
        let mut all_files = std::collections::HashSet::new();
        
        for conn in self.connections.values() {
            if let Ok(mut query) = conn.file_cache_table.query().select(Select::Columns(vec!["file_path".to_string()])).execute().await {
                while let Some(Ok(batch)) = query.next().await {
                    if let Some(col) = batch.column_by_name("file_path") {
                        if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
                            for i in 0..arr.len() {
                                all_files.insert(arr.value(i).to_string());
                            }
                        }
                    }
                }
            }
        }
        
        all_files.into_iter().collect()
    }

    // ========================================================================
    // FILE CACHE OPERATIONS
    // ========================================================================

    /// Get cached file entry by path from a specific table
    async fn get_file_cache_from_table(
        &self,
        table: &Table,
        file_path: &str,
    ) -> Option<FileCacheEntry> {
        let escaped = file_path.replace("'", "''");
        let query = table
            .query()
            .only_if(format!("file_path = '{}'", escaped))
            .limit(1);
        let mut stream = query.execute().await.ok()?;

        if let Some(Ok(batch)) = stream.next().await {
            if batch.num_rows() > 0 {
                let paths = batch
                    .column_by_name("file_path")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>())?;
                let crcs = batch
                    .column_by_name("crc32")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::UInt32Array>())?;
                let counts = batch
                    .column_by_name("chunk_count")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int64Array>())?;
                let timestamps = batch
                    .column_by_name("indexed_at")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int64Array>())?;

                return Some(FileCacheEntry {
                    file_path: paths.value(0).to_string(),
                    crc32: crcs.value(0),
                    chunk_count: counts.value(0) as usize,
                    indexed_at: timestamps.value(0),
                });
            }
        }
        None
    }

    /// Save or update file cache entry in a specific table
    async fn save_file_cache_to_table(
        &self,
        table: &Table,
        entry: &FileCacheEntry,
    ) -> Result<(), String> {
        // Delete existing entry if any
        let escaped = entry.file_path.replace("'", "''");
        let _ = table.delete(&format!("file_path = '{}'", escaped)).await;

        // Insert new entry
        let schema = self.file_cache_schema();
        let paths = Arc::new(StringArray::from(vec![entry.file_path.clone()]));
        let crcs = Arc::new(arrow_array::UInt32Array::from(vec![entry.crc32]));
        let counts = Arc::new(arrow_array::Int64Array::from(vec![entry.chunk_count as i64]));
        let timestamps = Arc::new(arrow_array::Int64Array::from(vec![entry.indexed_at]));

        let batch = RecordBatch::try_new(schema.clone(), vec![paths, crcs, counts, timestamps])
            .map_err(|e| format!("Failed to create file cache batch: {}", e))?;

        table
            .add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
            .execute()
            .await
            .map_err(|e| format!("Failed to save file cache: {}", e))?;

        Ok(())
    }

    /// Check if file needs reindexing based on CRC
    fn should_reindex_file(&self, current_crc: u32, cached: Option<&FileCacheEntry>) -> bool {
        match cached {
            Some(entry) if entry.crc32 == current_crc => false,
            _ => true,
        }
    }

    // ========================================================================
    // BATCH EMBEDDING CACHE OPERATIONS
    // ========================================================================

    /// Batch lookup of cached embeddings - returns HashMap of hash -> vector
    /// This now searches across ALL known connections
    async fn get_cached_embeddings_batch(&mut self, hashes: &[String]) -> HashMap<String, Vec<f32>> {
        let mut result = HashMap::new();
        let mut db_lookup_needed = Vec::new();

        // First check LRU cache
        for hash in hashes {
            if let Some(vector) = self.embedding_lru_cache.get(hash) {
                result.insert(hash.clone(), vector.clone());
            } else {
                db_lookup_needed.push(hash.clone());
            }
        }

        if db_lookup_needed.is_empty() {
            return result;
        }

        // Batch query ALL LanceDB connections for cache misses
        // (An embedding might be in a different sidecar if the same content exists elsewhere)
        for conn in self.connections.values() {
            if db_lookup_needed.is_empty() {
                break;
            }

            let table = &conn.chunks_table;
            for chunk in db_lookup_needed.clone().chunks(500) {
                let hash_list: Vec<String> = chunk
                    .iter()
                    .map(|h| format!("'{}'", h.replace("'", "''")))
                    .collect();
                let filter = format!("hash IN ({})", hash_list.join(", "));

                if let Ok(query) = table
                    .query()
                    .only_if(filter)
                    .select(Select::Columns(vec![
                        "hash".to_string(),
                        "vector".to_string(),
                    ]))
                    .execute()
                    .await
                {
                    let mut stream = query;
                    while let Some(Ok(batch)) = stream.next().await {
                        let hashes_col = batch
                            .column_by_name("hash")
                            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
                        let vectors_col = batch
                            .column_by_name("vector")
                            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());

                        if let (Some(hashes), Some(vectors)) = (hashes_col, vectors_col) {
                            for i in 0..batch.num_rows() {
                                let hash = hashes.value(i).to_string();
                                let v = vectors.value(i);
                                if let Some(arr) = v.as_any().downcast_ref::<Float32Array>() {
                                    let vector = arr.values().to_vec();
                                    // Add to LRU cache
                                    self.embedding_lru_cache.put(hash.clone(), vector.clone());
                                    result.insert(hash.clone(), vector);
                                    // Remove from lookup needed
                                    db_lookup_needed.retain(|h| h != &hash);
                                }
                            }
                        }
                    }
                }
            }
        }

        result
    }

    // ========================================================================
    // DOCUMENT PROCESSING
    // ========================================================================

    async fn process_documents(
        &mut self,
        paths: Vec<String>,
        embedding_model: Arc<TextEmbedding>,
        use_gpu: bool,
    ) -> Result<RagIndexResult, String> {
        let indexing_start = Instant::now();
        let compute_device = if use_gpu { "GPU" } else { "CPU" }.to_string();
        let mut cache_hits = 0;
        let mut files_processed_count = 0;
        let mut file_errors = Vec::new();

        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!("║                    RAG INDEXING STARTED                      ║");
        println!("╚══════════════════════════════════════════════════════════════╝");

        // Collect all files to process
        if let Some(ref handle) = self.app_handle {
                let _ = handle.emit("rag-progress", RagProgressEvent {
                    phase: "collecting_files".to_string(),
                    total_files: 0,
                    processed_files: 0,
                    total_chunks: 0,
                    processed_chunks: 0,
                    current_file: String::new(),
                    is_complete: false,
                    extraction_progress: None,
                    extraction_total_pages: None,
                    compute_device: Some(compute_device.clone()),
                });
        }
        let mut files_to_process: Vec<PathBuf> = Vec::new();
        for path_str in &paths {
            let path = Path::new(path_str);
            if path.is_dir() {
                match self.collect_files_recursive(path).await {
                    Ok(entries) => files_to_process.extend(entries),
                    Err(e) => {
                        println!("[RAG] Error collecting files from {:?}: {}", path, e);
                        file_errors.push(FileError { 
                            file: path_str.clone(), 
                            error: e 
                        });
                    }
                }
            } else if path.is_file() {
                files_to_process.push(path.to_path_buf());
            }
        }

        let total_files = files_to_process.len();
        println!("RagActor: Found {} files to process", total_files);

        // Phase 1: Check file-level CRC cache and collect chunks that need processing
        let mut all_pending_chunks: Vec<IndexedChunk> = Vec::new();
        let mut total_chunks_in_index = 0;
        let mut files_skipped = 0;
        
        // Collect all chunks by file for cache updating later
        let mut chunks_by_file: HashMap<String, (u32, usize)> = HashMap::new();
        
        for (i, file_path) in files_to_process.iter().enumerate() {
            let file_path_str = file_path.to_string_lossy().to_string();

            if let Some(ref handle) = self.app_handle {
                let _ = handle.emit("rag-progress", RagProgressEvent {
                    phase: "reading_files".to_string(),
                    total_files,
                    processed_files: i,
                    total_chunks: 0,
                    processed_chunks: 0,
                    current_file: file_path_str.clone(),
                    is_complete: false,
                    extraction_progress: None,
                    extraction_total_pages: None,
                    compute_device: Some(compute_device.clone()),
                });
            }

            // Ensure we have a connection for this file's directory
            // We scope the borrow here so we can call other self methods later
            let (chunks_table, file_cache_table) = match self.ensure_connection_for_path(file_path).await {
                Ok(conn) => (conn.chunks_table.clone(), conn.file_cache_table.clone()),
                Err(e) => {
                    println!("RagActor ERROR: Skipping {:?} - {}", file_path, e);
                    continue;
                }
            };

            // Read file bytes
            let bytes = match tokio::fs::read(file_path).await {
                Ok(b) => b,
                Err(e) => {
                    let error_msg = if e.kind() == std::io::ErrorKind::PermissionDenied {
                        "Permission denied: cannot read file".to_string()
                    } else {
                        format!("Failed to read file: {}", e)
                    };
                    println!("[RAG] Error reading {:?}: {}", file_path, error_msg);
                    file_errors.push(FileError { 
                        file: file_path_str.clone(), 
                        error: error_msg 
                    });
                    continue;
                }
            };

            // Compute CRC32 (fast)
            let current_crc = crc32fast::hash(&bytes);

            // Check file-level cache
            let cached_entry = self
                .get_file_cache_from_table(&file_cache_table, &file_path_str)
                .await;
            if !self.should_reindex_file(current_crc, cached_entry.as_ref()) {
                // File unchanged, skip processing
                files_processed_count += 1;
                files_skipped += 1;
                if let Some(entry) = cached_entry {
                    cache_hits += entry.chunk_count;
                    total_chunks_in_index += entry.chunk_count;
                }
                continue;
            }

            // File needs processing - first remove any existing chunks for this file
            let escaped_path = file_path_str.replace("'", "''");
            let filter = format!("source_file = '{}'", escaped_path);
            let _ = chunks_table.delete(&filter).await;

            // Determine if binary file
            let ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let is_binary = ext == "pdf" || ext == "docx";

            if is_binary {
                if let Some(ref handle) = self.app_handle {
                    let _ = handle.emit("rag-progress", RagProgressEvent {
                        phase: "extracting_text".to_string(),
                        total_files,
                        processed_files: i,
                        total_chunks: 0,
                        processed_chunks: 0,
                        current_file: file_path_str.clone(),
                        is_complete: false,
                        extraction_progress: None,
                        extraction_total_pages: None,
                        compute_device: Some(compute_device.clone()),
                    });
                }
            }

            let content = if is_binary {
                String::new()
            } else {
                String::from_utf8_lossy(&bytes).to_string()
            };

            // Extract text and parse into elements
            match self.extract_text(file_path, &content, i, total_files) {
                Ok(text_content) => {
                    if let Some(ref handle) = self.app_handle {
                        let _ = handle.emit("rag-progress", RagProgressEvent {
                            phase: "chunking".to_string(),
                            total_files,
                            processed_files: i,
                            total_chunks: 0,
                            processed_chunks: 0,
                            current_file: file_path_str.clone(),
                            is_complete: false,
                            extraction_progress: None,
                            extraction_total_pages: None,
                            compute_device: Some(compute_device.clone()),
                        });
                    }

                    // Parse document into elements (with heading detection)
                    let elements = self.parse_document(&ext, &text_content);

                    // Chunk semantically with heading detection
                    let chunks_with_context = self.semantic_chunk(&elements);

                    total_chunks_in_index += chunks_with_context.len();

                    let mut chunks_for_this_file = Vec::new();
                    for (idx, (heading_ctx, chunk_content)) in chunks_with_context.into_iter().enumerate()
                    {
                        let chunk_hash = self.compute_hash(&chunk_content);
                        chunks_for_this_file.push(IndexedChunk {
                            id: format!("{}:{}:{}", chunk_hash, file_path_str, idx),
                            hash: chunk_hash,
                            file_crc32: current_crc,
                            content: chunk_content,
                            heading_context: heading_ctx,
                            source_file: file_path_str.clone(),
                            chunk_index: idx,
                            vector: Vec::new(),
                        });
                    }

                    // Track chunk count for cache update later
                    chunks_by_file.insert(file_path_str.clone(), (current_crc, chunks_for_this_file.len()));
                    
                    all_pending_chunks.extend(chunks_for_this_file);
                    files_processed_count += 1;
                }
                Err(e) => {
                    println!("[RAG] Error extracting {}: {}", file_path_str, e);
                    file_errors.push(FileError { file: file_path_str.clone(), error: e });
                    continue;
                }
            }
        }

        println!("RagActor: {} files skipped (CRC match), {} files to process", 
                 files_skipped, files_processed_count - files_skipped);

        let total_pending_chunks = all_pending_chunks.len();
        println!("RagActor: {} chunks need processing", total_pending_chunks);

        if total_pending_chunks == 0 {
            // FIX: Emit rag-progress with is_complete=true so frontend clears status bar
            if let Some(ref handle) = self.app_handle {
                let _ = handle.emit("rag-progress", RagProgressEvent {
                    phase: "complete".to_string(),
                    total_files,
                    processed_files: total_files,
                    total_chunks: 0,
                    processed_chunks: 0,
                    current_file: String::new(),
                    is_complete: true,
                    extraction_progress: None,
                    extraction_total_pages: None,
                    compute_device: Some(compute_device.clone()),
                });
            }
            return Ok(RagIndexResult {
                total_chunks: total_chunks_in_index,
                files_processed: files_processed_count,
                cache_hits,
                file_errors,
            });
        }

        // Phase 2: Batch lookup cached embeddings (eliminates N+1 queries)
        if let Some(ref handle) = self.app_handle {
            let _ = handle.emit("rag-progress", RagProgressEvent {
                phase: "checking_cache".to_string(),
                total_files,
                processed_files: total_files,
                total_chunks: total_pending_chunks,
                processed_chunks: 0,
                current_file: String::new(),
                is_complete: false,
                extraction_progress: None,
                extraction_total_pages: None,
                compute_device: Some(compute_device.clone()),
            });
        }
        let all_hashes: Vec<String> = all_pending_chunks.iter()
            .map(|c| c.hash.clone())
            .collect();
        let cached_embeddings = self.get_cached_embeddings_batch(&all_hashes).await;
        
        println!("RagActor: Found {} cached embeddings in batch lookup", cached_embeddings.len());

        // Phase 3: Separate chunks that have cached embeddings from those that need generation
        let mut final_chunks = Vec::new();
        let mut chunks_to_embed = Vec::new();
        
        for mut chunk in all_pending_chunks {
            if let Some(vector) = cached_embeddings.get(&chunk.hash) {
                chunk.vector = vector.clone();
                final_chunks.push(chunk);
                cache_hits += 1;
            } else {
                chunks_to_embed.push(chunk);
            }
        }

        // Phase 4: Batch generate embeddings for uncached chunks
        // Use smaller batches (10) to provide frequent progress updates to the UI
        const EMBEDDING_BATCH_SIZE: usize = 10;
        let chunks_to_embed_count = chunks_to_embed.len();
        
        if chunks_to_embed_count > 0 {
            println!(
                "╔══════════════════════════════════════════════════════════════╗\n\
                 ║  EMBEDDING GENERATION: {} chunks via {}                      \n\
                 ╚══════════════════════════════════════════════════════════════╝",
                chunks_to_embed_count, compute_device
            );
            
            // Emit initial progress event BEFORE starting embedding loop
            if let Some(ref handle) = self.app_handle {
                let _ = handle.emit("rag-progress", RagProgressEvent {
                    phase: "generating_embeddings".to_string(),
                    total_files,
                    processed_files: total_files,
                    total_chunks: total_pending_chunks,
                    processed_chunks: final_chunks.len(),
                    current_file: String::new(),
                    is_complete: false,
                    extraction_progress: None,
                    extraction_total_pages: None,
                    compute_device: Some(compute_device.clone()),
                });
            }
            
            let embedding_start = Instant::now();
            let mut batch_count = 0;
            
            for batch in chunks_to_embed.chunks_mut(EMBEDDING_BATCH_SIZE) {
                batch_count += 1;
                let batch_start = Instant::now();
                
                // Prepend heading context to content for better embeddings
                let texts: Vec<String> = batch.iter()
                    .map(|c| {
                        if c.heading_context.is_empty() {
                            c.content.clone()
                        } else {
                            format!("[Context: {}]\n\n{}", c.heading_context, c.content)
                        }
                    })
                    .collect();
                let batch_size = texts.len();
                let model = Arc::clone(&embedding_model);
                
                let embeddings = tokio::task::spawn_blocking(move || {
                    model.embed(texts, None)
                })
                .await
                .map_err(|e| format!("Embedding task failed: {}", e))?
                .map_err(|e| format!("Embedding generation failed: {}", e))?;

                for (chunk, vector) in batch.iter_mut().zip(embeddings.into_iter()) {
                    chunk.vector = vector.clone();
                    // Add to LRU cache
                    self.embedding_lru_cache.put(chunk.hash.clone(), vector);
                    final_chunks.push(chunk.clone());
                }
                
                let batch_elapsed = batch_start.elapsed();
                let progress_pct = (final_chunks.len() as f64 / total_pending_chunks as f64 * 100.0) as u32;
                
                println!(
                    "RagActor: [{}] Batch {} ({} chunks) in {:?} | Progress: {}/{} ({}%)",
                    compute_device,
                    batch_count,
                    batch_size,
                    batch_elapsed,
                    final_chunks.len(),
                    total_pending_chunks,
                    progress_pct
                );

                // Emit progress after every batch for responsive UI
                if let Some(ref handle) = self.app_handle {
                    let _ = handle.emit("rag-progress", RagProgressEvent {
                        phase: "generating_embeddings".to_string(),
                        total_files,
                        processed_files: total_files,
                        total_chunks: total_pending_chunks,
                        processed_chunks: final_chunks.len(),
                        current_file: final_chunks.last().map(|c| c.source_file.clone()).unwrap_or_default(),
                        is_complete: final_chunks.len() == total_pending_chunks,
                        extraction_progress: None,
                        extraction_total_pages: None,
                        compute_device: Some(compute_device.clone()),
                    });
                }
            }
            
            let total_embedding_time = embedding_start.elapsed();
            let chunks_per_sec = chunks_to_embed_count as f64 / total_embedding_time.as_secs_f64();
            println!(
                "RagActor: [{}] Embedding complete: {} chunks in {:.1}s ({:.1} chunks/sec)",
                compute_device,
                chunks_to_embed_count,
                total_embedding_time.as_secs_f64(),
                chunks_per_sec
            );
        } else if !final_chunks.is_empty() {
            // FIX: All embeddings were cached, emit progress event with is_complete=true
            if let Some(ref handle) = self.app_handle {
                let _ = handle.emit("rag-progress", RagProgressEvent {
                    phase: "complete".to_string(),
                    total_files,
                    processed_files: total_files,
                    total_chunks: final_chunks.len(),
                    processed_chunks: final_chunks.len(),
                    current_file: final_chunks.last().map(|c| c.source_file.clone()).unwrap_or_default(),
                    is_complete: true,
                    extraction_progress: None,
                    extraction_total_pages: None,
                    compute_device: Some(compute_device.clone()),
                });
            }
        }

        // Phase 5: Batch save to LanceDB (grouped by connection)
        if let Some(ref handle) = self.app_handle {
            let _ = handle.emit("rag-progress", RagProgressEvent {
                phase: "saving".to_string(),
                total_files,
                processed_files: total_files,
                total_chunks: final_chunks.len(),
                processed_chunks: final_chunks.len(),
                current_file: String::new(),
                is_complete: false,
                extraction_progress: None,
                extraction_total_pages: None,
                compute_device: Some(compute_device.clone()),
            });
        }
        println!("RagActor: Saving {} chunks to databases", final_chunks.len());
        
        // Group chunks by their target connection
        let mut chunks_by_cache_dir: HashMap<PathBuf, Vec<IndexedChunk>> = HashMap::new();
        for chunk in final_chunks {
            let cache_dir = self.get_cache_dir_for_file(Path::new(&chunk.source_file));
            chunks_by_cache_dir.entry(cache_dir).or_default().push(chunk);
        }

        for (cache_dir, chunks) in chunks_by_cache_dir {
            if let Some(conn) = self.connections.get(&cache_dir) {
                if let Err(e) = self.save_chunks_to_db(&conn.chunks_table, chunks.clone()).await {
                    println!("RagActor ERROR: Failed to save chunks to {:?}: {}", cache_dir, e);
                } else {
                    // Chunks saved successfully, now update file cache for these files
                    let mut files_in_this_batch = HashSet::new();
                    for chunk in chunks {
                        files_in_this_batch.insert(chunk.source_file);
                    }
                    
                    for file_path_str in files_in_this_batch {
                        if let Some((crc, count)) = chunks_by_file.get(&file_path_str) {
                            let cache_entry = FileCacheEntry {
                                file_path: file_path_str.clone(),
                                crc32: *crc,
                                chunk_count: *count,
                                indexed_at: chrono::Utc::now().timestamp(),
                            };
                            if let Err(e) = self.save_file_cache_to_table(&conn.file_cache_table, &cache_entry).await {
                                println!("RagActor ERROR: Failed to update file cache for {}: {}", file_path_str, e);
                            }
                        }
                    }
                }
            }
        }

        let total_time = indexing_start.elapsed();
        println!("RagActor: Indexing complete in {} ms", total_time.as_millis());

        // Emit final completion event so frontend clears status bar
        if let Some(ref handle) = self.app_handle {
            let _ = handle.emit("rag-progress", RagProgressEvent {
                phase: "complete".to_string(),
                total_files,
                processed_files: total_files,
                total_chunks: total_chunks_in_index,
                processed_chunks: total_chunks_in_index,
                current_file: String::new(),
                is_complete: true,
                extraction_progress: None,
                extraction_total_pages: None,
                compute_device: Some(compute_device.clone()),
            });
        }

        Ok(RagIndexResult {
            total_chunks: total_chunks_in_index,
            files_processed: files_processed_count,
            cache_hits,
            file_errors,
        })
    }

    async fn save_chunks_to_db(&self, table: &Table, chunks: Vec<IndexedChunk>) -> Result<(), String> {
        if chunks.is_empty() {
            return Ok(());
        }

        let schema = self.chunks_schema();

        let mut ids = Vec::with_capacity(chunks.len());
        let mut hashes = Vec::with_capacity(chunks.len());
        let mut file_crcs = Vec::with_capacity(chunks.len());
        let mut contents = Vec::with_capacity(chunks.len());
        let mut heading_contexts = Vec::with_capacity(chunks.len());
        let mut source_files = Vec::with_capacity(chunks.len());
        let mut indices = Vec::with_capacity(chunks.len());
        let mut vectors = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            ids.push(chunk.id);
            hashes.push(chunk.hash);
            file_crcs.push(chunk.file_crc32);
            contents.push(chunk.content);
            heading_contexts.push(chunk.heading_context);
            source_files.push(chunk.source_file);
            indices.push(chunk.chunk_index as i64);
            vectors.push(Some(chunk.vector.into_iter().map(Some).collect::<Vec<_>>()));
        }

        let id_arr = Arc::new(StringArray::from(ids));
        let hash_arr = Arc::new(StringArray::from(hashes));
        let file_crc_arr = Arc::new(arrow_array::UInt32Array::from(file_crcs));
        let content_arr = Arc::new(StringArray::from(contents));
        let heading_ctx_arr = Arc::new(StringArray::from(heading_contexts));
        let source_arr = Arc::new(StringArray::from(source_files));
        let index_arr = Arc::new(arrow_array::Int64Array::from(indices));
        
        let vector_arr = Arc::new(FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            vectors,
            768,
        ));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![id_arr, hash_arr, file_crc_arr, content_arr, heading_ctx_arr, source_arr, index_arr, vector_arr],
        ).map_err(|e| format!("Failed to create record batch: {}", e))?;

        table.add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
            .execute()
            .await
            .map_err(|e| format!("Failed to add records to LanceDB: {}", e))?;

        Ok(())
    }

    // ========================================================================
    // DOCUMENT PARSING (Hierarchical)
    // ========================================================================

    /// Parse document into structured elements based on file type
    fn parse_document(&self, extension: &str, content: &str) -> Vec<DocumentElement> {
        match extension {
            "md" => self.parse_markdown(content),
            "docx" => self.parse_docx_elements(content),
            "pdf" => self.parse_pdf_elements(content),
            "txt" => self.parse_plaintext(content),
            _ => self.parse_plaintext(content),
        }
    }

    /// Parse Markdown document
    fn parse_markdown(&self, content: &str) -> Vec<DocumentElement> {
        let mut elements = Vec::new();
        let mut current_paragraph = String::new();
        let mut in_code_block = false;
        let mut code_block_content = String::new();
        
        for line in content.lines() {
            // Handle code blocks
            if line.starts_with("```") {
                if in_code_block {
                    // End of code block
                    elements.push(DocumentElement::CodeBlock(code_block_content.trim().to_string()));
                    code_block_content.clear();
                    in_code_block = false;
                } else {
                    // Start of code block - flush paragraph first
                    if !current_paragraph.is_empty() {
                        elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                        current_paragraph.clear();
                    }
                    in_code_block = true;
                }
                continue;
            }
            
            if in_code_block {
                if !code_block_content.is_empty() {
                    code_block_content.push('\n');
                }
                code_block_content.push_str(line);
                continue;
            }
            
            // Handle headings
            if let Some(level) = self.detect_markdown_heading(line) {
                // Flush current paragraph
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                let text = line.trim_start_matches('#').trim().to_string();
                elements.push(DocumentElement::Heading { level, text });
                continue;
            }
            
            // Handle list items
            if let Some((indent, text)) = self.detect_list_item(line) {
                // Flush current paragraph
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                elements.push(DocumentElement::ListItem { indent, text });
                continue;
            }
            
            // Handle blank lines
            if line.trim().is_empty() {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                continue;
            }
            
            // Accumulate paragraph text
            if !current_paragraph.is_empty() {
                current_paragraph.push(' ');
            }
            current_paragraph.push_str(line.trim());
        }
        
        // Flush remaining paragraph
        if !current_paragraph.is_empty() {
            elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
        }
        
        elements
    }

    fn detect_markdown_heading(&self, line: &str) -> Option<u8> {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            if level >= 1 && level <= 6 && trimmed.chars().nth(level) == Some(' ') {
                return Some(level as u8);
            }
        }
        None
    }

    fn detect_list_item(&self, line: &str) -> Option<(u8, String)> {
        let leading_spaces = line.len() - line.trim_start().len();
        let indent = (leading_spaces / 2) as u8;
        let trimmed = line.trim_start();
        
        // Bullet list: - item, * item, + item
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            return Some((indent, trimmed[2..].to_string()));
        }
        
        // Numbered list: 1. item, 2. item, etc.
        if let Some(dot_pos) = trimmed.find(". ") {
            if dot_pos > 0 && trimmed[..dot_pos].chars().all(|c| c.is_ascii_digit()) {
                return Some((indent, trimmed[dot_pos + 2..].to_string()));
            }
        }
        
        None
    }

    /// Parse DOCX content (already extracted to text)
    fn parse_docx_elements(&self, content: &str) -> Vec<DocumentElement> {
        // For DOCX, we rely on paragraph breaks already in the content
        // Headings would need to be detected from styles, which requires XML parsing
        // For now, use heuristics similar to plaintext
        self.parse_plaintext(content)
    }

    /// Parse PDF content (already extracted to text)
    fn parse_pdf_elements(&self, content: &str) -> Vec<DocumentElement> {
        let mut elements = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut current_paragraph = String::new();
        
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            if trimmed.is_empty() {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                continue;
            }
            
            // Heuristic: Short lines followed by longer content may be headings
            // Also: ALL CAPS lines, or lines that look like titles
            if self.looks_like_heading_pdf(trimmed, lines.get(i + 1).copied()) {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                elements.push(DocumentElement::Heading { 
                    level: 2, // Default to level 2 since we can't detect hierarchy from PDF
                    text: trimmed.to_string() 
                });
                continue;
            }
            
            // Accumulate paragraph
            if !current_paragraph.is_empty() {
                current_paragraph.push(' ');
            }
            current_paragraph.push_str(trimmed);
        }
        
        if !current_paragraph.is_empty() {
            elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
        }
        
        elements
    }

    fn looks_like_heading_pdf(&self, line: &str, next_line: Option<&str>) -> bool {
        // Heuristics for PDF heading detection:
        // 1. ALL CAPS and short
        // 2. Short line followed by blank or much longer line
        // 3. Ends with no punctuation and is relatively short
        
        let is_all_caps = line.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase())
            && line.len() > 3 && line.len() < 80;
        
        let is_short = line.len() < 60;
        let no_end_punct = !line.ends_with('.') && !line.ends_with(',') && !line.ends_with(';');
        let next_is_longer = next_line.map_or(true, |n| n.len() > line.len() * 2 || n.trim().is_empty());
        
        is_all_caps || (is_short && no_end_punct && next_is_longer && line.len() > 5)
    }

    /// Parse plain text document
    fn parse_plaintext(&self, content: &str) -> Vec<DocumentElement> {
        let mut elements = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut current_paragraph = String::new();
        
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            if trimmed.is_empty() {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                continue;
            }
            
            // Heuristic: ALL CAPS lines or lines followed by underlines may be headings
            let next_line = lines.get(i + 1).copied();
            if self.looks_like_heading_txt(trimmed, next_line) {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                elements.push(DocumentElement::Heading { 
                    level: 2,
                    text: trimmed.to_string() 
                });
                continue;
            }
            
            // Skip underlines (===, ---)
            if trimmed.chars().all(|c| c == '=' || c == '-') && trimmed.len() > 3 {
                continue;
            }
            
            // Accumulate paragraph
            if !current_paragraph.is_empty() {
                current_paragraph.push(' ');
            }
            current_paragraph.push_str(trimmed);
        }
        
        if !current_paragraph.is_empty() {
            elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
        }
        
        elements
    }

    fn looks_like_heading_txt(&self, line: &str, next_line: Option<&str>) -> bool {
        // ALL CAPS
        let is_all_caps = line.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase())
            && line.len() > 3 && line.len() < 80;
        
        // Followed by underline
        let followed_by_underline = next_line.map_or(false, |n| {
            let n = n.trim();
            n.len() > 3 && n.chars().all(|c| c == '=' || c == '-')
        });
        
        is_all_caps || followed_by_underline
    }

    // ========================================================================
    // SEMANTIC CHUNKING
    // ========================================================================

    /// Chunk elements semantically, respecting paragraph boundaries
    fn semantic_chunk(&self, elements: &[DocumentElement]) -> Vec<(String, String)> {
        let mut chunks: Vec<(String, String)> = Vec::new();
        let mut heading_stack = HeadingStackManager::new();
        let mut current_chunk_content = String::new();
        let mut current_chunk_context = String::new();
        
        for element in elements {
            match element {
                DocumentElement::Heading { level, text } => {
                    // Flush current chunk before heading change
                    if !current_chunk_content.is_empty() {
                        chunks.push((current_chunk_context.clone(), current_chunk_content.trim().to_string()));
                        current_chunk_content.clear();
                    }
                    
                    heading_stack.push_heading(*level, text.clone());
                    current_chunk_context = heading_stack.get_context();
                }
                
                DocumentElement::Paragraph(text) |
                DocumentElement::CodeBlock(text) => {
                    self.add_to_chunk(
                        text,
                        &mut current_chunk_content,
                        &current_chunk_context,
                        &mut chunks,
                    );
                }
                
                DocumentElement::ListItem { text, .. } => {
                    // Format list item with bullet
                    let formatted = format!("• {}", text);
                    self.add_to_chunk(
                        &formatted,
                        &mut current_chunk_content,
                        &current_chunk_context,
                        &mut chunks,
                    );
                }
            }
        }
        
        // Flush final chunk
        if !current_chunk_content.is_empty() {
            chunks.push((current_chunk_context, current_chunk_content.trim().to_string()));
        }
        
        chunks
    }

    fn add_to_chunk(
        &self,
        text: &str,
        current_chunk_content: &mut String,
        current_chunk_context: &str,
        chunks: &mut Vec<(String, String)>,
    ) {
        let text_len = text.chars().count();
        let current_len = current_chunk_content.chars().count();
        
        // If adding this text would exceed hard limit
        if current_len + text_len + 2 > CHUNK_HARD_LIMIT {
            // Flush current chunk if not empty
            if !current_chunk_content.is_empty() {
                chunks.push((current_chunk_context.to_string(), current_chunk_content.trim().to_string()));
                current_chunk_content.clear();
            }
            
            // If the text itself is too long, split it
            if text_len > CHUNK_HARD_LIMIT {
                self.split_long_text(text, current_chunk_context, chunks);
                return;
            }
        }
        
        // If we've hit soft limit, try to start a new chunk
        if current_len >= CHUNK_SOFT_LIMIT && !current_chunk_content.is_empty() {
            chunks.push((current_chunk_context.to_string(), current_chunk_content.trim().to_string()));
            current_chunk_content.clear();
        }
        
        // Add text to current chunk
        if !current_chunk_content.is_empty() {
            current_chunk_content.push_str("\n\n");
        }
        current_chunk_content.push_str(text);
    }

    fn split_long_text(
        &self,
        text: &str,
        context: &str,
        chunks: &mut Vec<(String, String)>,
    ) {
        let sentences = self.split_into_sentences(text);
        let mut current_chunk = String::new();
        
        for sentence in sentences {
            let sentence_len = sentence.chars().count();
            let current_len = current_chunk.chars().count();
            
            if current_len + sentence_len + 1 > CHUNK_HARD_LIMIT && !current_chunk.is_empty() {
                chunks.push((context.to_string(), current_chunk.trim().to_string()));
                current_chunk.clear();
            }
            
            // If sentence itself is too long, split at word boundaries
            if sentence_len > CHUNK_HARD_LIMIT {
                if !current_chunk.is_empty() {
                    chunks.push((context.to_string(), current_chunk.trim().to_string()));
                    current_chunk.clear();
                }
                self.split_at_words(&sentence, context, chunks);
                continue;
            }
            
            if !current_chunk.is_empty() {
                current_chunk.push(' ');
            }
            current_chunk.push_str(&sentence);
        }
        
        if !current_chunk.is_empty() {
            chunks.push((context.to_string(), current_chunk.trim().to_string()));
        }
    }

    fn split_into_sentences(&self, text: &str) -> Vec<String> {
        let mut sentences = Vec::new();
        let mut current = String::new();
        let mut chars = text.chars().peekable();
        
        while let Some(c) = chars.next() {
            current.push(c);
            
            // End of sentence detection
            if c == '.' || c == '!' || c == '?' {
                // Check if followed by space/newline or end
                if chars.peek().map_or(true, |&next| next.is_whitespace()) {
                    sentences.push(current.trim().to_string());
                    current.clear();
                }
            }
            // Also split at newlines
            else if c == '\n' {
                if !current.trim().is_empty() {
                    sentences.push(current.trim().to_string());
                }
                current.clear();
            }
        }
        
        if !current.trim().is_empty() {
            sentences.push(current.trim().to_string());
        }
        
        sentences
    }

    fn split_at_words(&self, text: &str, context: &str, chunks: &mut Vec<(String, String)>) {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut current = String::new();
        
        for word in words {
            if current.chars().count() + word.len() + 1 > CHUNK_HARD_LIMIT && !current.is_empty() {
                chunks.push((context.to_string(), current.trim().to_string()));
                current.clear();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        
        if !current.is_empty() {
            chunks.push((context.to_string(), current.trim().to_string()));
        }
    }

    // ========================================================================
    // FILE COLLECTION & TEXT EXTRACTION
    // ========================================================================

    async fn collect_files_recursive(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut files = Vec::new();

        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(format!("Permission denied: cannot read directory"));
            }
            Err(e) => {
                return Err(format!("Failed to read directory: {}", e));
            }
        };

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();

            // Skip hidden files and directories
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            if path.is_dir() {
                if let Ok(mut sub_files) = Box::pin(self.collect_files_recursive(&path)).await {
                    files.append(&mut sub_files);
                }
            } else if self.is_supported_file(&path) {
                files.push(path);
            }
        }

        Ok(files)
    }

    fn is_supported_file(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            matches!(
                ext.to_lowercase().as_str(),
                "txt" | "csv" | "tsv" | "md" | "json" | "pdf" | "docx"
            )
        } else {
            false
        }
    }

    fn extract_text(
        &self,
        file_path: &Path,
        content: &str,
        i: usize,
        total_files: usize,
    ) -> Result<String, String> {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "csv" => self.parse_csv(content, ','),
            "tsv" => self.parse_csv(content, '\t'),
            "json" => self.parse_json(content),
            "pdf" => self.extract_pdf_text_with_progress(file_path, i, total_files),
            "docx" => self.extract_docx_text(file_path),
            _ => Ok(content.to_string()),
        }
    }

    fn extract_pdf_text_with_progress(
        &self,
        file_path: &Path,
        file_index: usize,
        total_files: usize,
    ) -> Result<String, String> {
        // pdf-extract has better font encoding handling than raw lopdf
        // It properly handles ToUnicode CMaps and custom font encodings
        let pages = pdf_extract::extract_text_by_pages(file_path)
            .map_err(|e| format!("Failed to extract PDF text: {}", e))?;

        let total_pages = pages.len() as u32;
        let mut extracted_text = String::new();

        for (i, page_text) in pages.iter().enumerate() {
            extracted_text.push_str(page_text);
            extracted_text.push('\n');

            // Emit progress every 5 pages or on last page
            if i % 5 == 0 || i == pages.len() - 1 {
                let progress = ((i + 1) as f32 / pages.len() as f32 * 100.0) as u8;
                if let Some(ref handle) = self.app_handle {
                    let _ = handle.emit(
                        "rag-progress",
                        RagProgressEvent {
                            phase: "extracting_text".to_string(),
                            total_files,
                            processed_files: file_index,
                            total_chunks: 0,
                            processed_chunks: 0,
                            current_file: file_path.to_string_lossy().to_string(),
                            is_complete: false,
                            extraction_progress: Some(progress),
                            extraction_total_pages: Some(total_pages),
                            compute_device: None, // Not applicable during text extraction
                        },
                    );
                }
            }
        }

        Ok(extracted_text)
    }

    fn extract_docx_text(&self, file_path: &Path) -> Result<String, String> {
        use std::io::Read;

        let file = std::fs::File::open(file_path)
            .map_err(|e| format!("Failed to open DOCX: {}", e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("Invalid DOCX archive: {}", e))?;

        let mut doc_xml = archive
            .by_name("word/document.xml")
            .map_err(|_| "No document.xml found in DOCX".to_string())?;

        let mut xml_content = String::new();
        doc_xml
            .read_to_string(&mut xml_content)
            .map_err(|e| format!("Failed to read document.xml: {}", e))?;

        Ok(extract_text_from_docx_xml(&xml_content))
    }

    fn parse_csv(&self, content: &str, delimiter: char) -> Result<String, String> {
        let mut result = String::new();
        let mut lines = content.lines();

        let header: Vec<&str> = if let Some(header_line) = lines.next() {
            header_line.split(delimiter).collect()
        } else {
            return Ok(String::new());
        };

        for line in lines {
            let values: Vec<&str> = line.split(delimiter).collect();
            let mut row_text = String::new();

            for (i, value) in values.iter().enumerate() {
                if i < header.len() && !value.trim().is_empty() {
                    if !row_text.is_empty() {
                        row_text.push_str(", ");
                    }
                    row_text.push_str(&format!("{}: {}", header[i].trim(), value.trim()));
                }
            }

            if !row_text.is_empty() {
                result.push_str(&row_text);
                result.push('\n');
            }
        }

        Ok(result)
    }

    fn parse_json(&self, content: &str) -> Result<String, String> {
        match serde_json::from_str::<serde_json::Value>(content) {
            Ok(value) => Ok(self.json_to_text(&value, "")),
            Err(_) => Ok(content.to_string()),
        }
    }

    fn json_to_text(&self, value: &serde_json::Value, prefix: &str) -> String {
        match value {
            serde_json::Value::Object(map) => {
                let mut result = String::new();
                for (key, val) in map {
                    let new_prefix = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };
                    result.push_str(&self.json_to_text(val, &new_prefix));
                }
                result
            }
            serde_json::Value::Array(arr) => {
                let mut result = String::new();
                for (i, val) in arr.iter().enumerate() {
                    let new_prefix = format!("{}[{}]", prefix, i);
                    result.push_str(&self.json_to_text(val, &new_prefix));
                }
                result
            }
            serde_json::Value::String(s) => format!("{}: {}\n", prefix, s),
            serde_json::Value::Number(n) => format!("{}: {}\n", prefix, n),
            serde_json::Value::Bool(b) => format!("{}: {}\n", prefix, b),
            serde_json::Value::Null => String::new(),
        }
    }

    fn compute_hash(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Hash a directory path to create a unique, safe directory name for central cache
    fn hash_path(path: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(path.to_string_lossy().as_bytes());
        format!("{:x}", hasher.finalize())[..16].to_string() // First 16 chars
    }

    // ========================================================================
    // SEARCH
    // ========================================================================

    async fn search_documents(&self, query_vector: Vec<f32>, limit: usize) -> Vec<RagChunk> {
        let search_start = Instant::now();
        let mut all_results = Vec::new();

        // Query each connection in parallel
        for (cache_dir, conn) in &self.connections {
            let table = &conn.chunks_table;
            let query = match table.query().nearest_to(query_vector.clone()) {
                Ok(q) => q,
                Err(e) => {
                    println!(
                        "RagActor ERROR: Failed to create vector query for {:?}: {}",
                        cache_dir, e
                    );
                    continue;
                }
            };

            let mut query_stream = match query.limit(limit).execute().await {
                Ok(s) => s,
                Err(e) => {
                    println!(
                        "RagActor ERROR: Failed to execute search for {:?}: {}",
                        cache_dir, e
                    );
                    continue;
                }
            };

            while let Some(Ok(batch)) = query_stream.next().await {
                let ids = batch
                    .column_by_name("id")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>());
                let contents = batch
                    .column_by_name("content")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>());
                let heading_contexts = batch
                    .column_by_name("heading_context")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>());
                let source_files = batch
                    .column_by_name("source_file")
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>());
                let chunk_indices = batch
                    .column_by_name("chunk_index")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int64Array>());
                let distances = batch
                    .column_by_name("_distance")
                    .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

                if let (
                    Some(ids),
                    Some(contents),
                    Some(heading_contexts),
                    Some(source_files),
                    Some(chunk_indices),
                    Some(distances),
                ) = (
                    ids,
                    contents,
                    heading_contexts,
                    source_files,
                    chunk_indices,
                    distances,
                ) {
                    for i in 0..batch.num_rows() {
                        let distance = distances.value(i);
                        let score = 1.0 / (1.0 + distance);

                        // Include heading context in the returned content
                        let heading_ctx = heading_contexts.value(i);
                        let raw_content = contents.value(i);
                        let full_content = if heading_ctx.is_empty() {
                            raw_content.to_string()
                        } else {
                            format!("[Context: {}]\n\n{}", heading_ctx, raw_content)
                        };

                        all_results.push(RagChunk {
                            id: ids.value(i).to_string(),
                            content: full_content,
                            source_file: source_files.value(i).to_string(),
                            chunk_index: chunk_indices.value(i) as usize,
                            score,
                        });
                    }
                }
            }
        }

        // Sort by score and take top `limit`
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all_results.truncate(limit);

        let total_time = search_start.elapsed();
        println!(
            "RagActor: Federated search completed in {} ms ({} results across {} connections)",
            total_time.as_millis(),
            all_results.len(),
            self.connections.len()
        );

        all_results
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Extract text content from DOCX XML (word/document.xml)
fn extract_text_from_docx_xml(xml: &str) -> String {
    let mut result = String::new();
    let mut in_text = false;
    let mut chars = xml.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                tag.push(tc);
            }

            if tag.starts_with("w:t") && !tag.starts_with("w:t/") && !tag.ends_with('/') {
                in_text = true;
            } else if tag == "/w:t" {
                in_text = false;
            } else if tag.starts_with("w:p") && !tag.starts_with("w:p/") && !tag.ends_with('/') {
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
            }
        } else if in_text {
            result.push(c);
        }
    }

    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a minimal actor for testing parsing/chunking methods
    fn create_test_actor() -> RagRetrievalActor {
        let (_, rx) = mpsc::channel(1);
        RagRetrievalActor::new(rx, None)
    }

    // ========================================================================
    // HEADING STACK MANAGER TESTS
    // ========================================================================

    #[test]
    fn test_heading_stack_basic() {
        let mut stack = HeadingStackManager::new();
        
        stack.push_heading(1, "Chapter 1".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1");
        
        stack.push_heading(2, "Section A".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section A");
        
        stack.push_heading(3, "Details".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section A > H3: Details");
    }

    #[test]
    fn test_heading_stack_pops_same_level() {
        let mut stack = HeadingStackManager::new();
        
        stack.push_heading(1, "Chapter 1".to_string());
        stack.push_heading(2, "Section A".to_string());
        stack.push_heading(2, "Section B".to_string());
        
        // Section B should replace Section A
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section B");
    }

    #[test]
    fn test_heading_stack_pops_lower_levels() {
        let mut stack = HeadingStackManager::new();
        
        stack.push_heading(1, "Chapter 1".to_string());
        stack.push_heading(2, "Section A".to_string());
        stack.push_heading(3, "Details".to_string());
        stack.push_heading(2, "Section B".to_string());
        
        // Should pop H3 when H2 comes in
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section B");
    }

    #[test]
    fn test_heading_stack_new_chapter() {
        let mut stack = HeadingStackManager::new();
        
        stack.push_heading(1, "Chapter 1".to_string());
        stack.push_heading(2, "Section A".to_string());
        stack.push_heading(1, "Chapter 2".to_string());
        
        // New H1 should clear everything
        assert_eq!(stack.get_context(), "H1: Chapter 2");
    }

    // ========================================================================
    // PERMISSION & ACCESS TESTS
    // ========================================================================

    #[tokio::test]
    #[cfg(unix)] // chmod only works on Unix
    async fn test_file_no_read_permission_returns_error() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        // Create temp directory and file
        let temp_dir = tempdir().unwrap();
        let test_file = temp_dir.path().join("unreadable.txt");
        std::fs::write(&test_file, "test content").unwrap();

        // Remove read permission (write-only)
        std::fs::set_permissions(&test_file, std::fs::Permissions::from_mode(0o200)).unwrap();

        // Verify the file read fails with PermissionDenied
        let bytes_result = tokio::fs::read(&test_file).await;
        assert!(bytes_result.is_err());
        assert_eq!(
            bytes_result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );

        // Cleanup: restore permissions so tempdir can delete
        std::fs::set_permissions(&test_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_directory_no_read_permission_returns_error() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let temp_dir = tempdir().unwrap();
        let sub_dir = temp_dir.path().join("unreadable_dir");
        std::fs::create_dir(&sub_dir).unwrap();

        // Remove read+execute permission (can't list contents)
        std::fs::set_permissions(&sub_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let actor = create_test_actor();
        let result = actor.collect_files_recursive(&sub_dir).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Permission denied"));

        // Cleanup
        std::fs::set_permissions(&sub_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_readonly_directory_uses_central_cache() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let temp_dir = tempdir().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "test content").unwrap();

        // Make the directory read-only (can read files, can't create new ones)
        std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        // Verify we can't create the sidecar directory
        let sidecar_path = temp_dir.path().join(".plugable-rag-cache");
        let create_result = std::fs::create_dir(&sidecar_path);
        assert!(create_result.is_err());

        // The actor should fall back to central cache
        // Test the hash_path function
        let hash = RagRetrievalActor::hash_path(temp_dir.path());
        assert_eq!(hash.len(), 16); // First 16 chars of SHA256

        // Verify central cache path would be created
        let central_path = dirs::home_dir()
            .unwrap()
            .join(".plugable-chat")
            .join(CENTRAL_RAG_CACHE_DIR)
            .join(&hash);

        // Central cache should be writable (unless home is also readonly, which is unlikely)
        let central_create = std::fs::create_dir_all(&central_path);
        assert!(central_create.is_ok());

        // Cleanup
        std::fs::remove_dir_all(&central_path).ok();
        std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_hash_path_deterministic() {
        let path1 = Path::new("/some/test/path");
        let path2 = Path::new("/some/test/path");
        let path3 = Path::new("/different/path");

        let hash1 = RagRetrievalActor::hash_path(path1);
        let hash2 = RagRetrievalActor::hash_path(path2);
        let hash3 = RagRetrievalActor::hash_path(path3);

        assert_eq!(hash1, hash2); // Same path = same hash
        assert_ne!(hash1, hash3); // Different path = different hash
        assert_eq!(hash1.len(), 16); // Consistent length
    }

    // ========================================================================
    // MARKDOWN PARSING TESTS
    // ========================================================================

    #[test]
    fn test_parse_markdown_headings() {
        let actor = create_test_actor();
        let content = "# Title\n\nSome intro text.\n\n## Section 1\n\nContent here.\n\n### Subsection\n\nMore content.";
        
        let elements = actor.parse_markdown(content);
        
        assert!(matches!(elements[0], DocumentElement::Heading { level: 1, .. }));
        assert!(matches!(elements[1], DocumentElement::Paragraph(_)));
        assert!(matches!(elements[2], DocumentElement::Heading { level: 2, .. }));
        assert!(matches!(elements[3], DocumentElement::Paragraph(_)));
        assert!(matches!(elements[4], DocumentElement::Heading { level: 3, .. }));
    }

    #[test]
    fn test_parse_markdown_bullets() {
        let actor = create_test_actor();
        let content = "Introduction.\n\n- First item\n- Second item\n- Third item\n\nConclusion.";
        
        let elements = actor.parse_markdown(content);
        
        assert!(matches!(elements[0], DocumentElement::Paragraph(_)));
        assert!(matches!(elements[1], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[2], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[3], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[4], DocumentElement::Paragraph(_)));
    }

    #[test]
    fn test_parse_markdown_numbered_list() {
        let actor = create_test_actor();
        let content = "Steps:\n\n1. First step\n2. Second step\n3. Third step";
        
        let elements = actor.parse_markdown(content);
        
        assert!(matches!(elements[0], DocumentElement::Paragraph(_)));
        assert!(matches!(elements[1], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[2], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[3], DocumentElement::ListItem { .. }));
    }

    #[test]
    fn test_parse_markdown_code_blocks() {
        let actor = create_test_actor();
        let content = "Example:\n\n```rust\nfn main() {\n    println!(\"Hello\");\n}\n```\n\nEnd.";
        
        let elements = actor.parse_markdown(content);
        
        assert!(matches!(elements[0], DocumentElement::Paragraph(_)));
        assert!(matches!(elements[1], DocumentElement::CodeBlock(_)));
        assert!(matches!(elements[2], DocumentElement::Paragraph(_)));
        
        if let DocumentElement::CodeBlock(code) = &elements[1] {
            assert!(code.contains("fn main()"));
        }
    }

    // ========================================================================
    // SEMANTIC CHUNKING TESTS
    // ========================================================================

    #[test]
    fn test_semantic_chunk_with_headings() {
        let actor = create_test_actor();
        let content = "# Guide\n\nIntroduction paragraph.\n\n## Installation\n\nInstallation steps here.\n\n## Usage\n\nUsage instructions.";
        
        let elements = actor.parse_markdown(content);
        let chunks = actor.semantic_chunk(&elements);
        
        // Should have chunks with heading contexts
        assert!(!chunks.is_empty());
        
        // First chunk should have H1 context
        let (ctx, _content) = &chunks[0];
        assert!(ctx.contains("H1: Guide"));
        
        // Check that later chunks have hierarchical context
        let has_installation_context = chunks.iter().any(|(ctx, _)| ctx.contains("Installation"));
        assert!(has_installation_context);
    }

    #[test]
    fn test_semantic_chunk_respects_soft_limit() {
        let actor = create_test_actor();
        
        // Create content with multiple short paragraphs
        let content = "# Test\n\nParagraph one is short.\n\nParagraph two is also short.\n\nParagraph three continues.\n\nParagraph four ends this.";
        
        let elements = actor.parse_markdown(content);
        let chunks = actor.semantic_chunk(&elements);
        
        // Chunks should be created
        assert!(!chunks.is_empty());
        
        // Each chunk should not exceed hard limit
        for (_, content) in &chunks {
            assert!(content.chars().count() <= CHUNK_HARD_LIMIT, 
                "Chunk exceeded hard limit: {} chars", content.chars().count());
        }
    }

    #[test]
    fn test_semantic_chunk_long_paragraph_splits_at_sentences() {
        let actor = create_test_actor();
        
        // Create a very long paragraph (exceeds hard limit)
        let long_sentence = "This is a moderately long sentence that will be repeated many times to create a very long paragraph. ";
        let long_para = long_sentence.repeat(20); // ~2000 chars
        
        let elements = vec![DocumentElement::Paragraph(long_para)];
        let chunks = actor.semantic_chunk(&elements);
        
        // Should be split into multiple chunks
        assert!(chunks.len() > 1, "Long paragraph should be split into multiple chunks");
        
        // Each chunk should not exceed hard limit
        for (_, content) in &chunks {
            assert!(content.chars().count() <= CHUNK_HARD_LIMIT,
                "Chunk exceeded hard limit: {} chars", content.chars().count());
        }
    }

    #[test]
    fn test_semantic_chunk_preserves_bullets() {
        let actor = create_test_actor();
        let content = "# Features\n\n- Feature one is great\n- Feature two is better\n- Feature three is best";
        
        let elements = actor.parse_markdown(content);
        let chunks = actor.semantic_chunk(&elements);
        
        // Should have bullet points in content
        let all_content: String = chunks.iter().map(|(_, c)| c.as_str()).collect();
        assert!(all_content.contains("•"), "Bullets should be preserved");
    }

    // ========================================================================
    // SPLIT FUNCTIONS TESTS
    // ========================================================================

    #[test]
    fn test_split_into_sentences() {
        let actor = create_test_actor();
        
        let text = "First sentence. Second sentence! Third sentence? Fourth sentence.";
        let sentences = actor.split_into_sentences(text);
        
        assert_eq!(sentences.len(), 4);
        assert_eq!(sentences[0], "First sentence.");
        assert_eq!(sentences[1], "Second sentence!");
        assert_eq!(sentences[2], "Third sentence?");
        assert_eq!(sentences[3], "Fourth sentence.");
    }

    #[test]
    fn test_split_into_sentences_with_newlines() {
        let actor = create_test_actor();
        
        let text = "Line one\nLine two\nLine three";
        let sentences = actor.split_into_sentences(text);
        
        assert_eq!(sentences.len(), 3);
    }

    // ========================================================================
    // PDF/TXT HEURISTIC TESTS
    // ========================================================================

    #[test]
    fn test_pdf_heading_detection_all_caps() {
        let actor = create_test_actor();
        
        assert!(actor.looks_like_heading_pdf("INTRODUCTION", None));
        assert!(actor.looks_like_heading_pdf("CHAPTER ONE", Some("This is content")));
        assert!(!actor.looks_like_heading_pdf("This is a normal sentence.", None));
    }

    #[test]
    fn test_txt_heading_detection_underline() {
        let actor = create_test_actor();
        
        assert!(actor.looks_like_heading_txt("Chapter Title", Some("==================")));
        assert!(actor.looks_like_heading_txt("Section Name", Some("------------------")));
        assert!(!actor.looks_like_heading_txt("Normal text here.", Some("More normal text.")));
    }

    // ========================================================================
    // CRC32 TESTS
    // ========================================================================

    #[test]
    fn test_crc32_deterministic() {
        let content = b"Hello, World!";
        let crc1 = crc32fast::hash(content);
        let crc2 = crc32fast::hash(content);
        
        assert_eq!(crc1, crc2);
    }

    #[test]
    fn test_crc32_different_for_different_content() {
        let content1 = b"Hello, World!";
        let content2 = b"Hello, World?";
        
        let crc1 = crc32fast::hash(content1);
        let crc2 = crc32fast::hash(content2);
        
        assert_ne!(crc1, crc2);
    }

    // ========================================================================
    // EDGE CASES
    // ========================================================================

    #[test]
    fn test_empty_content() {
        let actor = create_test_actor();
        
        let elements = actor.parse_markdown("");
        assert!(elements.is_empty());
        
        let chunks = actor.semantic_chunk(&elements);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_only_headings() {
        let actor = create_test_actor();
        let content = "# H1\n\n## H2\n\n### H3";
        
        let elements = actor.parse_markdown(content);
        let chunks = actor.semantic_chunk(&elements);
        
        // Headings alone don't create content chunks
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_heading_context_updates_correctly() {
        let actor = create_test_actor();
        let content = "# Chapter 1\n\nFirst chapter content.\n\n# Chapter 2\n\nSecond chapter content.";
        
        let elements = actor.parse_markdown(content);
        let chunks = actor.semantic_chunk(&elements);
        
        assert!(chunks.len() >= 2);
        
        // First chunk should have Chapter 1 context
        assert!(chunks[0].0.contains("Chapter 1"), "First chunk should have Chapter 1 context");
        
        // Second chunk should have Chapter 2 context (not Chapter 1)
        let ch2_chunk = chunks.iter().find(|(ctx, _)| ctx.contains("Chapter 2"));
        assert!(ch2_chunk.is_some(), "Should have a chunk with Chapter 2 context");
        assert!(!ch2_chunk.unwrap().0.contains("Chapter 1"), "Chapter 2 chunk should not have Chapter 1");
    }
}
