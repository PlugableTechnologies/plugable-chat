//! RAG Retrieval Actor for document indexing and semantic search.
//!
//! This module contains the main `RagRetrievalActor` which handles:
//! - Document indexing with embedding generation
//! - Semantic search across indexed documents
//! - File and directory management for RAG context

use crate::protocol::{
    FileError, RagChunk, RagIndexResult, RagMsg, RagProgressEvent, RemoveFileResult,
};
#[cfg(test)]
use sha2::{Sha256, Digest};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::Schema;
use fastembed::TextEmbedding;
use futures::StreamExt;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::Table;
use lru::LruCache;
// sha2 is available if needed for cache key hashing in the future
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tauri::AppHandle;
use tauri::Emitter;
use tokio::sync::mpsc;

// Import from sibling modules
use super::cache_manager::{
    compute_content_hash, ensure_lancedb_connection_for_path, get_rag_chunks_schema,
    get_rag_file_cache_schema, get_rag_sidecar_cache_path, load_file_cache_entries_from_table,
    save_file_cache_entries_to_table, should_reindex_file_by_crc, DirectoryConnection,
    FileCacheEntry, IndexedChunk, EMBEDDING_LRU_CAPACITY,
};
use super::document_chunker::create_semantic_chunks;
use super::file_processor::{
    extract_text_from_file, is_rag_supported_file_type, parse_document_to_elements,
};

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

impl RagRetrievalActor {
    pub fn new(rx: mpsc::Receiver<RagMsg>, app_handle: Option<AppHandle>) -> Self {
        Self {
            rx,
            connections: HashMap::new(),
            app_handle,
            embedding_lru_cache: LruCache::new(
                NonZeroUsize::new(EMBEDDING_LRU_CAPACITY).unwrap(),
            ),
        }
    }

    // ========================================================================
    // DATABASE INITIALIZATION & SIDECAR MANAGEMENT
    // ========================================================================

    /// Helper to derive the sidecar cache path from a document path.
    fn get_cache_dir_for_file(&self, file_path: &Path) -> PathBuf {
        get_rag_sidecar_cache_path(file_path)
    }

    /// On-demand connection creation for a specific path.
    async fn ensure_connection_for_path(
        &mut self,
        file_path: &Path,
    ) -> Result<&mut DirectoryConnection, String> {
        let cache_dir = ensure_lancedb_connection_for_path(&mut self.connections, file_path).await?;
        Ok(self.connections.get_mut(&cache_dir).unwrap())
    }

    fn chunks_schema(&self) -> Arc<Schema> {
        get_rag_chunks_schema()
    }

