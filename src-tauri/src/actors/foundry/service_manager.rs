//! Service lifecycle management for Foundry Local.
//!
//! This module handles:
//! - Finding the Foundry CLI binary
//! - Service status parsing structures
//! - Helper types for model management

use crate::process_utils::HideConsoleWindow;
use serde::Deserialize;

/// Default fallback model to use when no model is specified or when errors occur.
/// This matches the phi-4-mini-instruct model that is auto-downloaded on first launch.
pub const DEFAULT_FALLBACK_MODEL: &str = "phi-4-mini-instruct";

/// Find the foundry CLI executable, checking PATH first then common installation locations.
/// This provides a fallback for production builds where PATH may not include the foundry binary.
pub fn find_foundry_binary() -> String {
    // First try PATH using which/where (will work after fix_macos_path_env() on macOS, or natively on Windows)
    #[cfg(windows)]
    let which_result = std::process::Command::new("where.exe")
        .arg("foundry")
        .hide_console_window()
        .output();

    #[cfg(not(windows))]
    let which_result = std::process::Command::new("which")
        .arg("foundry")
        .hide_console_window()
        .output();

    if let Ok(output) = which_result {
        if output.status.success() {
            if let Some(path) = String::from_utf8_lossy(&output.stdout).lines().next() {
                let path = path.trim();
                if !path.is_empty() && std::path::Path::new(path).exists() {
                    return path.to_string();
                }
            }
        }
    }

    // Fallback to common installation locations
    let common_paths: &[&str] = &[
        #[cfg(target_os = "macos")]
        "/opt/homebrew/bin/foundry",
        #[cfg(target_os = "macos")]
        "/usr/local/bin/foundry",
        #[cfg(target_os = "windows")]
        "C:\\Program Files\\Microsoft\\Foundry\\foundry.exe",
        #[cfg(target_os = "windows")]
        "C:\\Program Files (x86)\\Microsoft\\Foundry\\foundry.exe",
        #[cfg(target_os = "linux")]
        "/usr/local/bin/foundry",
        #[cfg(target_os = "linux")]
        "/usr/bin/foundry",
    ];

    for path in common_paths {
        if std::path::Path::new(path).exists() {
            println!("FoundryActor: Found foundry at fallback location: {}", path);
            return path.to_string();
        }
    }

    // Also check home directory for user-local installations (common for installers)
    if let Some(home) = dirs::home_dir() {
        let home_paths: &[std::path::PathBuf] = &[
            #[cfg(target_os = "macos")]
            home.join(".foundry").join("bin").join("foundry"),
            #[cfg(target_os = "windows")]
            home.join("AppData").join("Local").join("Microsoft").join("Foundry").join("foundry.exe"),
            #[cfg(target_os = "linux")]
            home.join(".foundry").join("bin").join("foundry"),
        ];

        for path in home_paths {
            if path.exists() {
                let path_str = path.to_string_lossy().to_string();
                println!("FoundryActor: Found foundry in home directory: {}", path_str);
                return path_str;
            }
        }
    }

    // Last resort: return "foundry" and hope it's in PATH
    println!("FoundryActor: foundry not found in common locations, trying PATH directly");
    "foundry".to_string()
}

/// Result of parsing `foundry service status` output
pub struct ServiceStatus {
    pub port: Option<u16>,
    pub registered_eps: Vec<String>,
    pub valid_eps: Vec<String>,
}

/// Model information from Foundry API
#[derive(Debug, Deserialize)]
pub struct FoundryModel {
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response from Foundry models endpoint
#[derive(Debug, Deserialize)]
pub struct FoundryModelsResponse {
    pub data: Vec<FoundryModel>,
}

/// Parse the output of `foundry service status`
pub fn parse_foundry_service_status_output(output: &str) -> ServiceStatus {
    let mut port = None;
    let mut registered_eps = Vec::new();
    let mut valid_eps = Vec::new();

    for line in output.lines() {
        // Parse port from URL: "http://127.0.0.1:54657" or "https://127.0.0.1:54657"
        if let Some(start_idx) = line.find("http://127.0.0.1:") {
            let rest = &line[start_idx + "http://127.0.0.1:".len()..];
            let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = port_str.parse::<u16>() {
                port = Some(p);
                println!("FoundryActor: Detected port {}", p);
            }
        } else if let Some(start_idx) = line.find("https://127.0.0.1:") {
            let rest = &line[start_idx + "https://127.0.0.1:".len()..];
            let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = port_str.parse::<u16>() {
                port = Some(p);
                println!("FoundryActor: Detected port {} (https)", p);
            }
        }

        // Parse registered EPs: "registered the following EPs: EP1, EP2."
        if let Some(start_idx) = line.find("registered the following EPs:") {
            let rest = &line[start_idx + "registered the following EPs:".len()..];
            // Remove trailing period and parse comma-separated list
            let eps_str = rest.trim().trim_end_matches('.');
            registered_eps = eps_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            println!("FoundryActor: Registered EPs: {:?}", registered_eps);
        }

        // Parse valid EPs: "Valid EPs: EP1, EP2, EP3"
        if let Some(start_idx) = line.find("Valid EPs:") {
            let rest = &line[start_idx + "Valid EPs:".len()..];
            valid_eps = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            println!("FoundryActor: Valid EPs: {:?}", valid_eps);
        }
    }

    ServiceStatus {
        port,
        registered_eps,
        valid_eps,
    }
}
