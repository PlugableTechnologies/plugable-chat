//! First-run diagnostics and self-service retries.
//!
//! These commands let the UI say why GPU models or the embedding model are unavailable and
//! offer a Retry that works without restarting the app.

use crate::app_state::EmbeddingModelState;
use crate::gpu_diagnostics::{self, GpuDiagnostics};
use tauri::{AppHandle, State};

/// Why GPU acceleration is or is not available: registered providers, classified failures,
/// NVIDIA driver version and whether the Visual C++ runtime is present.
#[tauri::command]
pub async fn get_gpu_diagnostics() -> Result<GpuDiagnostics, String> {
    tokio::task::spawn_blocking(gpu_diagnostics::snapshot_blocking)
        .await
        .map_err(|e| format!("GPU diagnostics probe failed: {e}"))
}

/// Try GPU provider registration again, with backoff, without restarting the app. Progress
/// arrives on `ep-registration-progress`; returns the diagnostics as they stand now.
#[tauri::command]
pub async fn retry_gpu_registration(app_handle: AppHandle) -> Result<GpuDiagnostics, String> {
    if crate::actors::foundry::gpu_registration::sdk().is_none() {
        return Err("GPU acceleration setup is not available: the Foundry SDK runtime is not in use.".to_string());
    }
    // Already running is not an error: the caller wants the attempt, and one is in flight.
    let _started = crate::actors::foundry::gpu_registration::spawn_retry(app_handle);
    Ok(gpu_diagnostics::snapshot_without_probe())
}

/// Run the embedding-model start-up again (download included) after it failed.
#[tauri::command]
pub async fn retry_embedding_init(
    app_handle: AppHandle,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<(), String> {
    crate::actors::foundry::embedding_init::init_cpu_embedding_model(app_handle, embedding_state.cpu_model.clone()).await
}
