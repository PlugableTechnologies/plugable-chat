//! Commands for detecting and installing the MCP Database Toolbox used by the demo database.

use crate::settings::find_toolbox_binary;
use crate::toolbox_install::{install_pinned_toolbox, toolbox_download_spec, TOOLBOX_VERSION};
use serde::Serialize;
use tauri::AppHandle;

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
/// Emits `toolbox-download-progress` (and the older `toolbox-install-progress`) and returns the
/// installed path. Also runs automatically when a database source that needs it is enabled.
#[tauri::command]
pub async fn install_toolbox(app_handle: AppHandle) -> Result<String, String> {
    install_pinned_toolbox(&app_handle, false)
        .await
        .map(|p| p.to_string_lossy().to_string())
}
