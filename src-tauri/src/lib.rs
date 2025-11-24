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
async fn chat(
    message: String,
    history: Vec<ChatMessage>,
    handles: State<'_, ActorHandles>
) -> Result<String, String> {
    let (tx, rx) = oneshot::channel();
    
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

    rx.await.map_err(|_| "Foundry actor died".to_string())
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
             tauri::async_runtime::spawn(async move {
                 println!("Starting Foundry Actor...");
                 let actor = FoundryActor::new(foundry_rx);
                 actor.run().await;
             });
             
             Ok(())
        })
        .invoke_handler(tauri::generate_handler![search_history, chat])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
