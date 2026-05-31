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

/// Detect the installed Foundry Local version by running `foundry --version`.
///
/// Used to key the incompatible-models blocklist: a model that fails to load under
/// one runtime version may load fine after an upgrade, so blocklist entries are scoped
/// to the version that produced the failure. Returns the trimmed version string
/// (e.g. "0.8.119") or `None` if the binary can't be run / parsed.
pub fn get_foundry_version() -> Option<String> {
    let foundry_bin = find_foundry_binary();
    let output = std::process::Command::new(&foundry_bin)
        .arg("--version")
        .hide_console_window()
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_foundry_version_output(&stdout)
}

/// Extract a version string from `foundry --version` output.
///
/// `foundry --version` prints just the semver (e.g. "0.8.119"), but we stay tolerant of any
/// surrounding text by grabbing the first dotted-numeric token on the first non-empty line,
/// falling back to the whole trimmed line.
pub fn parse_foundry_version_output(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(token) = trimmed
            .split_whitespace()
            .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))
        {
            return Some(token.to_string());
        }
        return Some(trimmed.to_string());
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_semver() {
        // `foundry --version` on 0.8.119 prints just the version.
        assert_eq!(
            parse_foundry_version_output("0.8.119\n"),
            Some("0.8.119".to_string())
        );
    }

    #[test]
    fn parses_version_with_surrounding_text() {
        assert_eq!(
            parse_foundry_version_output("foundry version 1.2.3 (build abc)"),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn skips_leading_blank_lines() {
        assert_eq!(
            parse_foundry_version_output("\n\n   0.9.0  \n"),
            Some("0.9.0".to_string())
        );
    }

    #[test]
    fn falls_back_to_trimmed_line_when_no_dotted_token() {
        // No dotted-numeric token: return the trimmed line rather than nothing.
        assert_eq!(
            parse_foundry_version_output("dev"),
            Some("dev".to_string())
        );
    }

    #[test]
    fn returns_none_for_empty_output() {
        assert_eq!(parse_foundry_version_output("   \n  \n"), None);
    }
}
