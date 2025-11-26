pub mod protocol;
pub mod actors;

use protocol::{VectorMsg, FoundryMsg, RagMsg, ChatMessage, CachedModel, RagChunk, RagIndexResult};
use actors::vector_actor::VectorActor;
use actors::foundry_actor::FoundryActor;
use actors::rag_actor::RagActor;
use tauri::{State, Manager, Emitter};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;
use std::sync::Arc;
use tokio::sync::RwLock;
use fastembed::TextEmbedding;

// State managed by Tauri
struct ActorHandles {
    vector_tx: mpsc::Sender<VectorMsg>,
    foundry_tx: mpsc::Sender<FoundryMsg>,
    rag_tx: mpsc::Sender<RagMsg>,
}

// Shared embedding model for RAG operations
struct EmbeddingModelState {
    model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
}

#[tauri::command]
async fn search_history(
    query: String, 
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle
) -> Result<(), String> {
    // Ask Foundry Actor for embedding
    let (emb_tx, emb_rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetEmbedding { 
        text: query, 
        respond_to: emb_tx 
    }).await.map_err(|e| e.to_string())?;

    // Wait for embedding
    let embedding = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // Send to Vector Actor
    let (search_tx, search_rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::SearchHistory {
        query_vector: embedding,
        limit: 10,
        respond_to: search_tx
    }).await.map_err(|e| e.to_string())?;

    let results = search_rx.await.map_err(|_| "Vector actor died")?;
    
    app_handle.emit("sidebar-update", results).map_err(|e| e.to_string())?;
    
    Ok(())
}