    #[allow(dead_code)]
    fn file_cache_schema(&self) -> Arc<Schema> {
        get_rag_file_cache_schema()
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
                    println!(
                        "RagActor: Processing {} paths ({})",
                        paths.len(),
                        if use_gpu { "GPU" } else { "CPU" }
                    );
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
                println!(
                    "RagActor ERROR: Failed to clear chunks in {:?}: {}",
                    cache_dir, e
                );
                success = false;
            }
            if let Err(e) = conn.file_cache_table.delete("1=1").await {
                println!(
                    "RagActor ERROR: Failed to clear file cache in {:?}: {}",
                    cache_dir, e
                );
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
        let mut all_files = HashSet::new();

        for conn in self.connections.values() {
            if let Ok(mut query) = conn
                .file_cache_table
                .query()
                .select(Select::Columns(vec!["file_path".to_string()]))
                .execute()
                .await
            {
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

    async fn get_file_cache_from_table(
        &self,
        table: &Table,
        file_path: &str,
    ) -> Option<FileCacheEntry> {
        load_file_cache_entries_from_table(table, file_path).await
    }

    async fn save_file_cache_to_table(
        &self,
        table: &Table,
        entry: &FileCacheEntry,
    ) -> Result<(), String> {
        save_file_cache_entries_to_table(table, entry).await
    }

    fn should_reindex_file(&self, current_crc: u32, cached: Option<&FileCacheEntry>) -> bool {
        should_reindex_file_by_crc(current_crc, cached)
    }

    // ========================================================================
    // BATCH EMBEDDING CACHE OPERATIONS
    // ========================================================================

    /// Batch lookup of cached embeddings - returns HashMap of hash -> vector
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

        // Look up remaining hashes from ALL known connections
        for conn in self.connections.values() {
            if db_lookup_needed.is_empty() {
                break;
            }

            // Build a query for all needed hashes
            let hash_conditions: Vec<String> = db_lookup_needed
                .iter()
                .map(|h| format!("hash = '{}'", h.replace("'", "''")))
                .collect();
            let filter = hash_conditions.join(" OR ");

            let query = conn.chunks_table.query().only_if(&filter);
            if let Ok(mut stream) = query.execute().await {
                while let Some(Ok(batch)) = stream.next().await {
                    let hashes_col = batch
                        .column_by_name("hash")
                        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
                    let vectors_col = batch
                        .column_by_name("vector")
                        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());

                    if let (Some(hashes_arr), Some(vectors_arr)) = (hashes_col, vectors_col) {
                        for i in 0..batch.num_rows() {
                            let hash = hashes_arr.value(i).to_string();
                            if db_lookup_needed.contains(&hash) {
                                if let Some(vector_list) = vectors_arr.value(i).as_any().downcast_ref::<Float32Array>() {
                                    let vector: Vec<f32> = vector_list.values().to_vec();
                                    result.insert(hash.clone(), vector);
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

    fn compute_hash(&self, content: &str) -> String {
        compute_content_hash(content)
    }

    // ========================================================================
    // FILE COLLECTION & TEXT EXTRACTION
    // ========================================================================

    async fn collect_files_recursive(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut files = Vec::new();

        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err("Permission denied: cannot read directory".to_string());
            }
            Err(e) => {
                return Err(format!("Failed to read directory: {}", e));
            }
        };

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Skip hidden files and directories
            if file_name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                match Box::pin(self.collect_files_recursive(&path)).await {
                    Ok(sub_files) => files.extend(sub_files),
                    Err(e) => {
                        println!("RagActor: Skipping directory {:?}: {}", path, e);
                    }
                }
            } else if is_rag_supported_file_type(&path) {
                files.push(path);
            }
        }

        Ok(files)
    }

    #[allow(dead_code)]
    fn is_supported_file(&self, path: &Path) -> bool {
        is_rag_supported_file_type(path)
    }

    fn extract_text(
        &self,
        file_path: &Path,
        content: &str,
        i: usize,
        total_files: usize,
    ) -> Result<String, String> {
        extract_text_from_file(file_path, content, i, total_files, self.app_handle.as_ref())
    }

    // ========================================================================
    // DOCUMENT PARSING
    // ========================================================================

    fn parse_document(
        &self,
        extension: &str,
        content: &str,
        file_path: Option<&Path>,
    ) -> Vec<super::document_chunker::DocumentElement> {
        parse_document_to_elements(extension, content, file_path)
    }

    fn semantic_chunk(
        &self,
        elements: &[super::document_chunker::DocumentElement],
    ) -> Vec<(String, String)> {
        create_semantic_chunks(elements)
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
                    // Pass file_path for PDF hybrid structure extraction
                    let elements = self.parse_document(&ext, &text_content, Some(file_path));

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
                
                // Calculate batch size metrics for diagnostics
                let batch_total_chars: usize = texts.iter().map(|t| t.len()).sum();
                let _batch_total_bytes: usize = texts.iter().map(|t| t.as_bytes().len()).sum();
                let batch_avg_chars = if batch_size > 0 { batch_total_chars / batch_size } else { 0 };
                
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
                let chars_per_sec = if batch_elapsed.as_secs_f64() > 0.0 {
                    batch_total_chars as f64 / batch_elapsed.as_secs_f64()
                } else {
                    0.0
                };
                
                println!(
                    "RagActor: [{}] Batch {} ({} chunks, {} chars, avg {} chars/chunk, {:.0} chars/sec) in {:?} | Progress: {}/{} ({}%)",
                    compute_device,
                    batch_count,
                    batch_size,
                    batch_total_chars,
                    batch_avg_chars,
                    chars_per_sec,
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

    #[cfg(test)]
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
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::document_chunker::{
        create_semantic_chunks, split_text_into_sentences, DocumentElement, HeadingStackManager, CHUNK_HARD_LIMIT,
    };
    use super::super::file_processor::{
        detect_pdf_heading_level_from_text, looks_like_plaintext_heading, merge_pdf_headings_with_content,
        parse_markdown_to_elements, parse_pdf_to_elements,
    };
    use super::super::pdf_extractor::{normalize_heading_text_for_matching, PdfHeading};

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
        let central_path = crate::paths::get_central_rag_cache_dir().join(&hash);

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
        
        let elements = parse_markdown_to_elements(content);
        
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
        
        let elements = parse_markdown_to_elements(content);
        
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
        
        let elements = parse_markdown_to_elements(content);
        
        assert!(matches!(elements[0], DocumentElement::Paragraph(_)));
        assert!(matches!(elements[1], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[2], DocumentElement::ListItem { .. }));
        assert!(matches!(elements[3], DocumentElement::ListItem { .. }));
    }

    #[test]
    fn test_parse_markdown_code_blocks() {
        let actor = create_test_actor();
        let content = "Example:\n\n```rust\nfn main() {\n    println!(\"Hello\");\n}\n```\n\nEnd.";
        
        let elements = parse_markdown_to_elements(content);
        
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
        
        let elements = parse_markdown_to_elements(content);
        let chunks = create_semantic_chunks(&elements);
        
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
        
        let elements = parse_markdown_to_elements(content);
        let chunks = create_semantic_chunks(&elements);
        
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
        let chunks = create_semantic_chunks(&elements);
        
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
        
        let elements = parse_markdown_to_elements(content);
        let chunks = create_semantic_chunks(&elements);
        
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
        let sentences = split_text_into_sentences(text);
        
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
        let sentences = split_text_into_sentences(text);
        
        assert_eq!(sentences.len(), 3);
    }

    // ========================================================================
    // PDF/TXT HEURISTIC TESTS
    // ========================================================================

    #[test]
    fn test_pdf_heading_level_h1_all_caps_standalone() {
        let actor = create_test_actor();
        
        // H1: ALL CAPS, standalone (surrounded by blank lines)
        assert_eq!(detect_pdf_heading_level_from_text("INTRODUCTION", true, Some("")), Some(1));
        assert_eq!(detect_pdf_heading_level_from_text("CHAPTER ONE", true, Some("")), Some(1));
        assert_eq!(detect_pdf_heading_level_from_text("SUMMARY", true, Some("")), Some(1));
        
        // Not H1 if not standalone (becomes H2)
        assert_eq!(detect_pdf_heading_level_from_text("INTRODUCTION", false, Some("")), Some(2));
        assert_eq!(detect_pdf_heading_level_from_text("INTRODUCTION", true, Some("content follows")), Some(2));
    }

    #[test]
    fn test_pdf_heading_level_h2_caps_or_title() {
        let actor = create_test_actor();
        
        // H2: ALL CAPS not standalone
        assert_eq!(detect_pdf_heading_level_from_text("TECHNICAL OVERVIEW", false, Some("Details")), Some(2));
        assert_eq!(detect_pdf_heading_level_from_text("KEY FEATURES", true, Some("Feature list")), Some(2));
        
        // H2: Short Title Case, standalone (preceded by blank)
        assert_eq!(detect_pdf_heading_level_from_text("Product Overview", true, Some("")), Some(2));
        assert_eq!(detect_pdf_heading_level_from_text("Key Features", true, Some("")), Some(2));
    }

    #[test]
    fn test_pdf_heading_level_h3_title_case() {
        let actor = create_test_actor();
        
        // H3: Title Case, medium length
        assert_eq!(detect_pdf_heading_level_from_text("Display Specifications", false, Some("")), Some(3));
        assert_eq!(detect_pdf_heading_level_from_text("Power Management Options", false, Some("")), Some(3));
        
        // H3: Lines ending with colon (sub-section markers)
        assert_eq!(detect_pdf_heading_level_from_text("Features:", false, Some("")), Some(3));
        assert_eq!(detect_pdf_heading_level_from_text("System Requirements:", false, Some("")), Some(3));
    }

    #[test]
    fn test_pdf_heading_level_h4_short_labels() {
        let actor = create_test_actor();
        
        // H4: Short lines that look like labels but don't match H2/H3 patterns
        // These have first word capitalized but aren't full Title Case
        assert_eq!(detect_pdf_heading_level_from_text("Memory and storage", false, Some("")), Some(4));
        assert_eq!(detect_pdf_heading_level_from_text("Ports available", false, Some("")), Some(4));
        
        // Note: Full Title Case like "Memory Configuration" is detected as H3
        // This is expected since we can't distinguish H3/H4 without font info
        assert_eq!(detect_pdf_heading_level_from_text("Memory Configuration", false, Some("")), Some(3));
    }

    #[test]
    fn test_pdf_heading_level_not_heading() {
        let actor = create_test_actor();
        
        // Sentences ending with punctuation are not headings
        assert_eq!(detect_pdf_heading_level_from_text("This is a complete sentence.", false, Some("")), None);
        assert_eq!(detect_pdf_heading_level_from_text("What is this?", false, Some("")), None);
        assert_eq!(detect_pdf_heading_level_from_text("Amazing!", false, Some("")), None);
        
        // Too short
        assert_eq!(detect_pdf_heading_level_from_text("Hi", false, Some("")), None);
        
        // Too long
        let long_line = "A".repeat(110);
        assert_eq!(detect_pdf_heading_level_from_text(&long_line, false, Some("")), None);
    }

    #[test]
    fn test_pdf_parse_elements_multilevel_hierarchy() {
        let actor = create_test_actor();
        
        // Simulate a typical document structure with multiple heading levels
        let content = r#"
CHAPTER ONE

Introduction

Overview:

The Background

This is a paragraph of content that should not be detected as a heading because it is longer and more prose-like in nature.
"#;
        
        // Test with None for file_path to use text-based heuristics
        let elements = parse_pdf_to_elements(content, None);
        
        // Verify we get multiple heading levels
        let h1_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { level: 1, .. })).count();
        let h2_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { level: 2, .. })).count();
        let h3_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { level: 3, .. })).count();
        let para_count = elements.iter().filter(|e| matches!(e, DocumentElement::Paragraph(_))).count();
        
        // Should detect multiple heading levels
        assert!(h1_count >= 1, "Should detect at least 1 H1 heading (ALL CAPS standalone)");
        assert!(h2_count + h3_count >= 1, "Should detect H2 or H3 headings");
        assert!(para_count >= 1, "Should detect paragraph content");
    }

