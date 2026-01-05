//! RAG (Retrieval-Augmented Generation) Tauri commands.
//!
//! Commands for managing document indexing and context retrieval
//! for RAG-based chat augmentation.

use crate::app_state::{ActorHandles, EmbeddingModelState};
use crate::protocol::{FoundryMsg, RagChunk, RagIndexResult, RagMsg, RemoveFileResult};
use tauri::State;
use tokio::sync::oneshot;

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
/// Tries to use GPU embedding model for speed, but falls back to CPU if GPU is busy.
/// When using GPU, explicitly unloads the LLM first to free GPU/Metal memory and avoid
/// context contention between Foundry Local's LLM and fastembed's ONNX Runtime + CoreML.
#[tauri::command]
pub async fn process_rag_documents(
    paths: Vec<String>,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<RagIndexResult, String> {
    println!("[RAG] Processing {} paths", paths.len());

    // Try to get GPU embedding model, but fall back to CPU if GPU is busy
    // This prevents GPU memory contention with LLM prewarm/chat operations
    let (embedding_model, use_gpu) = match handles.gpu_guard.mutex.try_lock() {
        Ok(_guard) => {
            // GPU is available - first unload the LLM to free GPU memory
            drop(_guard);
            
            // Unload LLM to free GPU/Metal memory before loading embedding model
            // This prevents Metal context contention between the two model frameworks
            let (unload_tx, unload_rx) = oneshot::channel();
            handles
                .foundry_tx
                .send(FoundryMsg::UnloadCurrentLlm {
                    respond_to: unload_tx,
                })
                .await
                .map_err(|e| format!("Failed to request LLM unload: {}", e))?;
            
            // Wait for unload to complete (best-effort, don't fail on error)
            match unload_rx.await {
                Ok(Ok(Some(model_name))) => {
                    println!("[RAG] LLM '{}' unloaded, GPU memory freed for embedding", model_name);
                }
                Ok(Ok(None)) => {
                    println!("[RAG] No LLM was loaded, proceeding with GPU embedding");
                }
                Ok(Err(e)) => {
                    println!("[RAG] WARNING: Failed to unload LLM: {}. Continuing anyway.", e);
                }
                Err(_) => {
                    println!("[RAG] WARNING: LLM unload channel closed. Continuing anyway.");
                }
            }
            
            // Now request the GPU embedding model
            let (model_tx, model_rx) = oneshot::channel();
            handles
                .foundry_tx
                .send(FoundryMsg::GetGpuEmbeddingModel {
                    respond_to: model_tx,
                })
                .await
                .map_err(|e| format!("Failed to request GPU embedding model: {}", e))?;

            match model_rx.await {
                Ok(Ok(model)) => (model, true),
                Ok(Err(gpu_error)) => {
                    // GPU embedding failed - fall back to CPU with warning
                    println!("[RAG] WARNING: GPU embedding failed: {}. Falling back to CPU.", gpu_error);
                    
                    let model_guard = embedding_state.cpu_model.read().await;
                    let model = model_guard
                        .clone()
                        .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
                    drop(model_guard);
                    
                    (model, false)
                }
                Err(_) => {
                    return Err("Foundry actor died while getting GPU embedding model".to_string());
                }
            }
        }
        Err(_) => {
            // GPU is busy - fall back to CPU
            let current_op = handles.gpu_guard.current_operation.read().await;
            let op_desc = current_op.clone().unwrap_or_else(|| "unknown operation".to_string());
            drop(current_op);
            
            println!("[RAG] GPU busy with '{}', falling back to CPU embeddings", op_desc);
            
            let model_guard = embedding_state.cpu_model.read().await;
            let model = model_guard
                .clone()
                .ok_or_else(|| "CPU embedding model not initialized".to_string())?;
            drop(model_guard);
            
            (model, false)
        }
    };

    let (tx, rx) = oneshot::channel();
    handles
        .rag_tx
        .send(RagMsg::IndexRagDocuments {
            paths,
            embedding_model,
            use_gpu,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let result = rx.await.map_err(|_| "RAG actor died".to_string())?;

    // Only re-warm LLM if we used GPU embeddings (which may have evicted the LLM)
    if result.is_ok() && use_gpu {
        println!("[RAG] Indexing complete (GPU), triggering LLM re-warm");
        let (rewarm_tx, _) = oneshot::channel();
        let _ = handles
            .foundry_tx
            .send(FoundryMsg::RewarmCurrentModel {
                respond_to: rewarm_tx,
            })
            .await;
    } else if result.is_ok() {
        println!("[RAG] Indexing complete (CPU), no re-warm needed");
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