#[tauri::command]
async fn get_all_chats(handles: State<'_, ActorHandles>) -> Result<Vec<protocol::ChatSummary>, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::GetAllChats { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn get_models(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn set_model(model: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::SetModel { model_id: model, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn get_cached_models(handles: State<'_, ActorHandles>) -> Result<Vec<CachedModel>, String> {
    let (tx, rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetCachedModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

#[tauri::command]
async fn chat(
    chat_id: Option<String>,
    title: Option<String>,
    message: String,
    history: Vec<ChatMessage>,
    reasoning_effort: String,
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle
) -> Result<String, String> {
    let chat_id = chat_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let chat_id_return = chat_id.clone(); // Clone for return value
    let title = title.unwrap_or_else(|| message.chars().take(50).collect::<String>());
    
    // Use unbounded channel to prevent blocking on long responses
    let (tx, mut rx) = mpsc::unbounded_channel();
    
    // Add the new message to history
    let mut full_history = history.clone();
    full_history.push(ChatMessage {
        role: "user".to_string(),
        content: message.clone(),
    });

    handles.foundry_tx.send(FoundryMsg::Chat {
        history: full_history.clone(),
        reasoning_effort: reasoning_effort.clone(),
        respond_to: tx,
    }).await.map_err(|e| e.to_string())?;

    // State<T> is a wrapper around Arc<T>, so we can clone it? No, State is not Clone.
    // But ActorHandles fields are senders which are cloneable.
    let vector_tx = handles.vector_tx.clone();
    let foundry_tx = handles.foundry_tx.clone();
    let chat_id_task = chat_id.clone();

    // Spawn a task to forward tokens to frontend
    tauri::async_runtime::spawn(async move {
        let mut assistant_response = String::new();
        
        while let Some(token) = rx.recv().await {
            assistant_response.push_str(&token);
            let _ = app_handle.emit("chat-token", token);
        }
        let _ = app_handle.emit("chat-finished", ());
        
        println!("[ChatSave] Response complete, saving chat {}...", &chat_id_task[..8.min(chat_id_task.len())]);
        
        // Save chat
        full_history.push(ChatMessage {
            role: "assistant".to_string(),
            content: assistant_response.clone(),
        });
        
        let messages_json = serde_json::to_string(&full_history).unwrap_or_default();
        
        // Generate embedding for the chat content (using last user message + assistant response for relevance)
        // Or better, use the title + last interaction.
        let embedding_text = format!("{}\nUser: {}\nAssistant: {}", title, message, assistant_response);
        
        println!("[ChatSave] Requesting embedding...");
        let (emb_tx, emb_rx) = oneshot::channel();
        match foundry_tx.send(FoundryMsg::GetEmbedding { 
            text: embedding_text.clone(), 
            respond_to: emb_tx 
        }).await {
            Ok(_) => {
                println!("[ChatSave] Waiting for embedding response...");
                match emb_rx.await {
                    Ok(vector) => {
                        println!("[ChatSave] Got embedding (len={}), sending to VectorActor...", vector.len());
                        match vector_tx.send(VectorMsg::UpsertChat {
                            id: chat_id_task.clone(),
                            title: title.clone(),
                            content: embedding_text,
                            messages: messages_json,
                            vector: Some(vector),
                            pinned: false, 
                        }).await {
                            Ok(_) => {
                                println!("[ChatSave] UpsertChat sent, emitting chat-saved event");
                                let _ = app_handle.emit("chat-saved", chat_id_task.clone());
                            }
                            Err(e) => println!("[ChatSave] ERROR: Failed to send UpsertChat: {}", e),
                        }
                    }
                    Err(e) => println!("[ChatSave] ERROR: Failed to receive embedding: {}", e),
                }
            }
            Err(e) => println!("[ChatSave] ERROR: Failed to send GetEmbedding: {}", e),
        }
    });

    Ok(chat_id_return)
}

#[tauri::command]
async fn delete_chat(id: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::DeleteChat { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn load_chat(id: String, handles: State<'_, ActorHandles>) -> Result<Option<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::GetChat { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
async fn update_chat(id: String, title: Option<String>, pinned: Option<bool>, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::UpdateChatMetadata { id, title, pinned, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

#[tauri::command]
fn log_to_terminal(message: String) {
    println!("[FRONTEND] {}", message);
}

// ============ RAG Commands ============

#[tauri::command]
async fn select_files() -> Result<Vec<String>, String> {
    // Note: File selection is handled directly by the frontend using the dialog plugin
    // This command is kept for potential future use
    Ok(Vec::new())
}

#[tauri::command]
async fn select_folder() -> Result<Option<String>, String> {
    // Similar to select_files - frontend will use dialog plugin directly
    Ok(None)
}

#[tauri::command]
async fn process_rag_documents(
    paths: Vec<String>,
    handles: State<'_, ActorHandles>,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<RagIndexResult, String> {
    println!("[RAG] Processing {} paths", paths.len());
    
    // Get the embedding model
    let model_guard = embedding_state.model.read().await;
    let embedding_model = model_guard.clone()
        .ok_or_else(|| "Embedding model not initialized".to_string())?;
    drop(model_guard);
    
    let (tx, rx) = oneshot::channel();
    handles.rag_tx.send(RagMsg::ProcessDocuments {
        paths,
        embedding_model,
        respond_to: tx,
    }).await.map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "RAG actor died".to_string())?
}

#[tauri::command]
async fn search_rag_context(
    query: String,
    limit: usize,
    handles: State<'_, ActorHandles>,
) -> Result<Vec<RagChunk>, String> {
    println!("[RAG] Searching for context with query length: {}", query.len());
    
    // First, get embedding for the query
    let (emb_tx, emb_rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetEmbedding {
        text: query,
        respond_to: emb_tx,
    }).await.map_err(|e| e.to_string())?;
    
    let query_vector = emb_rx.await.map_err(|_| "Foundry actor died")?;
    
    // Then search the RAG index
    let (search_tx, search_rx) = oneshot::channel();
    handles.rag_tx.send(RagMsg::SearchDocuments {
        query_vector,
        limit,
        respond_to: search_tx,
    }).await.map_err(|e| e.to_string())?;
    
    search_rx.await.map_err(|_| "RAG actor died".to_string())
}

#[tauri::command]
async fn clear_rag_context(handles: State<'_, ActorHandles>) -> Result<bool, String> {
    println!("[RAG] Clearing context");
    
    let (tx, rx) = oneshot::channel();
    handles.rag_tx.send(RagMsg::ClearContext { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    
    rx.await.map_err(|_| "RAG actor died".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
             // Initialize channels
             let (vector_tx, vector_rx) = mpsc::channel(32);
             let (foundry_tx, foundry_rx) = mpsc::channel(32);
             let (rag_tx, rag_rx) = mpsc::channel(32);
             
             // Store handles in state
             app.manage(ActorHandles { vector_tx, foundry_tx, rag_tx });
             
             // Initialize shared embedding model state
             let embedding_model_state = EmbeddingModelState {
                 model: Arc::new(RwLock::new(None)),
             };
             let embedding_model_arc = embedding_model_state.model.clone();
             app.manage(embedding_model_state);

             let app_handle = app.handle();
             // Spawn Vector Actor
             tauri::async_runtime::spawn(async move {
                 println!("Starting Vector Actor...");
                 // Ensure data directory exists
                 let _ = tokio::fs::create_dir_all("./data").await;
                 
                 let actor = VectorActor::new(vector_rx, "./data/lancedb").await;
                 println!("Vector Actor initialized.");
                 actor.run().await;
             });

             // Spawn Foundry Actor
            let foundry_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                 println!("Starting Foundry Actor...");
                let actor = FoundryActor::new(foundry_rx, foundry_app_handle);
                 actor.run().await;
             });
             
             // Spawn RAG Actor
             tauri::async_runtime::spawn(async move {
                 println!("Starting RAG Actor...");
                 let actor = RagActor::new(rag_rx);
                 actor.run().await;
             });
             
             // Initialize embedding model in background (shared between FoundryActor and RAG)
             tauri::async_runtime::spawn(async move {
                 println!("Initializing shared embedding model for RAG...");
                 use fastembed::{InitOptions, EmbeddingModel};
                 
                 match tokio::task::spawn_blocking(|| {
                     let mut options = InitOptions::default();
                     options.model_name = EmbeddingModel::AllMiniLML6V2;
                     options.show_download_progress = true;
                     TextEmbedding::try_new(options)
                 }).await {
                     Ok(Ok(model)) => {
                         let mut guard = embedding_model_arc.write().await;
                         *guard = Some(Arc::new(model));
                         println!("Shared embedding model initialized successfully");
                     }
                     Ok(Err(e)) => {
                         println!("ERROR: Failed to initialize embedding model: {}", e);
                     }
                     Err(e) => {
                         println!("ERROR: Embedding model task panicked: {}", e);
                     }
                 }
             });
             
             Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search_history, 
            chat, 
            get_models, 
            get_cached_models, 
            set_model, 
            get_all_chats, 
            log_to_terminal, 
            delete_chat, 
            load_chat, 
            update_chat,
            // RAG commands
            select_files,
            select_folder,
            process_rag_documents,
            search_rag_context,
            clear_rag_context
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
