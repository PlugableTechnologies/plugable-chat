//! Service lifecycle management for Foundry Local.
//!
//! This module handles:
//! - Finding the Foundry CLI binary
//! - Service status parsing structures
//! - Helper types for model management

use crate::process_utils::HideConsoleWindow;
use serde::Deserialize;

/// The model the app selects and downloads by default when the user has not chosen one.
/// Matched as a substring of the model id (for example `qwen3.5-4b-cuda-gpu:4`).
///
/// If it cannot run on this machine (a deterministic load or generation failure is recorded
/// in the version-keyed blocklist) the app falls back to [`DEFAULT_FALLBACK_MODEL`].
/// Measured 2026-09-29: this model fails to generate on an NVIDIA T4 (Turing) with
/// `LinearAttention ... CUDA failure 1: invalid argument`.
pub const DEFAULT_MODEL: &str = "qwen3.5-4b";

/// The last-resort model, used when [`DEFAULT_MODEL`] is missing or known-incompatible, and
/// when errors occur. It is never added to the incompatibility blocklist, so the app always
/// has something to fall back to. Downloaded on demand by the frontend's fallback handler.
pub const DEFAULT_FALLBACK_MODEL: &str = "phi-4-mini-instruct";

/// Where the Windows `winget`/MSIX installs of Foundry Local put `foundry.exe`, so a GUI-launched
/// app finds it even when its inherited PATH lacks the alias directory. Pure so it is testable
/// on any OS.
pub fn windows_foundry_candidates(local_app_data: Option<&str>, program_files: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(l) = local_app_data {
        out.push(format!("{l}\\Microsoft\\WindowsApps\\foundry.exe"));
        out.push(format!("{l}\\Programs\\Microsoft\\FoundryLocal\\foundry.exe"));
        out.push(format!("{l}\\Microsoft\\Foundry\\foundry.exe"));
    }
    if let Some(p) = program_files {
        out.push(format!("{p}\\Microsoft\\FoundryLocal\\foundry.exe"));
        out.push(format!("{p}\\Microsoft\\Foundry\\foundry.exe"));
    }
    out
}

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

    for path in windows_foundry_candidates(
        std::env::var("LOCALAPPDATA").ok().as_deref(),
        std::env::var("ProgramFiles").ok().as_deref(),
    ) {
        if cfg!(windows) && std::path::Path::new(&path).exists() {
            println!("FoundryActor: Found foundry at Windows install location: {}", path);
            return path;
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

/// Port from a loopback URL in a status line. Accepts http/https and 127.0.0.1, localhost or
/// [::1], because the CLI's hostname wording differs between releases and platforms.
fn parse_local_port(line: &str) -> Option<u16> {
    for scheme in ["http://", "https://"] {
        for host in ["127.0.0.1:", "localhost:", "[::1]:"] {
            let needle = format!("{scheme}{host}");
            if let Some(i) = line.find(&needle) {
                let digits: String = line[i + needle.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(p) = digits.parse::<u16>() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Parse the output of `foundry service status`
pub fn parse_foundry_service_status_output(output: &str) -> ServiceStatus {
    let mut port = None;
    let mut registered_eps = Vec::new();
    let mut valid_eps = Vec::new();

    for line in output.lines() {
        if let Some(p) = parse_local_port(line) {
            port = Some(p);
            println!("FoundryActor: Detected port {}", p);
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
mod candidate_tests {
    use super::parse_foundry_service_status_output as parse;

    #[test]
    fn status_parser_reads_port_from_any_loopback_spelling() {
        assert_eq!(parse("running on http://127.0.0.1:54657/openai/status").port, Some(54657));
        assert_eq!(parse("running on http://localhost:5273/").port, Some(5273));
        assert_eq!(parse("https://[::1]:6000").port, Some(6000));
        assert_eq!(parse("service is stopped").port, None);
    }

    #[test]
    fn status_parser_reads_eps() {
        let s = parse("registered the following EPs: CUDA, CPU.\nValid EPs: CUDA, CPU, WebGPU");
        assert_eq!(s.registered_eps, vec!["CUDA", "CPU"]);
        assert_eq!(s.valid_eps.len(), 3);
    }

    use super::windows_foundry_candidates as c;

    #[test]
    fn windows_candidates_cover_winget_alias_and_install_dirs() {
        let v = c(Some("C:\\Users\\a\\AppData\\Local"), Some("C:\\Program Files"));
        assert!(v.iter().any(|p| p.ends_with("Microsoft\\WindowsApps\\foundry.exe")));
        assert!(v.iter().any(|p| p.ends_with("Programs\\Microsoft\\FoundryLocal\\foundry.exe")));
        assert!(v.iter().any(|p| p.starts_with("C:\\Program Files\\Microsoft\\FoundryLocal")));
    }

    #[test]
    fn no_env_means_no_candidates() {
        assert!(c(None, None).is_empty());
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
