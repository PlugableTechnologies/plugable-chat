use crate::protocol::{RagMsg, RagChunk, RagIndexResult};
use tokio::sync::mpsc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use sha2::{Sha256, Digest};
use fastembed::TextEmbedding;

/// Chunk size in characters
const CHUNK_SIZE: usize = 500;

/// Overlap between chunks in characters
const CHUNK_OVERLAP: usize = 100;

/// A document chunk with its embedding
#[derive(Clone)]
struct IndexedChunk {
    id: String,
    content: String,
    source_file: String,
    chunk_index: usize,
    vector: Vec<f32>,
}

/// Manifest entry for a cached file
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ManifestEntry {
    file_hash: String,
    chunk_count: usize,
}

/// The RAG Actor handles document processing and retrieval
pub struct RagActor {
    rx: mpsc::Receiver<RagMsg>,
    /// In-memory index of all chunks (for simplicity, we keep it in memory)
    /// In production, this would be persisted to LanceDB
    chunks: Vec<IndexedChunk>,
    /// Cache directory path (set when processing documents)
    cache_dir: Option<PathBuf>,
    /// Manifest of processed files: file_path -> (hash, chunk_ids)
    manifest: HashMap<String, ManifestEntry>,
}

impl RagActor {
    pub fn new(rx: mpsc::Receiver<RagMsg>) -> Self {
        Self {
            rx,
            chunks: Vec::new(),
            cache_dir: None,
            manifest: HashMap::new(),
        }
    }

    pub async fn run(mut self) {
        println!("RagActor: Starting...");
        
        while let Some(msg) = self.rx.recv().await {
            match msg {
                RagMsg::ProcessDocuments { paths, embedding_model, respond_to } => {
                    println!("RagActor: Processing {} paths", paths.len());
                    let result = self.process_documents(paths, embedding_model).await;
                    let _ = respond_to.send(result);
                }
                RagMsg::SearchDocuments { query_vector, limit, respond_to } => {
                    println!("RagActor: Searching with limit {}", limit);
                    let results = self.search_documents(query_vector, limit);
                    let _ = respond_to.send(results);
                }
                RagMsg::ClearContext { respond_to } => {
                    println!("RagActor: Clearing context");
                    self.chunks.clear();
                    self.manifest.clear();
                    self.cache_dir = None;
                    let _ = respond_to.send(true);
                }
            }
        }
        
        println!("RagActor: Shutting down");
    }

