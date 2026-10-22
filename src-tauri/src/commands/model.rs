//! Model management Tauri commands.
//!
//! Commands for listing, loading, unloading, and managing AI models
//! through the Foundry Local service.

use crate::app_state::{ActorHandles, SettingsState};
use crate::protocol::{CachedModel, CatalogModel, FoundryMsg, FoundryServiceStatus, ModelInfo, ModelState};
use crate::settings;
use tauri::State;
use tokio::sync::oneshot;

/// Get list of available model IDs
#[tauri::command]
pub async fn get_models(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

/// Set the active model and persist selection to settings
#[tauri::command]
pub async fn set_model(
    model: String,
    handles: State<'_, ActorHandles>,
    settings_state: State<'_, SettingsState>,
) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::SetModel {
            model_id: model.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| e.to_string())?;
    let result = rx.await.map_err(|_| "Foundry actor died".to_string())?;

    // Persist the model selection to settings
    if result {
        let mut guard = settings_state.settings.write().await;
        guard.selected_model = Some(model.clone());
        if let Err(e) = settings::save_settings(&guard).await {
            // Log but don't fail - the model is already set in Foundry
            println!("[set_model] Warning: Failed to persist model selection: {}", e);
        } else {
            println!("[set_model] Model selection persisted: {}", model);
        }
    }

    Ok(result)
}

/// Get cached model information
#[tauri::command]
pub async fn get_cached_models(handles: State<'_, ActorHandles>) -> Result<Vec<CachedModel>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetCachedModels { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

/// Get detailed model info for all available models
#[tauri::command]
pub async fn get_model_info(handles: State<'_, ActorHandles>) -> Result<Vec<ModelInfo>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetModelInfo { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "Foundry actor died".to_string())
}

/// Download a model from the catalog
#[tauri::command]
pub async fn download_model(
    model_name: String,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    use crate::actors::foundry::download_cancel::{download_cancels, DOWNLOAD_CANCELLED};
    println!("[download_model] Starting download for: {}", model_name);
    // Registered here, before the actor queues it, so Cancel works while it is still waiting.
    download_cancels().register(&model_name);
    let (tx, rx) = oneshot::channel();
    let sent = handles
        .foundry_tx
        .send(FoundryMsg::DownloadModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send download request: {}", e));
    let result = match sent {
        Ok(()) => rx.await.map_err(|_| "Foundry actor died".to_string()).and_then(|r| r),
        Err(e) => Err(e),
    };
    let cancelled = download_cancels().is_cancelled(&model_name);
    download_cancels().finish(&model_name);
    match result {
        // The SDK's own wording for an interrupted download varies; the UI keys on ours.
        Err(_) if cancelled => Err(DOWNLOAD_CANCELLED.to_string()),
        other => other,
    }
}

/// "Retry anyway": forget that a model was marked as not working on this runtime.
#[tauri::command]
pub async fn clear_model_incompatible(
    model_name: String,
    settings_state: State<'_, SettingsState>,
) -> Result<bool, String> {
    let mut guard = settings_state.settings.write().await;
    let s = &mut *guard;
    let removed = crate::actors::foundry::remove_from_blocklist(
        &mut s.incompatible_models,
        &mut s.incompatible_model_reasons,
        &model_name,
    );
    if removed {
        crate::actors::foundry::failure_strikes::failure_strikes().clear(&model_name);
        settings::save_settings(&guard).await?;
    }
    Ok(removed)
}

/// Stop a queued or running model download. Does not go through the foundry actor, which is busy
/// running the download it would be asked to stop.
#[tauri::command]
pub fn cancel_model_download(model_name: String) -> bool {
    crate::actors::foundry::download_cancel::download_cancels().cancel(&model_name)
}

/// Load a model into memory
#[tauri::command]
pub async fn load_model(model_name: String, handles: State<'_, ActorHandles>) -> Result<(), String> {
    println!("[load_model] Loading model: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::LoadModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send load request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

/// Get list of currently loaded models
#[tauri::command]
pub async fn get_loaded_models(handles: State<'_, ActorHandles>) -> Result<Vec<String>, String> {
    println!("[get_loaded_models] Getting loaded models");
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetLoadedModels { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    Ok(rx.await.map_err(|_| "Foundry actor died".to_string())?)
}

/// Reload the Foundry service
#[tauri::command]
pub async fn reload_foundry(handles: State<'_, ActorHandles>) -> Result<(), String> {
    use std::io::Write;
    println!("\n[reload_foundry] Reloading foundry service (requested by UI)");
    let _ = std::io::stdout().flush();
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::Reload { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    match rx.await {
        Ok(res) => {
            println!(
                "[reload_foundry] Reload command completed with result: {:?}",
                res
            );
            let _ = std::io::stdout().flush();
            res
        }
        Err(_) => {
            println!("[reload_foundry] Foundry actor died while reloading");
            let _ = std::io::stdout().flush();
            Err("Foundry actor died".to_string())
        }
    }
}

/// Get list of models from the catalog
#[tauri::command]
pub async fn get_catalog_models(
    handles: State<'_, ActorHandles>,
) -> Result<Vec<CatalogModel>, String> {
    println!("[get_catalog_models] Getting catalog models");
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetCatalogModels { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    let models = rx.await.map_err(|_| "Foundry actor died".to_string())?;
    // A failed read comes back as an empty list. Say so: the Models tab used to show that empty
    // list as "No GPU models are available".
    if models.is_empty() {
        if let Some(why) = crate::preflight::last_catalog_error() {
            return Err(format!("The model list could not be read: {why}"));
        }
    }
    Ok(models)
}

/// Unload a model from memory
#[tauri::command]
pub async fn unload_model(model_name: String, handles: State<'_, ActorHandles>) -> Result<(), String> {
    println!("[unload_model] Unloading model: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::UnloadModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

/// Get Foundry service status
#[tauri::command]
pub async fn get_foundry_service_status(
    handles: State<'_, ActorHandles>,
) -> Result<FoundryServiceStatus, String> {
    println!("[get_foundry_service_status] Getting service status");
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetServiceStatus { respond_to: tx })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

/// Remove a cached model from disk
#[tauri::command]
pub async fn remove_cached_model(
    model_name: String,
    handles: State<'_, ActorHandles>,
) -> Result<(), String> {
    println!("[remove_cached_model] Removing cached model: {}", model_name);
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::RemoveCachedModel {
            model_name: model_name.clone(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;
    rx.await.map_err(|_| "Foundry actor died".to_string())?
}

/// Get the currently active model
#[tauri::command]
pub async fn get_current_model(handles: State<'_, ActorHandles>) -> Result<Option<ModelInfo>, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetCurrentModel { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    Ok(rx.await.map_err(|_| "Foundry actor died".to_string())?)
}

/// Get the current model state machine state
#[tauri::command]
pub async fn get_model_state(handles: State<'_, ActorHandles>) -> Result<ModelState, String> {
    let (tx, rx) = oneshot::channel();
    handles
        .foundry_tx
        .send(FoundryMsg::GetModelState { respond_to: tx })
        .await
        .map_err(|e| e.to_string())?;
    Ok(rx.await.map_err(|_| "Foundry actor died".to_string())?)
}
