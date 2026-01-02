//! Chat and history management Tauri commands.
//!
//! Commands for managing chat sessions, searching history, and controlling
//! the generation lifecycle.
//!
//! Note: The main `chat` command and related functions are defined in lib.rs
//! due to their extensive dependencies on the agentic loop. This module
//! contains the simpler chat-related commands.

use crate::app_state::{ActorHandles, CancellationState, TurnProgress, TurnTrackerState};
use crate::protocol::{FoundryMsg, VectorMsg};
use std::io::Write;
use tauri::{Emitter, State};
use tokio::sync::oneshot;

/// Search chat history by semantic similarity
#[tauri::command]
pub async fn search_history(
    query: String,
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Ask Foundry Actor for embedding (use CPU to avoid evicting LLM from GPU)
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

    // Wait for embedding
    let embedding = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // Send to Vector Actor
    let (search_tx, search_rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::SearchChatsByEmbedding {
            query_vector: embedding,
            limit: 10,
            respond_to: search_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    let results = search_rx.await.map_err(|_| "Vector actor died")?;

    app_handle
        .emit("sidebar-update", results)
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Get all chat summaries for the sidebar
#[tauri::command]
pub async fn get_all_chats(
    handles: State<'_, ActorHandles>,
) -> Result<Vec<crate::protocol::ChatSummary>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::FetchAllChats { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

/// Delete a chat by ID
#[tauri::command]
pub async fn delete_chat(id: String, handles: State<'_, ActorHandles>) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::DeleteChatById { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

/// Load a chat's messages by ID
#[tauri::command]
pub async fn load_chat(
    id: String,
    handles: State<'_, ActorHandles>,
) -> Result<Option<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::FetchChatMessages { id, respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

/// Update chat title and/or pinned status
#[tauri::command]
pub async fn update_chat(
    id: String,
    title: Option<String>,
    pinned: Option<bool>,
    handles: State<'_, ActorHandles>,
) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .vector_tx
        .send(VectorMsg::UpdateChatTitleAndPin {
            id,
            title,
            pinned,
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Vector actor died".to_string())
}

/// Cancel an in-progress generation
#[tauri::command]
pub async fn cancel_generation(
    generation_id: u32,
    cancellation_state: State<'_, CancellationState>,
) -> Result<(), String> {
    println!("\n[cancel_generation] STOP BUTTON PRESSED - User requested cancellation");
    println!(
        "[cancel_generation] Requested generation_id: {}",
        generation_id
    );
    let _ = std::io::stdout().flush();

    // Check if this matches the current generation
    let current_gen = {
        let guard = cancellation_state.current_generation_id.read().await;
        *guard
    };

    println!(
        "[cancel_generation] Current active generation_id: {}",
        current_gen
    );
    let _ = std::io::stdout().flush();

    if current_gen != generation_id {
        println!(
            "[cancel_generation] Mismatch - requested {} but active is {}, ignoring",
            generation_id, current_gen
        );
        return Ok(());
    }

    // Send cancel signal
    let signal = {
        let guard = cancellation_state.cancel_signal.read().await;
        guard.clone()
    };

    if let Some(sender) = signal {
        println!("[cancel_generation] Sending cancel signal to generation {}", generation_id);
        let _ = std::io::stdout().flush();
        if let Err(e) = sender.send(true) {
            println!(
                "[cancel_generation] Warning: failed to send cancel signal: {:?}",
                e
            );
        } else {
            println!("[cancel_generation] Cancel signal sent successfully");
        }
    } else {
        println!("[cancel_generation] No active cancel signal channel found");
    }
    let _ = std::io::stdout().flush();

    Ok(())
}

/// Get the current turn progress status
#[tauri::command]
pub async fn get_turn_status(
    turn_tracker: State<'_, TurnTrackerState>,
) -> Result<TurnProgress, String> {
    let guard = turn_tracker.progress.read().await;
    Ok(guard.clone())
}

/// Log a message from the frontend to the terminal
#[tauri::command]
pub fn log_to_terminal(message: String) {
    println!("[FRONTEND] {}", message);
}

/// Handle frontend heartbeat for monitoring
#[tauri::command]
pub async fn heartbeat_ping(
    heartbeat_state: State<'_, crate::app_state::HeartbeatState>,
) -> Result<(), String> {
    use std::time::Instant;

    let mut last = heartbeat_state.last_frontend_beat.write().await;
    *last = Some(Instant::now());

    // Clear any previous unresponsive flag so a new gap will log once.
    let mut logged = heartbeat_state.logged_unresponsive.write().await;
    *logged = false;

    Ok(())
}
