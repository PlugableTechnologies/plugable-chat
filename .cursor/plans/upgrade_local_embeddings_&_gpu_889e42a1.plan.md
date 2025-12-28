---
name: Upgrade to BGE-Base + GPU
overview: Upgrade from all-MiniLM-L6-v2 (384 dims) to BGE-Base-EN-v1.5 (768 dims) with GPU acceleration. Includes automatic schema migration.
todos:
  - id: update-foundry-actor
    content: Update foundry_actor.rs - new model, GPU providers, EMBEDDING_DIM=768
    status: completed
  - id: update-vector-actor
    content: Update vector_actor.rs - change 384→768, add dimension-aware migration
    status: completed
  - id: update-rag-actor
    content: Update rag_actor.rs - change 384→768, add dimension-aware migration
    status: completed
  - id: update-schema-vector-actor
    content: Update schema_vector_actor.rs - change 384→768, add dimension-aware migration
    status: completed
---

# Upgrade to BGE-Base-EN-v1.5 + GPU Acceleration

Upgrade the local embedding model from `all-MiniLM-L6-v2` (384 dims) to `BGE-Base-EN-v1.5` (768 dims) and enable explicit GPU acceleration. Migration is fully automatic.

## Why BGE-Base-EN-v1.5?

- Significantly higher MTEB retrieval scores than MiniLM
- 768-dimension embeddings capture more semantic nuance
- Directly supported by fastembed-rs as `EmbeddingModel::BGEBaseENV15`

## Automatic Migration Strategy

The current schema migration only checks **field count**, which won't detect a dimension change (384→768) since the number of fields stays the same. We need to enhance the migration logic to:

1. Extract the vector field's `FixedSizeList` dimension from the existing schema
2. Compare it against the expected dimension (768)
3. If mismatched, drop and recreate the table automatically
```rust
// Helper to extract vector dimension from schema
fn get_vector_dimension(schema: &Schema, field_name: &str) -> Option<i32> {
    schema.field_with_name(field_name).ok().and_then(|f| {
        if let DataType::FixedSizeList(_, dim) = f.data_type() {
            Some(*dim)
        } else {
            None
        }
    })
}

// Enhanced migration check
let existing_dim = get_vector_dimension(&existing_schema, "vector");
let expected_dim = get_vector_dimension(&expected_schema, "vector");
if existing_dim != expected_dim {
    println!("Embedding dimension changed ({:?} -> {:?}), recreating table...", existing_dim, expected_dim);
    // Drop and recreate
}
```


## Files to Modify

### 1. [src-tauri/src/actors/foundry_actor.rs](src-tauri/src/actors/foundry_actor.rs)

**Changes:**

1. Update import to include `ExecutionProviderDispatch` and GPU provider types
2. Change `EMBEDDING_DIM` from `384` to `768` (line ~22)
3. Change `EmbeddingModel::AllMiniLML6V2` to `EmbeddingModel::BGEBaseENV15` (line ~490)
4. Build and pass `execution_providers` from detected `valid_eps`

**Key code change:**

```rust
// Line ~10: Updated imports
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding, ExecutionProviderDispatch};
use ort::{CUDAExecutionProvider, CoreMLExecutionProvider, DirectMLExecutionProvider};

// Line ~22: Updated constant
const EMBEDDING_DIM: usize = 768;

// Lines ~487-495: Updated initialization with GPU support
let valid_eps_clone = self.valid_eps.clone();
match tokio::task::spawn_blocking(move || {
    let mut options = InitOptions::new(EmbeddingModel::BGEBaseENV15);
    options.show_download_progress = true;
    
    // Map detected Foundry EPs to fastembed ExecutionProviderDispatch
    let mut eps: Vec<ExecutionProviderDispatch> = Vec::new();
    for ep_str in &valid_eps_clone {
        match ep_str.as_str() {
            s if s.contains("CUDA") => eps.push(CUDAExecutionProvider::default().into()),
            s if s.contains("CoreML") => eps.push(CoreMLExecutionProvider::default().into()),
            s if s.contains("DirectML") => eps.push(DirectMLExecutionProvider::default().into()),
            _ => {}
        }
    }
    if !eps.is_empty() {
        options.execution_providers = eps;
    }
    
    TextEmbedding::try_new(options)
})
```

### 2. [src-tauri/src/actors/vector_actor.rs](src-tauri/src/actors/vector_actor.rs)

**Changes:**

- Line ~54: `vec![0.0; 384]` → `vec![0.0; 768]`
- Line ~262: `FixedSizeList(..., 384)` → `FixedSizeList(..., 768)`
- Line ~378: `384` → `768`
- Lines ~278-303: Enhance schema check to compare vector dimensions, not just field count

**Enhanced migration logic:**

```rust
// In setup_table(), replace field count check with dimension-aware check
match table.schema().await {
    Ok(existing_schema) => {
        // Check vector field dimension specifically
        let existing_dim = existing_schema
            .field_with_name("embedding_vector")
            .ok()
            .and_then(|f| match f.data_type() {
                DataType::FixedSizeList(_, dim) => Some(*dim),
                _ => None,
            });
        
        let expected_dim = Some(768i32);
        
        if existing_dim != expected_dim {
            println!(
                "VectorActor: Embedding dimension changed ({:?} -> {:?}). Recreating table...",
                existing_dim, expected_dim
            );
            // Drop and recreate table
            let _ = db_connection.drop_table("chats").await;
            // ... create new table with expected_schema
        }
    }
    // ...
}
```

### 3. [src-tauri/src/actors/rag_actor.rs](src-tauri/src/actors/rag_actor.rs)

**Changes:**

- Line ~246: `FixedSizeList(..., 384)` → `FixedSizeList(..., 768)`
- Line ~778: `384` → `768`
- Enhance `ensure_table_exists()` to check vector dimension, not just field count

### 4. [src-tauri/src/actors/schema_vector_actor.rs](src-tauri/src/actors/schema_vector_actor.rs)

**Changes:**

- Line ~25: `pub const SCHEMA_EMBEDDING_DIM: i32 = 384;` → `768`
- Enhance `ensure_tables_table_schema()` and `ensure_columns_table_schema()` to check vector dimension

## Impact

| Aspect | Before | After |

| :--- | :--- | :--- |

| Model | all-MiniLM-L6-v2 | BGE-Base-EN-v1.5 |

| Dimensions | 384 | 768 |

| Model Size | ~90 MB | ~438 MB |

| GPU | Not configured | CUDA/CoreML/DirectML |

| Quality | Baseline | ~15% better on MTEB retrieval |

| Migration | Manual | Automatic (on startup) |

## User Experience

On first launch after upgrade:

1. App detects dimension mismatch in existing tables
2. Tables are automatically dropped and recreated with 768-dim schema
3. User sees log messages like: `"Embedding dimension changed (384 -> 768). Recreating table..."`
4. RAG/chat history is cleared (embeddings incompatible anyway)
5. New embeddings generated with BGE-Base model on GPU