    #[test]
    fn test_pdf_hierarchy_context_multilevel() {
        let actor = create_test_actor();
        
        // Test that chunking produces multi-level context
        let content = r#"
MAIN SECTION

Subsection Title

Details And Specifications

This is content that should be associated with the heading hierarchy above it.
"#;
        
        // Test with None for file_path to use text-based heuristics
        let elements = parse_pdf_to_elements(content, None);
        let chunks = create_semantic_chunks(&elements);
        
        // At least one chunk should have multi-level context (contains " > ")
        let has_multilevel = chunks.iter().any(|(ctx, _)| ctx.contains(" > "));
        assert!(has_multilevel, "Should produce chunks with multi-level hierarchy context. Chunks: {:?}", 
            chunks.iter().map(|(ctx, _)| ctx).collect::<Vec<_>>());
    }

    // ========================================================================
    // HYBRID PDF EXTRACTION TESTS
    // ========================================================================

    #[test]
    fn test_pdf_heading_struct() {
        // Test PdfHeading struct creation
        let heading = PdfHeading {
            level: 1,
            title: "Chapter 1".to_string(),
            page: Some(1),
        };
        assert_eq!(heading.level, 1);
        assert_eq!(heading.title, "Chapter 1");
    }

    #[test]
    fn test_normalize_heading_text_for_matching() {
        // Test text normalization
        assert_eq!(normalize_heading_text_for_matching("  Hello   World  "), "hello world");
        assert_eq!(normalize_heading_text_for_matching("CHAPTER ONE"), "chapter one");
        assert_eq!(normalize_heading_text_for_matching("A\t\tB"), "a b");
    }