    async fn process_documents(
        &mut self,
        paths: Vec<String>,
        embedding_model: Arc<TextEmbedding>,
    ) -> Result<RagIndexResult, String> {
        let mut total_chunks = 0;
        let mut files_processed = 0;
        let mut cache_hits = 0;

        // Determine cache directory from the first path
        if let Some(first_path) = paths.first() {
            let path = Path::new(first_path);
            let parent = if path.is_dir() {
                path.to_path_buf()
            } else {
                path.parent().unwrap_or(Path::new(".")).to_path_buf()
            };
            self.cache_dir = Some(parent.join(".rag-cache"));
            
            // Create cache directory if it doesn't exist
            if let Some(ref cache_dir) = self.cache_dir {
                if let Err(e) = tokio::fs::create_dir_all(cache_dir).await {
                    println!("RagActor: Warning - failed to create cache dir: {}", e);
                }
            }
        }

        // Load existing manifest if present
        self.load_manifest().await;

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

        println!("RagActor: Found {} files to process", files_to_process.len());

        // Process each file
        for file_path in files_to_process {
            match self.process_single_file(&file_path, &embedding_model).await {
                Ok((chunks_added, was_cached)) => {
                    total_chunks += chunks_added;
                    files_processed += 1;
                    if was_cached {
                        cache_hits += 1;
                    }
                }
                Err(e) => {
                    println!("RagActor: Error processing {:?}: {}", file_path, e);
                }
            }
        }

        // Save manifest
        self.save_manifest().await;

        println!(
            "RagActor: Processed {} files, {} total chunks, {} cache hits",
            files_processed, total_chunks, cache_hits
        );

        Ok(RagIndexResult {
            total_chunks,
            files_processed,
            cache_hits,
        })
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
            matches!(ext.to_lowercase().as_str(), "txt" | "csv" | "tsv" | "md" | "json")
        } else {
            // Also support files without extension if they look like text
            false
        }
    }

    async fn process_single_file(
        &mut self,
        file_path: &Path,
        embedding_model: &Arc<TextEmbedding>,
    ) -> Result<(usize, bool), String> {
        let path_str = file_path.to_string_lossy().to_string();
        
        // Read file content
        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;
        
        // Compute content hash
        let content_hash = self.compute_hash(&content);
        
        // Check if already in cache with same hash
        if let Some(entry) = self.manifest.get(&path_str) {
            if entry.file_hash == content_hash {
                // File hasn't changed, but we need to ensure chunks are loaded
                // For now, we'll re-process but mark as cache hit
                println!("RagActor: Cache hit for {:?}", file_path);
                // In a full implementation, we'd load chunks from disk cache here
            }
        }
        
        // Parse content based on file type
        let text_content = self.extract_text(file_path, &content)?;
        
        // Chunk the content
        let text_chunks = self.chunk_text(&text_content);
        let chunk_count = text_chunks.len();
        
        if chunk_count == 0 {
            return Ok((0, false));
        }
        
        println!("RagActor: Chunking {:?} into {} chunks", file_path, chunk_count);
        
        // Generate embeddings for all chunks
        let model = Arc::clone(embedding_model);
        let chunks_clone = text_chunks.clone();
        
        let embeddings = tokio::task::spawn_blocking(move || {
            model.embed(chunks_clone, None)
        })
        .await
        .map_err(|e| format!("Embedding task failed: {}", e))?
        .map_err(|e| format!("Embedding generation failed: {}", e))?;
        
        // Store indexed chunks
        let file_name = file_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        
        for (idx, (chunk_text, embedding)) in text_chunks.into_iter().zip(embeddings.into_iter()).enumerate() {
            let chunk = IndexedChunk {
                id: format!("{}:{}", path_str, idx),
                content: chunk_text,
                source_file: file_name.clone(),
                chunk_index: idx,
                vector: embedding,
            };
            self.chunks.push(chunk);
        }
        
        // Update manifest
        self.manifest.insert(path_str, ManifestEntry {
            file_hash: content_hash,
            chunk_count,
        });
        
        Ok((chunk_count, false))
    }

    fn extract_text(&self, file_path: &Path, content: &str) -> Result<String, String> {
        let ext = file_path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        
        match ext.as_str() {
            "csv" => self.parse_csv(content, ','),
            "tsv" => self.parse_csv(content, '\t'),
            "json" => self.parse_json(content),
            _ => Ok(content.to_string()), // txt, md, etc. - use as-is
        }
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

    fn search_documents(&self, query_vector: Vec<f32>, limit: usize) -> Vec<RagChunk> {
        if self.chunks.is_empty() {
            return Vec::new();
        }
        
        // Compute cosine similarity with all chunks
        let mut scored: Vec<(f32, &IndexedChunk)> = self.chunks
            .iter()
            .map(|chunk| {
                let score = cosine_similarity(&query_vector, &chunk.vector);
                (score, chunk)
            })
            .collect();
        
        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        
        // Take top results
        scored
            .into_iter()
            .take(limit)
            .map(|(score, chunk)| RagChunk {
                id: chunk.id.clone(),
                content: chunk.content.clone(),
                source_file: chunk.source_file.clone(),
                chunk_index: chunk.chunk_index,
                score,
            })
            .collect()
    }

    async fn load_manifest(&mut self) {
        if let Some(ref cache_dir) = self.cache_dir {
            let manifest_path = cache_dir.join("manifest.json");
            if let Ok(content) = tokio::fs::read_to_string(&manifest_path).await {
                if let Ok(manifest) = serde_json::from_str(&content) {
                    self.manifest = manifest;
                    println!("RagActor: Loaded manifest with {} entries", self.manifest.len());
                }
            }
        }
    }

    async fn save_manifest(&self) {
        if let Some(ref cache_dir) = self.cache_dir {
            let manifest_path = cache_dir.join("manifest.json");
            if let Ok(content) = serde_json::to_string_pretty(&self.manifest) {
                if let Err(e) = tokio::fs::write(&manifest_path, content).await {
                    println!("RagActor: Warning - failed to save manifest: {}", e);
                }
            }
        }
    }
}

/// Compute cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    
    dot / (norm_a * norm_b)
}

