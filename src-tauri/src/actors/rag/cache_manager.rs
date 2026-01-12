//! Cache management for RAG document indexing.
//!
//! This module handles:
//! - LanceDB sidecar database connections
//! - File cache entries and CRC-based reindexing
//! - Indexed chunk storage with embeddings
//! - Schema definitions for RAG tables

use arrow_array::{RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures::StreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, Table};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The name of the table in LanceDB for RAG chunks
pub const RAG_CHUNKS_TABLE: &str = "rag_chunks";

/// The name of the table in LanceDB for file cache
pub const RAG_FILE_CACHE_TABLE: &str = "rag_file_cache";

/// LRU cache capacity for embeddings (~150MB for 384-dim vectors at 10k entries)
pub const EMBEDDING_LRU_CAPACITY: usize = 10_000;

/// Represents a cached file entry
#[derive(Clone)]
pub struct FileCacheEntry {
    pub file_path: String,
    pub crc32: u32,
    pub chunk_count: usize,
    pub indexed_at: i64,
}

/// A document chunk with its embedding
#[derive(Clone)]
pub struct IndexedChunk {
    pub id: String,
    pub hash: String,
    pub file_crc32: u32,
    pub content: String,
    pub heading_context: String,
    pub source_file: String,
    pub chunk_index: usize,
    pub vector: Vec<f32>,
}

/// Represents a connection to a specific directory's sidecar database
pub struct DirectoryConnection {
    /// LanceDB connection (kept alive for table handles)
    #[allow(dead_code)]
    pub db: Connection,
    /// Table handle for RAG chunks
    pub chunks_table: Table,
    /// Table handle for file cache
    pub file_cache_table: Table,
    /// The root path this connection serves
    #[allow(dead_code)]
    pub root_path: PathBuf,
}

/// Get the sidecar cache path for a file
pub fn get_rag_sidecar_cache_path(file_path: &Path) -> PathBuf {
    crate::paths::get_rag_sidecar_cache_dir(file_path)
}

/// Get the schema for RAG chunks table
pub fn get_rag_chunks_schema() -> Arc<Schema> {
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

/// Get the schema for file cache table
pub fn get_rag_file_cache_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("file_path", DataType::Utf8, false),
        Field::new("crc32", DataType::UInt32, false),
        Field::new("chunk_count", DataType::Int64, false),
        Field::new("indexed_at", DataType::Int64, false),
    ]))
}

/// Check if file needs reindexing based on CRC
pub fn should_reindex_file_by_crc(current_crc: u32, cached: Option<&FileCacheEntry>) -> bool {
    match cached {
        Some(entry) if entry.crc32 == current_crc => false,
        _ => true,
    }
}

/// Compute SHA-256 hash of content
pub fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Hash a directory path to create a unique, safe directory name
#[allow(dead_code)]
pub fn compute_path_hash(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

/// Ensure a LanceDB connection exists for a file path
pub async fn ensure_lancedb_connection_for_path(
    connections: &mut HashMap<PathBuf, DirectoryConnection>,
    file_path: &Path,
) -> Result<PathBuf, String> {
    // Use centralized fallback chain from paths module
    let writable = crate::paths::ensure_rag_cache_dir(file_path).await;
    let cache_dir = writable.path.clone();

    if !connections.contains_key(&cache_dir) {
        if writable.is_fallback {
            if let Some(reason) = &writable.fallback_reason {
                println!("RagActor: {}", reason);
            }
        }

        let db_path_str = cache_dir.to_string_lossy().to_string();
        let db = connect(&db_path_str)
            .execute()
            .await
            .map_err(|e| format!("Failed to connect to LanceDB at {}: {}", db_path_str, e))?;

        // Initialize chunks table
        let chunks_schema = get_rag_chunks_schema();
        let chunks_table = ensure_lancedb_table_exists(&db, RAG_CHUNKS_TABLE, chunks_schema.clone()).await?;

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
        let file_cache_schema = get_rag_file_cache_schema();
        let file_cache_table =
            ensure_lancedb_table_exists(&db, RAG_FILE_CACHE_TABLE, file_cache_schema.clone()).await?;

        // Create index for file cache
        let _ = file_cache_table
            .create_index(&["file_path"], Index::Auto)
            .execute()
            .await;

        connections.insert(
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

    Ok(cache_dir)
}

/// Ensure a table exists in the LanceDB connection with correct schema
pub async fn ensure_lancedb_table_exists(
    db: &Connection,
    table_name: &str,
    schema: Arc<Schema>,
) -> Result<Table, String> {
    let table_names = db.table_names().execute().await.map_err(|e| e.to_string())?;

    if table_names.contains(&table_name.to_string()) {
        let table = db
            .open_table(table_name)
            .execute()
            .await
            .map_err(|e| e.to_string())?;

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
                table_name, existing_dim, expected_dim, existing_field_count, expected_field_count
            );
            let _ = db.drop_table(table_name, &[]).await;
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

/// Load file cache entry from table
pub async fn load_file_cache_entries_from_table(
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

/// Save file cache entry to table
pub async fn save_file_cache_entries_to_table(
    table: &Table,
    entry: &FileCacheEntry,
) -> Result<(), String> {
    // Delete existing entry if any
    let escaped = entry.file_path.replace("'", "''");
    let _ = table.delete(&format!("file_path = '{}'", escaped)).await;

    // Insert new entry
    let schema = get_rag_file_cache_schema();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_content_hash() {
        let hash1 = compute_content_hash("hello world");
        let hash2 = compute_content_hash("hello world");
        assert_eq!(hash1, hash2);

        let hash3 = compute_content_hash("different content");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_compute_path_hash() {
        let hash1 = compute_path_hash(Path::new("/some/path"));
        let hash2 = compute_path_hash(Path::new("/some/path"));
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 16);
    }

    #[test]
    fn test_should_reindex_file_by_crc() {
        let cached = FileCacheEntry {
            file_path: "test.txt".to_string(),
            crc32: 12345,
            chunk_count: 10,
            indexed_at: 0,
        };

        assert!(!should_reindex_file_by_crc(12345, Some(&cached)));
        assert!(should_reindex_file_by_crc(54321, Some(&cached)));
        assert!(should_reindex_file_by_crc(12345, None));
    }
}
