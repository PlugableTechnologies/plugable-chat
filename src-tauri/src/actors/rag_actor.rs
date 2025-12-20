use crate::protocol::{RagChunk, RagIndexResult, RagMsg, RagProgressEvent, RemoveFileResult};
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
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use tauri::Emitter;
use tokio::sync::mpsc;

/// Chunk size in characters
const CHUNK_SIZE: usize = 500;

/// Overlap between chunks in characters
const CHUNK_OVERLAP: usize = 100;

/// The name of the table in LanceDB for RAG chunks
const RAG_CHUNKS_TABLE: &str = "rag_chunks";

/// A document chunk with its embedding
#[derive(Clone)]
struct IndexedChunk {
    id: String,
    content: String,
    source_file: String,
    chunk_index: usize,
    vector: Vec<f32>,
}

/// The RAG Actor handles document processing and retrieval
pub struct RagRetrievalActor {
    rx: mpsc::Receiver<RagMsg>,
    /// LanceDB connection
    db: Option<Connection>,
    /// Table handle for RAG chunks
    table: Option<Table>,
    /// App handle for emitting events
    app_handle: Option<AppHandle>,
    /// Path to LanceDB directory
    db_path: PathBuf,
}

impl RagRetrievalActor {
    pub fn new(rx: mpsc::Receiver<RagMsg>, db_path: PathBuf, app_handle: Option<AppHandle>) -> Self {
        Self {
            rx,
            db: None,
            table: None,
            app_handle,
            db_path,
        }
    }

    async fn init_db(&mut self) -> Result<(), String> {
        let db_path_str = self.db_path.to_string_lossy().to_string();
        let db = connect(&db_path_str)
            .execute()
            .await
            .map_err(|e| format!("Failed to connect to LanceDB: {}", e))?;

        let schema = self.expected_schema();
        
        // Ensure table exists
        let table = if db.table_names().execute().await.map_err(|e| e.to_string())?.contains(&RAG_CHUNKS_TABLE.to_string()) {
            db.open_table(RAG_CHUNKS_TABLE).execute().await.map_err(|e| e.to_string())?
        } else {
            let batch = RecordBatch::new_empty(schema.clone());
            db.create_table(
                RAG_CHUNKS_TABLE,
                RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema),
            )
            .execute()
            .await
            .map_err(|e| e.to_string())?
        };

        self.db = Some(db);
        self.table = Some(table.clone());

        // Create scalar indexes for faster lookups
        // Note: In LanceDB 0.4, create_index on a non-vector column creates a scalar index
        let _ = table.create_index(&["id"], Index::Auto).execute().await;
        let _ = table.create_index(&["hash"], Index::Auto).execute().await;

