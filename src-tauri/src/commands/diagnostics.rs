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

/// Try GPU provider registration again and wait for the result (backoff included), then return
/// the assessment as it stands. Progress arrives on `ep-registration-progress` meanwhile.
#[tauri::command]
pub async fn retry_gpu_registration(app_handle: AppHandle) -> Result<crate::gpu_assessment::GpuAssessment, String> {
    if crate::actors::foundry::gpu_registration::sdk().is_none() {
        return Err("GPU acceleration setup is not available: the Foundry SDK runtime is not in use.".to_string());
    }
    let outcome = crate::actors::foundry::gpu_registration::retry_now(app_handle).await;
    if !outcome.started {
        return Err("A GPU setup attempt is already running. Wait for it to finish.".to_string());
    }
    Ok(crate::preflight::assessment_now())
}

/// Why the GPU is or is not in use, from Windows' view and the model engine's view together.
/// `refresh` asks Windows again; the answer is never reused from a failed earlier look.
#[tauri::command]
pub async fn get_gpu_assessment(refresh: Option<bool>) -> Result<crate::gpu_assessment::GpuAssessment, String> {
    tokio::task::spawn_blocking(move || {
        if refresh.unwrap_or(false) {
            crate::preflight::refresh_blocking()
        } else {
            crate::preflight::assessment_now()
        }
    })
    .await
    .map_err(|e| format!("GPU assessment failed: {e}"))
}

/// Everything support needs in one block of text: versions, machine facts, the GPU view from both
/// sides, and the last lines of the app log. Written to the logs folder as well.
#[tauri::command]
pub async fn get_support_bundle(extra: Option<String>) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let report = crate::preflight::build_report_blocking();
        let mut text = crate::preflight::format_report(&report);
        if let Some(extra) = extra.filter(|e| !e.trim().is_empty()) {
            text.push_str("\n\nWhat the app showed:\n");
            text.push_str(extra.trim());
        }
        let tail = crate::app_log::tail(60);
        if !tail.is_empty() {
            text.push_str("\n\nRecent log:\n");
            text.push_str(&tail.join("\n"));
        }
        text
    })
    .await
    .map_err(|e| format!("could not collect support details: {e}"))
}

/// Run the embedding-model start-up again (download included) after it failed.
#[tauri::command]
pub async fn retry_embedding_init(
    app_handle: AppHandle,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<(), String> {
    crate::actors::foundry::embedding_init::init_cpu_embedding_model(app_handle, embedding_state.cpu_model.clone()).await
}
