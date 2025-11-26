pub mod protocol;
pub mod actors;

use protocol::{VectorMsg, FoundryMsg, ChatMessage};
use actors::vector_actor::VectorActor;
use actors::foundry_actor::FoundryActor;
use tauri::{State, Manager, Emitter};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

// State managed by Tauri
struct ActorHandles {
    vector_tx: mpsc::Sender<VectorMsg>,
    foundry_tx: mpsc::Sender<FoundryMsg>,
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
        
        // Save chat
        full_history.push(ChatMessage {
            role: "assistant".to_string(),
            content: assistant_response.clone(),
        });
        
        let messages_json = serde_json::to_string(&full_history).unwrap_or_default();
        
        // Generate embedding for the chat content (using last user message + assistant response for relevance)
        // Or better, use the title + last interaction.
        let embedding_text = format!("{}\nUser: {}\nAssistant: {}", title, message, assistant_response);
        
        let (emb_tx, emb_rx) = oneshot::channel();
        if let Ok(_) = foundry_tx.send(FoundryMsg::GetEmbedding { 
            text: embedding_text.clone(), 
            respond_to: emb_tx 
        }).await {
            if let Ok(vector) = emb_rx.await {
                if let Ok(_) = vector_tx.send(VectorMsg::UpsertChat {
                    id: chat_id_task.clone(),
                    title,
                    content: embedding_text, // This is what we search against
                    messages: messages_json,
                    vector: Some(vector),
                    pinned: false, 
                }).await {
                    let _ = app_handle.emit("chat-saved", chat_id_task.clone());
                }
            }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        
        .setup(|app| {
             // Initialize channels
             let (vector_tx, vector_rx) = mpsc::channel(32);
             let (foundry_tx, foundry_rx) = mpsc::channel(32);
             
             // Store handles in state
             app.manage(ActorHandles { vector_tx, foundry_tx });

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
             
             Ok(())
        })
        .invoke_handler(tauri::generate_handler![search_history, chat, get_models, set_model, get_all_chats, log_to_terminal, delete_chat, load_chat, update_chat])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
