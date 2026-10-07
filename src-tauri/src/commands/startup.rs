//! Startup coordination commands.
//!
//! This module provides commands for the frontend/backend startup handshake:
//! - `frontend_ready`: Frontend signals it's ready and receives full state snapshot

use crate::actors::startup_actor::StartupMsg;
use crate::app_state::ActorHandles;
use crate::protocol::StartupSnapshot;
use tauri::State;
use tokio::sync::oneshot;

/// Frontend signals it's ready and requests current state snapshot.
///
/// This command implements the handshake protocol:
/// 1. Frontend sets up all event listeners
/// 2. Frontend calls this command
/// 3. Backend returns complete state snapshot
/// 4. Frontend applies snapshot atomically
///
/// This ensures no events are lost during startup since the frontend
/// receives the complete current state after listeners are ready.
#[tauri::command]
pub async fn frontend_ready(handles: State<'_, ActorHandles>) -> Result<StartupSnapshot, String> {
    let (tx, rx) = oneshot::channel();
    
    handles
        .startup_tx
        .send(StartupMsg::FrontendReady { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send frontend_ready: {}", e))?;
    
    rx.await.map_err(|_| "Startup coordinator died".to_string())
}

/// Get current startup state for diagnostics.
#[tauri::command]
pub async fn get_startup_snapshot(handles: State<'_, ActorHandles>) -> Result<StartupSnapshot, String> {
    let (tx, rx) = oneshot::channel();
    
    handles
        .startup_tx
        .send(StartupMsg::GetSnapshot { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to get startup snapshot: {}", e))?;
    
    rx.await.map_err(|_| "Startup coordinator died".to_string())
}

/// Notice text if the previous launch never finished starting (crash, kill, or closed mid-setup).
#[tauri::command]
pub fn get_previous_launch_crash() -> Option<String> {
    crate::launch_marker::previous_launch_started_at().map(|at| {
        let dir = crate::paths::get_config_dir().display().to_string();
        crate::startup_failure::describe_unclean_previous_launch(crate::startup_failure::Os::current(), &dir, &at)
    })
}

/// Frontend reports startup finished (model ready, or a failure card is showing), so the
/// next launch must not report this one as unfinished.
#[tauri::command]
pub fn mark_launch_ok() {
    crate::launch_marker::clear();
}