        Ok(())
    }

    fn expected_schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("hash", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("source_file", DataType::Utf8, false),
            Field::new("chunk_index", DataType::Int64, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 384),
                true,
            ),
        ]))
    }

    pub async fn run(mut self) {
        // Initialize LanceDB
        if let Err(e) = self.init_db().await {
            println!("RagActor ERROR: Failed to initialize database: {}", e);
        }

        while let Some(msg) = self.rx.recv().await {
            match msg {
                RagMsg::IndexRagDocuments {
                    paths,
                    embedding_model,
                    respond_to,
                } => {
                    println!("RagActor: Processing {} paths", paths.len());
                    let result = self.process_documents(paths, embedding_model).await;
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
                    let result = if let Some(ref table) = self.table {
                        match table.delete("1=1").await {
                            Ok(_) => true,
                            Err(e) => {
                                println!("RagActor ERROR: Failed to clear context: {}", e);
                                false
                            }
                        }
                    } else {
                        false
                    };
                    let _ = respond_to.send(result);
                }
                RagMsg::RemoveFile {
                    source_file,
                    respond_to,
                } => {
                    println!("RagActor: Removing file from index: {}", source_file);
                    let result = if let Some(ref table) = self.table {
                        let filter = format!("source_file = '{}'", source_file.replace("'", "''"));
                        match table.delete(&filter).await {
                            Ok(_) => {
                                // Get remaining count
                                let count = self.get_total_chunks().await;
                                RemoveFileResult {
                                    chunks_removed: 0, // LanceDB doesn't return count easily on delete
                                    remaining_chunks: count,
                                }
                            }
                            Err(e) => {
                                println!("RagActor ERROR: Failed to remove file: {}", e);
                                RemoveFileResult {
                                    chunks_removed: 0,
                                    remaining_chunks: self.get_total_chunks().await,
                                }
                            }
                        }
                    } else {
                        RemoveFileResult {
                            chunks_removed: 0,
                            remaining_chunks: 0,
                        }
                    };
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

    async fn get_total_chunks(&self) -> usize {
        if let Some(ref table) = self.table {
            if let Ok(count) = table.count_rows(None).await {
                return count;
            }
        }
        0
    }

    async fn get_indexed_files(&self) -> Vec<String> {
        if let Some(ref table) = self.table {
            let mut files = std::collections::HashSet::new();
            if let Ok(mut query) = table.query().select(Select::Columns(vec!["source_file".to_string()])).execute().await {
                while let Some(Ok(batch)) = query.next().await {
                    if let Some(col) = batch.column_by_name("source_file") {
                        if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
                            for i in 0..arr.len() {
                                files.insert(arr.value(i).to_string());
                            }
                        }
                    }
                }
            }
            return files.into_iter().collect();
        }
        Vec::new()
    }

    async fn process_documents(
        &mut self,
        paths: Vec<String>,
        embedding_model: Arc<TextEmbedding>,
    ) -> Result<RagIndexResult, String> {
        let indexing_start = Instant::now();
        let mut cache_hits = 0;
        let mut files_processed_count = 0;

        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!("║                    RAG INDEXING STARTED                      ║");
        println!("╚══════════════════════════════════════════════════════════════╝");

        // Collect all files to process
        let mut files_to_process: Vec<PathBuf> = Vec::new();
        for path_str in &paths {
            let path = Path::new(path_str);
            if path.is_dir() {
                // Recursively collect files from directory
                if let Ok(entries) = self.collect_files_recursive(path).await {
                    files_to_process.extend(entries);
                }
            } else if path.is_file() {
                files_to_process.push(path.to_path_buf());
            }
        }

        println!(
            "RagActor: Found {} files to process",
            files_to_process.len()
        );

        // Load existing chunk IDs for the files we are about to process
        let mut existing_ids = std::collections::HashSet::new();
        if let Some(ref table) = self.table {
            let file_names: Vec<String> = files_to_process.iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .map(|s| s.to_string())
                .collect();
            
            if !file_names.is_empty() {
                // If there are many files, we process in batches to avoid huge filters
                for chunk in file_names.chunks(100) {
                    let filter = chunk.iter()
                        .map(|f| format!("source_file = '{}'", f.replace("'", "''")))
                        .collect::<Vec<_>>()
                        .join(" OR ");
                    
                    if let Ok(mut query) = table.query()
                        .only_if(filter)
                        .select(Select::Columns(vec!["id".to_string()]))
                        .execute()
                        .await 
                    {
                        while let Some(Ok(batch)) = query.next().await {
                            if let Some(col) = batch.column_by_name("id") {
                                if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
                                    for i in 0..arr.len() {
                                        existing_ids.insert(arr.value(i).to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Pre-process: Collect all chunks from all files
        struct PendingChunk {
            hash: String,
            content: String,
            source_file: String,
            chunk_index: usize,
        }

        let mut pending_chunks = Vec::new();
        for file_path in &files_to_process {
            let ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let is_binary = ext == "pdf" || ext == "docx";

            let content = if is_binary {
                // Binary files: extract_text reads from disk
                String::new()
            } else {
                match tokio::fs::read_to_string(file_path).await {
                    Ok(s) => s,
                    Err(e) => {
                        println!("RagActor: Error reading {:?}: {}", file_path, e);
                        continue;
                    }
                }
            };

            if let Ok(text_content) = self.extract_text(file_path, &content) {
                let chunks = self.chunk_text(&text_content);
                let source_file_name = file_path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                for (idx, chunk_content) in chunks.into_iter().enumerate() {
                    let hash = self.compute_hash(&chunk_content);
                    pending_chunks.push(PendingChunk {
                        hash,
                        content: chunk_content,
                        source_file: source_file_name.clone(),
                        chunk_index: idx,
                    });
                }
                files_processed_count += 1;
            }
        }

        let total_chunks = pending_chunks.len();
        println!("RagActor: Total chunks to process: {}", total_chunks);

        // Process chunks one by one
        let mut processed_chunks = 0;
        let mut session_hash_cache: HashMap<String, Vec<f32>> = HashMap::new();

        for chunk in pending_chunks {
            let chunk_id = format!("{}:{}:{}", chunk.hash, chunk.source_file, chunk.chunk_index);
            
            // 1. Skip if already processed for this file (uses in-memory HashSet from pre-pass)
            if existing_ids.contains(&chunk_id) {
                processed_chunks += 1;
                cache_hits += 1;
                continue;
            }

            // 2. Check if we have an embedding for this content anywhere
            if let Some(v) = session_hash_cache.get(&chunk.hash) {
                cache_hits += 1;
                let v = v.clone();
                // Save new association for this file
                self.save_chunk_to_db(&chunk_id, &chunk.hash, &chunk.content, &chunk.source_file, chunk.chunk_index, &v).await?;
            } else if let Some(v) = self.get_cached_embedding(&chunk.hash).await {
                cache_hits += 1;
                session_hash_cache.insert(chunk.hash.clone(), v.clone());
                // Save new association for this file
                self.save_chunk_to_db(&chunk_id, &chunk.hash, &chunk.content, &chunk.source_file, chunk.chunk_index, &v).await?;
            } else {
                // 3. Generate embedding
                let model = Arc::clone(&embedding_model);
                let content = chunk.content.clone();
                let v = tokio::task::spawn_blocking(move || {
                    model.embed(vec![content], None)
                })
                .await
                .map_err(|e| format!("Embedding task failed: {}", e))?
                .map_err(|e| format!("Embedding generation failed: {}", e))?
                .into_iter()
                .next()
                .ok_or("Failed to get embedding")?;

                session_hash_cache.insert(chunk.hash.clone(), v.clone());

                // 4. Save to LanceDB
                self.save_chunk_to_db(&chunk_id, &chunk.hash, &chunk.content, &chunk.source_file, chunk.chunk_index, &v).await?;
            };

            processed_chunks += 1;
            
            // Emit progress event
            if let Some(ref handle) = self.app_handle {
                let _ = handle.emit("rag-progress", RagProgressEvent {
                    total_chunks,
                    processed_chunks,
                    current_file: chunk.source_file.clone(),
                    is_complete: processed_chunks == total_chunks,
                });
            }

            if processed_chunks % 100 == 0 || processed_chunks == total_chunks {
                println!("RagActor: Progress: {}/{} chunks", processed_chunks, total_chunks);
            }
        }

        let total_time = indexing_start.elapsed();
        println!("RagActor: Indexing complete in {} ms", total_time.as_millis());

        Ok(RagIndexResult {
            total_chunks,
            files_processed: files_processed_count,
            cache_hits,
        })
    }

    async fn chunk_exists(&self, chunk_id: &str) -> bool {
        let table = match self.table.as_ref() {
            Some(t) => t,
            None => return false,
        };
        // Escape single quotes for filter
        let safe_id = chunk_id.replace("'", "''");
        if let Ok(count) = table.count_rows(Some(format!("id = '{}'", safe_id))).await {
            return count > 0;
        }
        false
    }

    async fn get_cached_embedding(&self, hash: &str) -> Option<Vec<f32>> {
        let table = self.table.as_ref()?;
        let query = table.query().only_if(format!("hash = '{}'", hash)).limit(1);
        let mut stream = query.execute().await.ok()?;
        
        if let Some(Ok(batch)) = stream.next().await {
            if batch.num_rows() > 0 {
                let vectors = batch.column_by_name("vector")
                    .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())?;
                let v = vectors.value(0);
                let arr = v.as_any().downcast_ref::<Float32Array>()?;
                return Some(arr.values().to_vec());
            }
        }
        None
    }

    async fn save_chunk_to_db(&self, id: &str, hash: &str, content: &str, source_file: &str, chunk_index: usize, vector: &[f32]) -> Result<(), String> {
        let table = self.table.as_ref().ok_or("Table not initialized")?;
        let schema = self.expected_schema();

        let id_arr = Arc::new(StringArray::from(vec![id.to_string()]));
        let hash_arr = Arc::new(StringArray::from(vec![hash.to_string()]));
        let content_arr = Arc::new(StringArray::from(vec![content.to_string()]));
        let source_arr = Arc::new(StringArray::from(vec![source_file.to_string()]));
        let index_arr = Arc::new(arrow_array::Int64Array::from(vec![chunk_index as i64]));
        
        let vector_values = Float32Array::from(vector.to_vec());
        let vector_arr = Arc::new(FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            vec![Some(vector_values.values().iter().map(|v| Some(*v)).collect::<Vec<_>>())],
            384,
        ));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![id_arr, hash_arr, content_arr, source_arr, index_arr, vector_arr],
        ).map_err(|e| format!("Failed to create record batch: {}", e))?;

        table.add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
            .execute()
            .await
            .map_err(|e| format!("Failed to add record to LanceDB: {}", e))?;

        Ok(())
    }

    async fn collect_files_recursive(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut files = Vec::new();

        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let path = entry.path();

            // Skip hidden files and directories (including .rag-cache)
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
            // Also support files without extension if they look like text
            false
        }
    }

    fn extract_text(&self, file_path: &Path, content: &str) -> Result<String, String> {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "csv" => self.parse_csv(content, ','),
            "tsv" => self.parse_csv(content, '\t'),
            "json" => self.parse_json(content),
            "pdf" => self.extract_pdf_text(file_path),
            "docx" => self.extract_docx_text(file_path),
            _ => Ok(content.to_string()), // txt, md, etc. - use as-is
        }
    }

    /// Extract text from a PDF file
    fn extract_pdf_text(&self, file_path: &Path) -> Result<String, String> {
        pdf_extract::extract_text(file_path).map_err(|e| format!("PDF extraction failed: {}", e))
    }

    /// Extract text from a DOCX file
    fn extract_docx_text(&self, file_path: &Path) -> Result<String, String> {
        use std::io::Read;

        let file =
            std::fs::File::open(file_path).map_err(|e| format!("Failed to open DOCX: {}", e))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("Invalid DOCX archive: {}", e))?;

        let mut doc_xml = archive
            .by_name("word/document.xml")
            .map_err(|_| "No document.xml found in DOCX".to_string())?;

        let mut xml_content = String::new();
        doc_xml
            .read_to_string(&mut xml_content)
            .map_err(|e| format!("Failed to read document.xml: {}", e))?;

        // Extract text from XML (strip tags, decode entities)
        Ok(extract_text_from_docx_xml(&xml_content))
    }

    fn parse_csv(&self, content: &str, delimiter: char) -> Result<String, String> {
        let mut result = String::new();
        let mut lines = content.lines();

        // Get header
        let header: Vec<&str> = if let Some(header_line) = lines.next() {
            header_line.split(delimiter).collect()
        } else {
            return Ok(String::new());
        };

        // Process rows
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
        // For JSON, we just pretty-print or flatten it to text
        // A more sophisticated approach would extract meaningful fields
        match serde_json::from_str::<serde_json::Value>(content) {
            Ok(value) => Ok(self.json_to_text(&value, "")),
            Err(_) => Ok(content.to_string()), // Fall back to raw content
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

    fn chunk_text(&self, text: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let total_len = chars.len();

        if total_len == 0 {
            return chunks;
        }

        let mut start = 0;
        while start < total_len {
            let end = (start + CHUNK_SIZE).min(total_len);

            // Try to find a good break point (end of sentence or paragraph)
            let mut actual_end = end;
            if end < total_len {
                // Look for sentence boundaries within the last 20% of the chunk
                let search_start = start + (CHUNK_SIZE * 80 / 100);
                for i in (search_start..end).rev() {
                    if i < chars.len() {
                        let c = chars[i];
                        if c == '.' || c == '!' || c == '?' || c == '\n' {
                            actual_end = i + 1;
                            break;
                        }
                    }
                }
            }

            let chunk: String = chars[start..actual_end].iter().collect();
            let trimmed = chunk.trim();
            if !trimmed.is_empty() {
                chunks.push(trimmed.to_string());
            }

            // Move start forward, accounting for overlap
            if actual_end >= total_len {
                break;
            }
            start = actual_end.saturating_sub(CHUNK_OVERLAP);
            if start >= actual_end {
                start = actual_end;
            }
        }

        chunks
    }

    fn compute_hash(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn compute_hash_bytes(&self, content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        format!("{:x}", hasher.finalize())
    }

    async fn search_documents(&self, query_vector: Vec<f32>, limit: usize) -> Vec<RagChunk> {
        let search_start = Instant::now();
        let table = match &self.table {
            Some(t) => t,
            None => return Vec::new(),
        };

        // LanceDB includes _distance column with similarity scores (lower = more similar)
        let query = match table.query().nearest_to(query_vector.clone()) {
            Ok(q) => q,
            Err(e) => {
                println!("RagActor ERROR: Failed to create vector query: {}", e);
                return Vec::new();
            }
        };

        let mut results = Vec::new();
        let mut query_stream = match query.limit(limit).execute().await {
            Ok(s) => s,
            Err(e) => {
                println!("RagActor ERROR: Failed to execute search: {}", e);
                return Vec::new();
            }
        };

        while let Some(Ok(batch)) = query_stream.next().await {
            let ids = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let contents = batch
                .column_by_name("content")
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

            if let (Some(ids), Some(contents), Some(source_files), Some(chunk_indices), Some(distances)) =
                (ids, contents, source_files, chunk_indices, distances)
            {
                for i in 0..batch.num_rows() {
                    let distance = distances.value(i);
                    // Convert distance to similarity score (1 / (1 + distance))
                    let score = 1.0 / (1.0 + distance);

                    results.push(RagChunk {
                        id: ids.value(i).to_string(),
                        content: contents.value(i).to_string(),
                        source_file: source_files.value(i).to_string(),
                        chunk_index: chunk_indices.value(i) as usize,
                        score,
                    });
                }
            }
        }

        let total_time = search_start.elapsed();
        println!(
            "RagActor: Search completed in {} ms ({} results)",
            total_time.as_millis(),
            results.len()
        );

        results
    }

    /// Get current Unix timestamp in seconds
    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Extract text content from DOCX XML (word/document.xml)
/// Parses <w:t> tags for text and <w:p> tags for paragraph breaks
fn extract_text_from_docx_xml(xml: &str) -> String {
    let mut result = String::new();
    let mut in_text = false;
    let mut chars = xml.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            // Collect the tag content
            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                tag.push(tc);
            }

            // Check for <w:t> or <w:t ...> (text tag)
            if tag.starts_with("w:t") && !tag.starts_with("w:t/") && !tag.ends_with('/') {
                in_text = true;
            } else if tag == "/w:t" {
                in_text = false;
            } else if tag.starts_with("w:p") && !tag.starts_with("w:p/") && !tag.ends_with('/') {
                // New paragraph - add newline if we have content
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
            }
        } else if in_text {
            result.push(c);
        }
    }

    // Clean up: decode XML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Truncate a string to a maximum length, adding ellipsis if needed
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("{}...", &s[..max_len - 3])
    } else {
        s[..max_len].to_string()
    }
}
