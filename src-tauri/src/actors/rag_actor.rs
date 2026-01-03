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
// PDF STRUCTURE EXTRACTION (Hybrid: Bookmarks + Font Size)
// ============================================================================

/// Extracted heading from PDF with explicit level (from bookmarks or font size)
#[derive(Debug, Clone)]
struct PdfHeading {
    level: u8,      // 1-4
    title: String,
    #[allow(dead_code)]
    page: Option<u32>,
}

/// Extract structure from PDF using hybrid approach:
/// 1. Try bookmarks/outlines first (explicit hierarchy from PDF metadata)
/// 2. Fall back to font-size detection (infer hierarchy from typography)
fn extract_pdf_structure(path: &Path) -> Result<Vec<PdfHeading>, String> {
    // Tier 1: Try bookmarks first (most reliable when available)
    match extract_pdf_bookmarks(path) {
        Ok(headings) if !headings.is_empty() => {
            println!("RagActor: Found {} bookmarks in PDF", headings.len());
            return Ok(headings);
        }
        Ok(_) => {
            println!("RagActor: No bookmarks found, trying font-size detection");
        }
        Err(e) => {
            println!("RagActor: Bookmark extraction failed ({}), trying font-size detection", e);
        }
    }
    
    // Tier 2: Fall back to font-size detection
    match extract_pdf_by_font_size(path) {
        Ok(headings) if !headings.is_empty() => {
            println!("RagActor: Detected {} headings from font sizes", headings.len());
            Ok(headings)
        }
        Ok(_) => {
            println!("RagActor: No font-size headings detected, will use text heuristics");
            Ok(Vec::new())
        }
        Err(e) => {
            println!("RagActor: Font-size detection failed ({}), will use text heuristics", e);
            Ok(Vec::new())
        }
    }
}

/// Extract PDF bookmarks/outlines (Tier 1)
/// Uses lopdf to read the document outline tree
fn extract_pdf_bookmarks(path: &Path) -> Result<Vec<PdfHeading>, String> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| format!("Failed to load PDF: {}", e))?;
    
    let mut headings = Vec::new();
    
    // Recursively traverse bookmark tree
    fn traverse_bookmarks(
        doc: &lopdf::Document,
        bookmark_ids: &[u32],
        level: u8,
        headings: &mut Vec<PdfHeading>,
    ) {
        for &id in bookmark_ids {
            if let Some(bookmark) = doc.bookmark_table.get(&id) {
                // bookmark.page is (page_num, y_offset) tuple - extract page number
                let page_num = Some(bookmark.page.0);
                headings.push(PdfHeading {
                    level: level.min(4), // Cap at H4
                    title: bookmark.title.clone(),
                    page: page_num,
                });
                // Recurse into children
                traverse_bookmarks(doc, &bookmark.children, level + 1, headings);
            }
        }
    }
    
    traverse_bookmarks(&doc, &doc.bookmarks, 1, &mut headings);
    Ok(headings)
}

