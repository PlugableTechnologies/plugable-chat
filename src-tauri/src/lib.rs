pub mod protocol;
pub mod actors;

use protocol::{VectorMsg, FoundryMsg, ChatMessage};
use actors::vector_actor::VectorActor;
use actors::foundry_actor::FoundryActor;
use tauri::{State, Manager, Emitter};
use tokio::sync::{mpsc, oneshot};

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
    message: String,
    history: Vec<ChatMessage>,
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle
) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel(100);
    
    // Add the new message to history
    let mut full_history = history;
    full_history.push(ChatMessage {
        role: "user".to_string(),
        content: message,
    });

    handles.foundry_tx.send(FoundryMsg::Chat {
        history: full_history,
        respond_to: tx,
    }).await.map_err(|e| e.to_string())?;

    // Spawn a task to forward tokens to frontend
    tauri::async_runtime::spawn(async move {
        while let Some(token) = rx.recv().await {
            let _ = app_handle.emit("chat-token", token);
        }
        let _ = app_handle.emit("chat-finished", ());
    });

    Ok(())
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
        .invoke_handler(tauri::generate_handler![search_history, chat, get_models, set_model, get_all_chats, log_to_terminal])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
