//! Commands for detecting and installing the MCP Database Toolbox used by the demo database.

use crate::settings::find_toolbox_binary;
use crate::toolbox_install::{
    download_toolbox_verified, installed_toolbox_path, toolbox_download_spec,
    ToolboxInstallProgress, TOOLBOX_VERSION,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct ToolboxInstallStatus {
    /// Path of the toolbox the app would use right now, if any.
    pub toolbox_path: Option<String>,
    /// Whether this platform has a pinned download available.
    pub download_supported: bool,
    pub download_size_bytes: Option<u64>,
    pub pinned_version: String,
}

/// Report whether a toolbox binary can be found and whether the app can download one.
#[tauri::command]
pub async fn get_toolbox_install_status() -> ToolboxInstallStatus {
    let spec = toolbox_download_spec();
    ToolboxInstallStatus {
        toolbox_path: find_toolbox_binary(),
        download_supported: spec.is_some(),
        download_size_bytes: spec.map(|s| s.size_bytes),
        pinned_version: TOOLBOX_VERSION.to_string(),
    }
}

/// Download the pinned toolbox, verify its SHA-256, and install it into the app data directory.
/// Emits `toolbox-install-progress` events and returns the installed path.
#[tauri::command]
pub async fn install_toolbox(app_handle: AppHandle) -> Result<String, String> {
    let spec = toolbox_download_spec()
        .ok_or_else(|| "No toolbox download is available for this platform; install it manually from https://github.com/googleapis/genai-toolbox".to_string())?;

    let progress_handle = app_handle.clone();
    let mut last_emitted_percent: u64 = u64::MAX;
    let result = download_toolbox_verified(
        spec.url,
        spec.sha256_hex,
        &installed_toolbox_path(),
        move |downloaded_bytes, total_bytes| {
            let total = if total_bytes > 0 { total_bytes } else { spec.size_bytes };
            let percent = downloaded_bytes * 100 / total.max(1);
            if percent != last_emitted_percent {
                last_emitted_percent = percent;
                let _ = progress_handle.emit(
                    "toolbox-install-progress",
                    ToolboxInstallProgress {
                        downloaded_bytes,
                        total_bytes: total,
                        is_complete: false,
                        error: None,
                    },
                );
            }
        },
    )
    .await;

    let (is_complete, error) = match &result {
        Ok(_) => (true, None),
        Err(e) => (false, Some(e.clone())),
    };
    let _ = app_handle.emit(
        "toolbox-install-progress",
        ToolboxInstallProgress {
            downloaded_bytes: spec.size_bytes,
            total_bytes: spec.size_bytes,
            is_complete,
            error,
        },
    );
    result.map(|p| p.to_string_lossy().to_string())
}
