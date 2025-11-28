use crate::protocol::{RagMsg, RagChunk, RagIndexResult};
use tokio::sync::mpsc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
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

/// Statistics from processing a single file
struct FileProcessingStats {
    chunks_added: usize,
    bytes_processed: usize,
    chars_processed: usize,
    embedding_time_ms: u128,
    was_cached: bool,
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
        let indexing_start = Instant::now();
        let mut total_chunks = 0;
        let mut files_processed = 0;
        let mut cache_hits = 0;
        let mut total_bytes: usize = 0;
        let mut total_chars: usize = 0;
        let mut embedding_time_ms: u128 = 0;

        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!("║                    RAG INDEXING STARTED                      ║");
        println!("╚══════════════════════════════════════════════════════════════╝");

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
            match self.process_single_file_with_stats(&file_path, &embedding_model).await {
                Ok(stats) => {
                    total_chunks += stats.chunks_added;
                    total_bytes += stats.bytes_processed;
                    total_chars += stats.chars_processed;
                    embedding_time_ms += stats.embedding_time_ms;
                    files_processed += 1;
                    if stats.was_cached {
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

        let total_time = indexing_start.elapsed();
        let vector_dim = if !self.chunks.is_empty() {
            self.chunks[0].vector.len()
        } else {
            0
        };
        let avg_chunk_chars = if total_chunks > 0 {
            total_chars / total_chunks
        } else {
            0
        };
        let memory_estimate_kb = (self.chunks.len() * (std::mem::size_of::<IndexedChunk>() + vector_dim * 4 + 500)) / 1024;

        println!("\n┌──────────────────────────────────────────────────────────────┐");
        println!("│                  RAG INDEXING SUMMARY                        │");
        println!("├──────────────────────────────────────────────────────────────┤");
        println!("│  Files processed:      {:>8}                              │", files_processed);
        println!("│  Cache hits:           {:>8}                              │", cache_hits);
        println!("│  Total chunks:         {:>8}                              │", total_chunks);
        println!("│  Total bytes:          {:>8} ({:.2} KB)                   │", total_bytes, total_bytes as f64 / 1024.0);
        println!("│  Total chars:          {:>8}                              │", total_chars);
        println!("│  Avg chunk size:       {:>8} chars                        │", avg_chunk_chars);
        println!("│  Vector dimension:     {:>8}                              │", vector_dim);
        println!("├──────────────────────────────────────────────────────────────┤");
        println!("│  Embedding time:       {:>8} ms                           │", embedding_time_ms);
        println!("│  Total time:           {:>8} ms                           │", total_time.as_millis());
        println!("│  Throughput:           {:>8.1} chunks/sec                  │", 
            if total_time.as_secs_f64() > 0.0 { total_chunks as f64 / total_time.as_secs_f64() } else { 0.0 });
        println!("├──────────────────────────────────────────────────────────────┤");
        println!("│  Total chunks in index:{:>8}                              │", self.chunks.len());
        println!("│  Est. memory usage:    {:>8} KB                           │", memory_estimate_kb);
        println!("└──────────────────────────────────────────────────────────────┘\n");

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

    async fn process_single_file_with_stats(
        &mut self,
        file_path: &Path,
        embedding_model: &Arc<TextEmbedding>,
    ) -> Result<FileProcessingStats, String> {
        let path_str = file_path.to_string_lossy().to_string();
        let mut was_cached = false;
        
        // Read file content
        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;
        
        let bytes_processed = content.len();
        
        // Compute content hash
        let content_hash = self.compute_hash(&content);
        
        // Check if already in cache with same hash
        if let Some(entry) = self.manifest.get(&path_str) {
            if entry.file_hash == content_hash {
                // File hasn't changed, but we need to ensure chunks are loaded
                // For now, we'll re-process but mark as cache hit
                println!("RagActor: Cache hit for {:?}", file_path);
                was_cached = true;
                // In a full implementation, we'd load chunks from disk cache here
            }
        }
        
        // Parse content based on file type
        let text_content = self.extract_text(file_path, &content)?;
        let chars_processed = text_content.chars().count();
        
        // Chunk the content
        let text_chunks = self.chunk_text(&text_content);
        let chunk_count = text_chunks.len();
        
        if chunk_count == 0 {
            return Ok(FileProcessingStats {
                chunks_added: 0,
                bytes_processed,
                chars_processed,
                embedding_time_ms: 0,
                was_cached,
            });
        }
        
        println!("RagActor: Chunking {:?} into {} chunks ({} bytes, {} chars)", 
            file_path, chunk_count, bytes_processed, chars_processed);
        
        // Generate embeddings for all chunks
        let model = Arc::clone(embedding_model);
        let chunks_clone = text_chunks.clone();
        
        let embed_start = Instant::now();
        let embeddings = tokio::task::spawn_blocking(move || {
            model.embed(chunks_clone, None)
        })
        .await
        .map_err(|e| format!("Embedding task failed: {}", e))?
        .map_err(|e| format!("Embedding generation failed: {}", e))?;
        let embedding_time_ms = embed_start.elapsed().as_millis();
        
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
        
        Ok(FileProcessingStats {
            chunks_added: chunk_count,
            bytes_processed,
            chars_processed,
            embedding_time_ms,
            was_cached,
        })
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
        let search_start = Instant::now();
        
        if self.chunks.is_empty() {
            println!("\n┌─────────────────────────────────────────────────────────────┐");
            println!("│                   RAG SEARCH (empty index)                  │");
            println!("└─────────────────────────────────────────────────────────────┘\n");
            return Vec::new();
        }
        
        // Compute cosine similarity with all chunks
        let similarity_start = Instant::now();
        let mut scored: Vec<(f32, &IndexedChunk)> = self.chunks
            .iter()
            .map(|chunk| {
                let score = cosine_similarity(&query_vector, &chunk.vector);
                (score, chunk)
            })
            .collect();
        let similarity_time = similarity_start.elapsed();
        
        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        
        // Calculate score statistics
        let all_scores: Vec<f32> = scored.iter().map(|(s, _)| *s).collect();
        let min_score = all_scores.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_score = all_scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let avg_score: f32 = all_scores.iter().sum::<f32>() / all_scores.len() as f32;
        
        // Take top results
        let results: Vec<RagChunk> = scored
            .into_iter()
            .take(limit)
            .map(|(score, chunk)| RagChunk {
                id: chunk.id.clone(),
                content: chunk.content.clone(),
                source_file: chunk.source_file.clone(),
                chunk_index: chunk.chunk_index,
                score,
            })
            .collect();
        
        let total_time = search_start.elapsed();
        
        // Calculate top-K statistics
        let top_scores: Vec<f32> = results.iter().map(|r| r.score).collect();
        let top_min = top_scores.iter().cloned().fold(f32::INFINITY, f32::min);
        let top_max = top_scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let top_avg: f32 = if !top_scores.is_empty() {
            top_scores.iter().sum::<f32>() / top_scores.len() as f32
        } else {
            0.0
        };
        
        // Collect unique source files in results
        let unique_sources: std::collections::HashSet<&str> = results
            .iter()
            .map(|r| r.source_file.as_str())
            .collect();
        
        println!("\n┌─────────────────────────────────────────────────────────────┐");
        println!("│                      RAG SEARCH RESULTS                     │");
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│  Query vector dim:     {:>8}                             │", query_vector.len());
        println!("│  Chunks searched:      {:>8}                             │", self.chunks.len());
        println!("│  Results returned:     {:>8}                             │", results.len());
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│  All Scores (cosine similarity):                           │");
        println!("│    Min:                {:>8.4}                             │", min_score);
        println!("│    Max:                {:>8.4}                             │", max_score);
        println!("│    Avg:                {:>8.4}                             │", avg_score);
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│  Top-{} Scores:                                             │", limit);
        println!("│    Min:                {:>8.4}                             │", top_min);
        println!("│    Max:                {:>8.4}                             │", top_max);
        println!("│    Avg:                {:>8.4}                             │", top_avg);
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│  Source files in results: {}                               │", unique_sources.len());
        for source in unique_sources.iter().take(5) {
            println!("│    - {:<52} │", truncate_str(source, 52));
        }
        if unique_sources.len() > 5 {
            println!("│    ... and {} more                                         │", unique_sources.len() - 5);
        }
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│  Similarity calc:      {:>8.2} ms                         │", similarity_time.as_secs_f64() * 1000.0);
        println!("│  Total search time:    {:>8.2} ms                         │", total_time.as_secs_f64() * 1000.0);
        println!("└─────────────────────────────────────────────────────────────┘\n");
        
        // Log individual top results
        if !results.is_empty() {
            println!("Top {} results:", results.len().min(5));
            for (i, result) in results.iter().take(5).enumerate() {
                let preview = truncate_str(&result.content.replace('\n', " "), 60);
                println!("  {}. [{}] score={:.4}: \"{}\"", 
                    i + 1, result.source_file, result.score, preview);
            }
            println!();
        }
        
        results
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