    #[test]
    fn test_merge_headings_with_content() {
        let actor = create_test_actor();
        
        // Simulate headings from bookmarks
        let headings = vec![
            PdfHeading { level: 1, title: "Introduction".to_string(), page: Some(1) },
            PdfHeading { level: 2, title: "Background".to_string(), page: Some(2) },
            PdfHeading { level: 3, title: "Details".to_string(), page: Some(3) },
        ];
        
        let content = r#"
Introduction

This is the introduction paragraph.

Background

Some background information here.

Details

Detailed explanation follows.
"#;
        
        let elements = merge_pdf_headings_with_content(&headings, content);
        
        // Should find the headings
        let h1_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { level: 1, .. })).count();
        let h2_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { level: 2, .. })).count();
        let h3_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { level: 3, .. })).count();
        let para_count = elements.iter().filter(|e| matches!(e, DocumentElement::Paragraph(_))).count();
        
        assert_eq!(h1_count, 1, "Should detect 1 H1 heading");
        assert_eq!(h2_count, 1, "Should detect 1 H2 heading");
        assert_eq!(h3_count, 1, "Should detect 1 H3 heading");
        assert!(para_count >= 3, "Should detect paragraph content");
    }

    #[test]
    fn test_hybrid_extraction_fallback() {
        let actor = create_test_actor();
        
        // Test with None for file_path - should use heuristics fallback
        let content = r#"
MAIN SECTION

Subsection Title

This is paragraph content.
"#;
        
        let elements = parse_pdf_to_elements(content, None);
        
        // Should still detect headings via heuristics
        let heading_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { .. })).count();
        assert!(heading_count >= 1, "Should detect headings via heuristics fallback");
    }

    #[test]
    fn test_txt_heading_detection_underline() {
        let actor = create_test_actor();
        
        assert!(looks_like_plaintext_heading("Chapter Title", Some("==================")));
        assert!(looks_like_plaintext_heading("Section Name", Some("------------------")));
        assert!(!looks_like_plaintext_heading("Normal text here.", Some("More normal text.")));
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
        
        let elements = parse_markdown_to_elements("");
        assert!(elements.is_empty());
        
        let chunks = create_semantic_chunks(&elements);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_only_headings() {
        let actor = create_test_actor();
        let content = "# H1\n\n## H2\n\n### H3";
        
        let elements = parse_markdown_to_elements(content);
        let chunks = create_semantic_chunks(&elements);
        
        // Headings alone don't create content chunks
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_heading_context_updates_correctly() {
        let actor = create_test_actor();
        let content = "# Chapter 1\n\nFirst chapter content.\n\n# Chapter 2\n\nSecond chapter content.";
        
        let elements = parse_markdown_to_elements(content);
        let chunks = create_semantic_chunks(&elements);
        
        assert!(chunks.len() >= 2);
        
        // First chunk should have Chapter 1 context
        assert!(chunks[0].0.contains("Chapter 1"), "First chunk should have Chapter 1 context");
        
        // Second chunk should have Chapter 2 context (not Chapter 1)
        let ch2_chunk = chunks.iter().find(|(ctx, _)| ctx.contains("Chapter 2"));
        assert!(ch2_chunk.is_some(), "Should have a chunk with Chapter 2 context");
        assert!(!ch2_chunk.unwrap().0.contains("Chapter 1"), "Chapter 2 chunk should not have Chapter 1");
    }
}
