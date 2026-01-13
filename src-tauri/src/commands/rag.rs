//! RAG (Retrieval-Augmented Generation) Tauri commands.
//!
//! Commands for managing document indexing and context retrieval
//! for RAG-based chat augmentation.

use crate::app_state::{ActorHandles, EmbeddingModelState};
use crate::protocol::{FoundryMsg, RagChunk, RagIndexResult, RagMsg, RemoveFileResult};
use tauri::State;
use tokio::sync::oneshot;

// NOTE: GPU EMBEDDING DISABLED - FoundryMsg::GetGpuEmbeddingModel and LLM unload/rewarm
// are no longer used in this file. The imports remain for search_rag_context which still
// uses FoundryMsg::GetEmbedding for CPU embeddings.

/// Select files for RAG indexing (placeholder - frontend uses dialog plugin)
#[tauri::command]
pub async fn select_files() -> Result<Vec<String>, String> {
    // Note: File selection is handled directly by the frontend using the dialog plugin
    // This command is kept for potential future use
    Ok(Vec::new())
}

/// Select a folder for RAG indexing (placeholder - frontend uses dialog plugin)
#[tauri::command]
pub async fn select_folder() -> Result<Option<String>, String> {
    // Similar to select_files - frontend will use dialog plugin directly
    Ok(None)
}

/// Process documents and add them to the RAG index
///
/// NOTE: GPU EMBEDDING DISABLED - Always uses CPU embedding model.
/// This simplifies the code and avoids GPU memory contention issues.
/// To re-enable GPU embedding, see the commented code in foundry_actor.rs.
#[tauri::command]
pub async fn process_rag_documents(
    paths: Vec<String>,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<RagIndexResult, String> {
    println!("[RAG] Processing {} paths (CPU embedding)", paths.len());

    // Always use CPU embedding model (GPU embedding is disabled)
    let model_guard = embedding_state.cpu_model.read().await;
    let embedding_model = model_guard
        .clone()
        .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
    drop(model_guard);

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::IndexRagDocuments {
            paths,
            embedding_model,
            use_gpu: false, // Always CPU
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let result = rx.await.map_err(|_| "RAG actor died".to_string())?;

    if result.is_ok() {
        println!("[RAG] Indexing complete (CPU)");
    }

    result
}

/// Search the RAG index for relevant context
#[tauri::command]
pub async fn search_rag_context(
    query: String,
    limit: usize,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<RagChunk>, String> {
    println!(
        "[RAG] Searching for context with query length: {}",
        query.len()
    );

    // First, get embedding for the query (use CPU to avoid evicting LLM from GPU)
    let (emb_tx, emb_rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetEmbedding {
            text: query,
            use_gpu: false, // CPU model for search during chat
            respond_to: emb_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let query_vector = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // Then search the RAG index
    let (search_tx, search_rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::SearchRagChunksByEmbedding {
            query_vector,
            limit,
            respond_to: search_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    search_rx.await.map_err(|_| "RAG actor died".to_string())
}

/// Clear all documents from the RAG index
#[tauri::command]
pub async fn clear_rag_context(handles: State<'_, ActorHandles>) -> Result<bool, String> {
    println!("[RAG] Clearing context");

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::ClearContext { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "RAG actor died".to_string())
}

/// Remove a specific file from the RAG index
#[tauri::command]
pub async fn remove_rag_file(
    handles: State<'_, ActorHandles>,
    source_file: String,
) -> Result<RemoveFileResult, String> {
    println!("[RAG] Removing file from index: {}", source_file);

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::RemoveFile {
            source_file,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "RAG actor died".to_string())
}

/// Get list of files currently indexed for RAG
#[tauri::command]
pub async fn get_rag_indexed_files(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    println!("[RAG] Getting indexed files");

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::GetIndexedFiles { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;

    rx.await.map_err(|_| "RAG actor died".to_string())
}

/// Get the default test-data directory path for file dialogs
#[tauri::command]
pub fn get_test_data_directory() -> Option<String> {
    // Try to find test-data directory relative to executable or current directory
    let candidates = [
        // Development: relative to current working directory
        std::path::PathBuf::from("test-data"),
        // macOS app bundle: relative to Resources
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../Resources/test-data")))
            .unwrap_or_default(),
        // Fallback: user's Downloads directory
        dirs::download_dir().unwrap_or_default(),
    ];

    for candidate in &candidates {
        if candidate.exists() && candidate.is_dir() {
            if let Some(path_str) = candidate.canonicalize().ok().and_then(|p| p.to_str().map(String::from)) {
                println!("[RAG] Test data directory: {}", path_str);
                return Some(path_str);
            }
        }
    }

    // If nothing found, return Downloads as fallback
    dirs::download_dir().and_then(|p| p.to_str().map(String::from))
}

/// Parse headers and row count from a tabular file (CSV, TSV, XLS, XLSX).
/// 
/// Returns a lightweight preview for the UI without loading all data into memory.
#[tauri::command]
pub fn parse_tabular_headers(file_path: String) -> Result<crate::tabular_parser::TabularHeaderPreview, String> {
    let path = std::path::Path::new(&file_path);
    crate::tabular_parser::parse_tabular_headers(path)
}