/// Decode PDF string bytes to a Rust String
/// PDF strings can be UTF-8, UTF-16BE (with BOM 0xFEFF), or PDFDocEncoding
fn decode_pdf_string(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    
    // Check for UTF-16BE BOM (0xFE 0xFF)
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        // UTF-16BE with BOM - skip the BOM and decode
        let utf16_chars: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter_map(|chunk| {
                if chunk.len() == 2 {
                    Some(u16::from_be_bytes([chunk[0], chunk[1]]))
                } else {
                    None
                }
            })
            .collect();
        return String::from_utf16(&utf16_chars).ok();
    }
    
    // Check for UTF-16LE pattern (common: alternating null bytes with ASCII)
    // Pattern: 0xXX 0x00 0xYY 0x00 where XX, YY are ASCII
    if bytes.len() >= 4 {
        let looks_like_utf16le = bytes.chunks(2).take(4).all(|chunk| {
            chunk.len() == 2 && chunk[1] == 0 && chunk[0] < 128
        });
        if looks_like_utf16le {
            let utf16_chars: Vec<u16> = bytes
                .chunks(2)
                .filter_map(|chunk| {
                    if chunk.len() == 2 {
                        Some(u16::from_le_bytes([chunk[0], chunk[1]]))
                    } else {
                        None
                    }
                })
                .collect();
            if let Ok(s) = String::from_utf16(&utf16_chars) {
                // Filter out control characters
                let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }
    
    // Check for UTF-16BE pattern without BOM (alternating 0x00 0xXX)
    if bytes.len() >= 4 {
        let looks_like_utf16be = bytes.chunks(2).take(4).all(|chunk| {
            chunk.len() == 2 && chunk[0] == 0 && chunk[1] < 128
        });
        if looks_like_utf16be {
            let utf16_chars: Vec<u16> = bytes
                .chunks(2)
                .filter_map(|chunk| {
                    if chunk.len() == 2 {
                        Some(u16::from_be_bytes([chunk[0], chunk[1]]))
                    } else {
                        None
                    }
                })
                .collect();
            if let Ok(s) = String::from_utf16(&utf16_chars) {
                let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }
    
    // Try UTF-8 first
    if let Ok(s) = String::from_utf8(bytes.to_vec()) {
        let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    
    // Fall back to Latin-1 / PDFDocEncoding (treat each byte as Unicode codepoint)
    let s: String = bytes.iter()
        .filter_map(|&b| {
            let c = b as char;
            if c.is_control() && c != ' ' { None } else { Some(c) }
        })
        .collect();
    if !s.is_empty() { Some(s) } else { None }
}

/// Extract headings by font size (Tier 2)
/// Parses PDF content streams to find text with different font sizes,
/// then maps the largest fonts to heading levels H1-H4
fn extract_pdf_by_font_size(path: &Path) -> Result<Vec<PdfHeading>, String> {
    use lopdf::{Document, Object};
    
    let doc = Document::load(path)
        .map_err(|e| format!("Failed to load PDF: {}", e))?;
    
    // Collect all text runs with their font sizes
    let mut text_runs: Vec<(f32, String)> = Vec::new();
    
    for (page_num, page_id) in doc.get_pages() {
        if let Ok(content) = doc.get_page_content(page_id) {
            let operations = lopdf::content::Content::decode(&content)
                .map(|c| c.operations)
                .unwrap_or_default();
            
            let mut current_font_size: f32 = 12.0; // Default
            let mut current_text = String::new();
            
            for op in operations {
                match op.operator.as_str() {
                    // Tf: Set text font and size
                    "Tf" => {
                        // Flush previous text if any
                        if !current_text.trim().is_empty() {
                            text_runs.push((current_font_size, current_text.trim().to_string()));
                            current_text.clear();
                        }
                        // Extract font size (second operand)
                        if let Some(Object::Real(size)) = op.operands.get(1) {
                            current_font_size = *size as f32;
                        } else if let Some(Object::Integer(size)) = op.operands.get(1) {
                            current_font_size = *size as f32;
                        }
                    }
                    // Tj: Show text string
                    "Tj" => {
                        if let Some(Object::String(bytes, _)) = op.operands.first() {
                            if let Some(text) = decode_pdf_string(bytes) {
                                current_text.push_str(&text);
                            }
                        }
                    }
                    // TJ: Show text array (with kerning)
                    "TJ" => {
                        if let Some(Object::Array(arr)) = op.operands.first() {
                            for item in arr {
                                if let Object::String(bytes, _) = item {
                                    if let Some(text) = decode_pdf_string(bytes) {
                                        current_text.push_str(&text);
                                    }
                                }
                            }
                        }
                    }
                    // Text positioning operators that indicate new line/block
                    "Td" | "TD" | "T*" | "'" | "\"" => {
                        // Flush current text as a separate run
                        if !current_text.trim().is_empty() {
                            text_runs.push((current_font_size, current_text.trim().to_string()));
                            current_text.clear();
                        }
                    }
                    // BT/ET: Begin/End text object
                    "BT" => {
                        current_text.clear();
                    }
                    "ET" => {
                        if !current_text.trim().is_empty() {
                            text_runs.push((current_font_size, current_text.trim().to_string()));
                            current_text.clear();
                        }
                    }
                    _ => {}
                }
            }
            
            // Flush any remaining text
            if !current_text.trim().is_empty() {
                text_runs.push((current_font_size, current_text.trim().to_string()));
            }
        }
        
        // Limit to first few pages to avoid excessive processing
        if page_num > 10 {
            break;
        }
    }
    
    if text_runs.is_empty() {
        return Ok(Vec::new());
    }
    
    // Determine heading levels based on font size distribution
    // Collect unique font sizes and sort descending
    let mut sizes: Vec<f32> = text_runs.iter().map(|(s, _)| *s).collect();
    sizes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    sizes.dedup_by(|a, b| (*a - *b).abs() < 0.5); // Treat similar sizes as same
    
    // Find the body text size (most common size, typically)
    let mut size_counts: HashMap<i32, usize> = HashMap::new();
    for (size, _) in &text_runs {
        *size_counts.entry((*size * 10.0) as i32).or_insert(0) += 1;
    }
    let body_size = size_counts.iter()
        .max_by_key(|(_, count)| *count)
        .map(|(size, _)| *size as f32 / 10.0)
        .unwrap_or(12.0);
    
    // Only consider sizes larger than body text as headings
    let heading_sizes: Vec<f32> = sizes.into_iter()
        .filter(|&s| s > body_size + 1.0)
        .take(4) // Max 4 heading levels
        .collect();
    
    if heading_sizes.is_empty() {
        return Ok(Vec::new());
    }
    
    // Map font sizes to heading levels
    let size_to_level: HashMap<i32, u8> = heading_sizes.iter()
        .enumerate()
        .map(|(i, &s)| ((s * 10.0) as i32, (i + 1) as u8))
        .collect();
    
    // Convert text runs to headings (only for heading-sized text)
    let headings: Vec<PdfHeading> = text_runs.iter()
        .filter_map(|(size, text)| {
            let size_key = (*size * 10.0) as i32;
            size_to_level.get(&size_key).map(|&level| PdfHeading {
                level,
                title: text.clone(),
                page: None,
            })
        })
        .filter(|h| !h.title.is_empty() && h.title.len() < 200) // Filter out very long text
        .collect();
    
    Ok(headings)
}

/// Normalize text for matching (lowercase, collapse whitespace)
fn normalize_text_for_matching(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

    // ========================================================================
    // DOCUMENT PARSING (Hierarchical)
    // ========================================================================

    /// Parse document into structured elements based on file type
    /// For PDFs, also accepts file_path for hybrid structure extraction
    fn parse_document(&self, extension: &str, content: &str, file_path: Option<&Path>) -> Vec<DocumentElement> {
        match extension {
            "md" => self.parse_markdown(content),
            "docx" => self.parse_docx_elements(content),
            "pdf" => self.parse_pdf_elements(content, file_path),
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

    /// Parse PDF content using hybrid structure extraction
    /// 1. Try extracting bookmarks/outlines (explicit hierarchy from PDF)
    /// 2. Fall back to font-size detection (with validation)
    /// 3. Fall back to text-based heuristics
    fn parse_pdf_elements(&self, content: &str, file_path: Option<&Path>) -> Vec<DocumentElement> {
        // Try hybrid structure extraction if file path is available
        if let Some(path) = file_path {
            if let Ok(headings) = extract_pdf_structure(path) {
                if !headings.is_empty() {
                    // Validate that heading titles actually appear in the content
                    // If font encoding wasn't properly decoded, titles won't match
                    let normalized_content = normalize_text_for_matching(content);
                    let matching_headings: Vec<_> = headings.iter()
                        .filter(|h| {
                            let normalized_title = normalize_text_for_matching(&h.title);
                            // Title should appear somewhere in the content
                            normalized_content.contains(&normalized_title)
                        })
                        .cloned()
                        .collect();
                    
                    // Only use font-size headings if most of them match the content
                    // (indicating proper font encoding decoding)
                    if matching_headings.len() >= headings.len() / 2 && !matching_headings.is_empty() {
                        return self.merge_headings_with_content(&matching_headings, content);
                    }
                }
            }
        }
        
        // Fall back to text-based heuristics
        self.parse_pdf_elements_by_heuristics(content)
    }
    
    /// Merge extracted PDF headings with text content
    /// Matches headings to their positions in the text stream
    fn merge_headings_with_content(
        &self,
        headings: &[PdfHeading],
        content: &str,
    ) -> Vec<DocumentElement> {
        let mut elements = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        
        // Create a map of normalized heading titles to their levels
        let heading_map: HashMap<String, u8> = headings.iter()
            .map(|h| (normalize_text_for_matching(&h.title), h.level))
            .collect();
        
        let mut current_paragraph = String::new();
        
        for line in lines {
            let trimmed = line.trim();
            
            if trimmed.is_empty() {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                continue;
            }
            
            let normalized = normalize_text_for_matching(trimmed);
            
            // Check if this line matches a known heading
            if let Some(&level) = heading_map.get(&normalized) {
                // Flush paragraph before heading
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                elements.push(DocumentElement::Heading { 
                    level, 
                    text: trimmed.to_string() 
                });
            } else {
                // Also check for partial matches (heading might be truncated in bookmark)
                let is_heading = heading_map.iter().any(|(h_title, _)| {
                    normalized.starts_with(h_title) || h_title.starts_with(&normalized)
                });
                
                if is_heading {
                    // Find the level for this partial match
                    let level = heading_map.iter()
                        .find(|(h_title, _)| normalized.starts_with(*h_title) || h_title.starts_with(&normalized))
                        .map(|(_, &l)| l)
                        .unwrap_or(2);
                    
                    if !current_paragraph.is_empty() {
                        elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                        current_paragraph.clear();
                    }
                    elements.push(DocumentElement::Heading { 
                        level, 
                        text: trimmed.to_string() 
                    });
                } else {
                    // Accumulate paragraph
                    if !current_paragraph.is_empty() {
                        current_paragraph.push(' ');
                    }
                    current_paragraph.push_str(trimmed);
                }
            }
        }
        
        // Flush final paragraph
        if !current_paragraph.is_empty() {
            elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
        }
        
        elements
    }
    
    /// Parse PDF content using text-based heuristics (fallback)
    fn parse_pdf_elements_by_heuristics(&self, content: &str) -> Vec<DocumentElement> {
        let mut elements = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut current_paragraph = String::new();
        let mut prev_blank = true; // Start of document counts as preceded by blank
        
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            if trimmed.is_empty() {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                prev_blank = true;
                continue;
            }
            
            // Detect heading level (H1-H4) using multi-level heuristics
            let next_line = lines.get(i + 1).copied();
            if let Some(level) = self.detect_pdf_heading_level(trimmed, prev_blank, next_line) {
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
                    current_paragraph.clear();
                }
                elements.push(DocumentElement::Heading { 
                    level,
                    text: trimmed.to_string() 
                });
                prev_blank = false;
                continue;
            }
            
            // Accumulate paragraph
            if !current_paragraph.is_empty() {
                current_paragraph.push(' ');
            }
            current_paragraph.push_str(trimmed);
            prev_blank = false;
        }
        
        if !current_paragraph.is_empty() {
            elements.push(DocumentElement::Paragraph(current_paragraph.trim().to_string()));
        }
        
        elements
    }

    /// Detect PDF heading level (H1-H4) based on structural heuristics.
    /// 
    /// Since PDF text extraction loses font size/style information, we use:
    /// - ALL CAPS patterns (often used for major headings)
    /// - Line length and word count
    /// - Surrounding blank lines (standalone vs inline)
    /// - Punctuation (headings rarely end with sentence punctuation)
    /// - Title Case patterns
    /// 
    /// Returns None if the line doesn't look like a heading.
    fn detect_pdf_heading_level(
        &self,
        line: &str,
        prev_blank: bool,
        next_line: Option<&str>,
    ) -> Option<u8> {
        let len = line.len();
        let has_alpha = line.chars().any(|c| c.is_alphabetic());
        
        // Must have alphabetic content and reasonable length for a heading
        if !has_alpha || len < 3 || len > 100 {
            return None;
        }
        
        // Structural signals
        let is_all_caps = line.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
        let ends_with_sentence_punct = line.ends_with('.') || line.ends_with('?') || line.ends_with('!');
        let ends_with_colon = line.ends_with(':');
        let next_is_blank = next_line.map_or(true, |n| n.trim().is_empty());
        let words: Vec<&str> = line.split_whitespace().collect();
        let word_count = words.len();
        
        // Headings typically don't end with sentence punctuation
        if ends_with_sentence_punct {
            return None;
        }
        
        // Count Title Case words (first letter uppercase)
        let title_case_count = words.iter().filter(|w| {
            w.chars().next().map_or(false, |c| c.is_uppercase())
        }).count();
        // Require more than half the words to be Title Case
        // This allows lowercase articles/prepositions ("and", "the", "of") in longer titles
        let is_title_case = word_count >= 2 && title_case_count > word_count / 2;
        
        // H1: ALL CAPS, short, standalone (surrounded by blank lines)
        // This pattern is commonly used for major document sections
        if is_all_caps && len < 40 && prev_blank && next_is_blank {
            return Some(1);
        }
        
        // H2: ALL CAPS (not standalone) or short standalone Title Case
        // ALL CAPS sections that aren't surrounded by blanks
        if is_all_caps && len > 3 && len < 60 {
            return Some(2);
        }
        // Short Title Case, standalone (preceded by blank)
        if is_title_case && len < 40 && prev_blank && word_count <= 6 {
            return Some(2);
        }
        
        // H3: Title Case, medium length, or ends with colon (sub-section marker)
        if is_title_case && len < 60 && word_count >= 2 && word_count <= 8 {
            return Some(3);
        }
        if ends_with_colon && len < 50 && word_count <= 6 {
            return Some(3);
        }
        
        // H4: Short lines that look like labels/headers but don't fit above
        // First word capitalized, short, no sentence punctuation
        if len < 50 && word_count >= 2 && word_count <= 8 {
            let first_cap = words.first()
                .and_then(|w| w.chars().next())
                .map_or(false, |c| c.is_uppercase());
            if first_cap && next_is_blank {
                return Some(4);
            }
        }
        
        None
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

    /// Fallback PDF text extraction using lopdf when pdf-extract fails.
    /// Less accurate for complex fonts but more tolerant of malformed PDFs.
    fn extract_pdf_text_with_lopdf(&self, file_path: &Path) -> Result<String, String> {
        use lopdf::{Document, Object};
        
        let doc = Document::load(file_path)
            .map_err(|e| format!("Failed to load PDF: {}", e))?;
        
        let mut all_text = String::new();
        let pages = doc.get_pages();
        
        for (_page_num, page_id) in pages {
            if let Ok(content) = doc.get_page_content(page_id) {
                let operations = lopdf::content::Content::decode(&content)
                    .map(|c| c.operations)
                    .unwrap_or_default();
                
                for op in operations {
                    match op.operator.as_str() {
                        // Tj: Show text string
                        "Tj" => {
                            if let Some(Object::String(bytes, _)) = op.operands.first() {
                                // Try UTF-8 first, then Latin-1 fallback
                                let text = String::from_utf8(bytes.clone())
                                    .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
                                all_text.push_str(&text);
                            }
                        }
                        // TJ: Show text array (with kerning)
                        "TJ" => {
                            if let Some(Object::Array(arr)) = op.operands.first() {
                                for item in arr {
                                    if let Object::String(bytes, _) = item {
                                        let text = String::from_utf8(bytes.clone())
                                            .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
                                        all_text.push_str(&text);
                                    }
                                }
                            }
                        }
                        // Text positioning that indicates new line/paragraph
                        "Td" | "TD" | "T*" | "'" | "\"" => {
                            if !all_text.ends_with('\n') && !all_text.ends_with(' ') {
                                all_text.push(' ');
                            }
                        }
                        "ET" => {
                            // End of text block - add newline
                            if !all_text.ends_with('\n') {
                                all_text.push('\n');
                            }
                        }
                        _ => {}
                    }
                }
            }
            all_text.push('\n'); // Page break
        }
        
        Ok(all_text)
    }
    
    fn extract_pdf_text_with_progress(
        &self,
        file_path: &Path,
        file_index: usize,
        total_files: usize,
    ) -> Result<String, String> {
        // pdf-extract has better font encoding handling than raw lopdf
        // It properly handles ToUnicode CMaps and custom font encodings
        // Use catch_unwind to capture panics from pdf-extract library
        let pages_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pdf_extract::extract_text_by_pages(file_path)
        }));
        
        let pages = match pages_result {
            Ok(Ok(pages)) => pages,
            Ok(Err(e)) => {
                // Try lopdf fallback
                println!("[RAG] pdf-extract failed for {:?}, trying lopdf fallback: {}", file_path.file_name().unwrap_or_default(), e);
                match self.extract_pdf_text_with_lopdf(file_path) {
                    Ok(text) => {
                        println!("[RAG] lopdf fallback succeeded, extracted {} chars", text.len());
                        return Ok(text);
                    }
                    Err(_fallback_err) => {
                        let filename = file_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown");
                        return Err(format!(
                            "Cannot read '{}': This PDF has an incompatible format. Re-attaching it won't help. Try re-exporting the PDF from its source application.",
                            filename
                        ));
                    }
                }
            }
            Err(panic_payload) => {
                let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Unknown panic".to_string()
                };
                
                // Try lopdf fallback after panic
                println!("[RAG] pdf-extract panicked for {:?}, trying lopdf fallback: {}", file_path.file_name().unwrap_or_default(), panic_msg);
                match self.extract_pdf_text_with_lopdf(file_path) {
                    Ok(text) => {
                        println!("[RAG] lopdf fallback succeeded, extracted {} chars", text.len());
                        return Ok(text);
                    }
                    Err(_fallback_err) => {
                        let filename = file_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown");
                        return Err(format!(
                            "Cannot read '{}': This PDF has an incompatible format. Re-attaching it won't help. Try re-exporting the PDF from its source application.",
                            filename
                        ));
                    }
                }
            }
        };

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
    fn test_pdf_heading_level_h1_all_caps_standalone() {
        let actor = create_test_actor();
        
        // H1: ALL CAPS, standalone (surrounded by blank lines)
        assert_eq!(actor.detect_pdf_heading_level("INTRODUCTION", true, Some("")), Some(1));
        assert_eq!(actor.detect_pdf_heading_level("CHAPTER ONE", true, Some("")), Some(1));
        assert_eq!(actor.detect_pdf_heading_level("SUMMARY", true, Some("")), Some(1));
        
        // Not H1 if not standalone (becomes H2)
        assert_eq!(actor.detect_pdf_heading_level("INTRODUCTION", false, Some("")), Some(2));
        assert_eq!(actor.detect_pdf_heading_level("INTRODUCTION", true, Some("content follows")), Some(2));
    }

    #[test]
    fn test_pdf_heading_level_h2_caps_or_title() {
        let actor = create_test_actor();
        
        // H2: ALL CAPS not standalone
        assert_eq!(actor.detect_pdf_heading_level("TECHNICAL OVERVIEW", false, Some("Details")), Some(2));
        assert_eq!(actor.detect_pdf_heading_level("KEY FEATURES", true, Some("Feature list")), Some(2));
        
        // H2: Short Title Case, standalone (preceded by blank)
        assert_eq!(actor.detect_pdf_heading_level("Product Overview", true, Some("")), Some(2));
        assert_eq!(actor.detect_pdf_heading_level("Key Features", true, Some("")), Some(2));
    }

    #[test]
    fn test_pdf_heading_level_h3_title_case() {
        let actor = create_test_actor();
        
        // H3: Title Case, medium length
        assert_eq!(actor.detect_pdf_heading_level("Display Specifications", false, Some("")), Some(3));
        assert_eq!(actor.detect_pdf_heading_level("Power Management Options", false, Some("")), Some(3));
        
        // H3: Lines ending with colon (sub-section markers)
        assert_eq!(actor.detect_pdf_heading_level("Features:", false, Some("")), Some(3));
        assert_eq!(actor.detect_pdf_heading_level("System Requirements:", false, Some("")), Some(3));
    }

    #[test]
    fn test_pdf_heading_level_h4_short_labels() {
        let actor = create_test_actor();
        
        // H4: Short lines that look like labels but don't match H2/H3 patterns
        // These have first word capitalized but aren't full Title Case
        assert_eq!(actor.detect_pdf_heading_level("Memory and storage", false, Some("")), Some(4));
        assert_eq!(actor.detect_pdf_heading_level("Ports available", false, Some("")), Some(4));
        
        // Note: Full Title Case like "Memory Configuration" is detected as H3
        // This is expected since we can't distinguish H3/H4 without font info
        assert_eq!(actor.detect_pdf_heading_level("Memory Configuration", false, Some("")), Some(3));
    }

    #[test]
    fn test_pdf_heading_level_not_heading() {
        let actor = create_test_actor();
        
        // Sentences ending with punctuation are not headings
        assert_eq!(actor.detect_pdf_heading_level("This is a complete sentence.", false, Some("")), None);
        assert_eq!(actor.detect_pdf_heading_level("What is this?", false, Some("")), None);
        assert_eq!(actor.detect_pdf_heading_level("Amazing!", false, Some("")), None);
        
        // Too short
        assert_eq!(actor.detect_pdf_heading_level("Hi", false, Some("")), None);
        
        // Too long
        let long_line = "A".repeat(110);
        assert_eq!(actor.detect_pdf_heading_level(&long_line, false, Some("")), None);
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
        let elements = actor.parse_pdf_elements(content, None);
        
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
        let elements = actor.parse_pdf_elements(content, None);
        let chunks = actor.semantic_chunk(&elements);
        
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
    fn test_normalize_text_for_matching() {
        // Test text normalization
        assert_eq!(normalize_text_for_matching("  Hello   World  "), "hello world");
        assert_eq!(normalize_text_for_matching("CHAPTER ONE"), "chapter one");
        assert_eq!(normalize_text_for_matching("A\t\tB"), "a b");
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
        
        let elements = actor.merge_headings_with_content(&headings, content);
        
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
        
        let elements = actor.parse_pdf_elements(content, None);
        
        // Should still detect headings via heuristics
        let heading_count = elements.iter().filter(|e| matches!(e, DocumentElement::Heading { .. })).count();
        assert!(heading_count >= 1, "Should detect headings via heuristics fallback");
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
