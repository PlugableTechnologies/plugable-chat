//! Centralized path resolution for cross-platform compatibility.
//!
//! This module provides platform-aware directory resolution with automatic
//! fallback chains when write access is denied. All path resolution in the
//! application should go through this module.
//!
//! ## Platform Directory Standards
//!
//! | Purpose | macOS | Windows |
//! |---------|-------|---------|
//! | Config | `~/Library/Application Support/plugable-chat/` | `%APPDATA%\plugable-chat\` |
//! | Data | `~/Library/Application Support/plugable-chat/data/` | `%LOCALAPPDATA%\plugable-chat\data\` |
//! | Cache | `~/Library/Caches/plugable-chat/` | `%LOCALAPPDATA%\plugable-chat\cache\` |

use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

/// Application name used in directory paths
const APP_NAME: &str = "plugable-chat";

/// Result of attempting to get a writable directory
#[derive(Debug, Clone)]
pub struct WritableDir {
    /// The path that was determined to be writable
    pub path: PathBuf,
    /// Whether this is a fallback location (not the primary platform-standard location)
    pub is_fallback: bool,
    /// Description of which fallback tier was used (if any)
    pub fallback_reason: Option<String>,
}

/// Get the configuration directory (for config.json, settings).
///
/// Uses Roaming AppData on Windows for cross-machine sync potential.
/// - macOS: `~/Library/Application Support/plugable-chat/`
/// - Windows: `%APPDATA%\plugable-chat\`
pub fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join(APP_NAME))
        .unwrap_or_else(|| fallback_base_dir().join("config"))
}

/// Get the data directory (for LanceDB, large persistent data).
///
/// Uses Local AppData on Windows (machine-specific, not synced to roaming).
/// - macOS: `~/Library/Application Support/plugable-chat/data/`
/// - Windows: `%LOCALAPPDATA%\plugable-chat\data\`
pub fn get_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .map(|p| p.join(APP_NAME).join("data"))
        .unwrap_or_else(|| fallback_base_dir().join("data"))
}

/// Get the cache directory (for temporary/regenerable caches).
///
/// - macOS: `~/Library/Caches/plugable-chat/`
/// - Windows: `%LOCALAPPDATA%\plugable-chat\cache\`
pub fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|p| p.join(APP_NAME))
        .unwrap_or_else(|| fallback_base_dir().join("cache"))
}

/// Get the RAG cache directory for a specific source file.
///
/// RAG caches are stored as sidecars next to the indexed files when possible,
/// falling back to a central cache location when the source directory is read-only.
///
/// Returns the cache directory path (not yet validated for writability).
pub fn get_rag_sidecar_cache_dir(source_file: &std::path::Path) -> PathBuf {
    source_file
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".plugable-rag-cache")
}

/// Get the central RAG cache directory for fallback when sidecar caches fail.
///
/// This is used when the source file's directory is read-only.
pub fn get_central_rag_cache_dir() -> PathBuf {
    get_cache_dir().join("rag")
}

/// Fallback base directory when platform dirs are unavailable.
///
/// Tries in order:
/// 1. `~/.plugable-chat/` (home directory)
/// 2. `./.plugable-chat/` (current working directory)
fn fallback_base_dir() -> PathBuf {
    dirs::home_dir()
        .map(|p| p.join(".plugable-chat"))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".plugable-chat")
        })
}

/// Test if a directory is writable by creating and removing a test file.
async fn test_write_access(dir: &PathBuf) -> bool {
    // First, try to create the directory
    if fs::create_dir_all(dir).await.is_err() {
        return false;
    }

    // Then try to write a test file
    let test_file = dir.join(format!(".write-test-{}", Uuid::new_v4()));
    match fs::write(&test_file, b"test").await {
        Ok(_) => {
            // Clean up test file
            let _ = fs::remove_file(&test_file).await;
            true
        }
        Err(_) => false,
    }
}

/// Ensure a directory is writable, with automatic fallback chain.
///
/// The fallback chain tries directories in order:
/// 1. **Primary** - The provided platform-standard directory
/// 2. **Home fallback** - `~/.plugable-chat/{purpose}/`
/// 3. **CWD fallback** - `./.plugable-chat/{purpose}/`
/// 4. **Memory** - Returns `memory://` as a last resort (for LanceDB compatibility)
///
/// Each step attempts `create_dir_all()` and writes a test file to verify actual write access.
///
/// # Arguments
/// * `primary` - The primary (preferred) directory path
/// * `purpose` - A short identifier for logging (e.g., "lancedb", "rag-cache")
///
/// # Returns
/// A `WritableDir` containing the usable path and whether it's a fallback location.
pub async fn ensure_writable_dir(primary: PathBuf, purpose: &str) -> WritableDir {
    // Try primary location
    if test_write_access(&primary).await {
        println!(
            "[Paths] Using primary directory for {}: {:?}",
            purpose, primary
        );
        return WritableDir {
            path: primary,
            is_fallback: false,
            fallback_reason: None,
        };
    }

    println!(
        "[Paths] Primary directory not writable for {}: {:?}",
        purpose, primary
    );

    // Try home directory fallback
    if let Some(home) = dirs::home_dir() {
        let home_fallback = home.join(".plugable-chat").join(purpose);
        if test_write_access(&home_fallback).await {
            println!(
                "[Paths] WARNING: Using home fallback for {}: {:?}",
                purpose, home_fallback
            );
            return WritableDir {
                path: home_fallback,
                is_fallback: true,
                fallback_reason: Some("Primary location not writable, using home directory".to_string()),
            };
        }
        println!(
            "[Paths] Home fallback not writable for {}: {:?}",
            purpose, home_fallback
        );
    }

    // Try CWD fallback
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_fallback = cwd.join(".plugable-chat").join(purpose);
        if test_write_access(&cwd_fallback).await {
            println!(
                "[Paths] WARNING: Using CWD fallback for {}: {:?}",
                purpose, cwd_fallback
            );
            return WritableDir {
                path: cwd_fallback,
                is_fallback: true,
                fallback_reason: Some("Using current directory as fallback".to_string()),
            };
        }
        println!(
            "[Paths] CWD fallback not writable for {}: {:?}",
            purpose, cwd_fallback
        );
    }

    // Last resort: memory (for LanceDB)
    println!(
        "[Paths] CRITICAL: All filesystem locations failed for {}. Using memory://",
        purpose
    );
    WritableDir {
        path: PathBuf::from("memory://"),
        is_fallback: true,
        fallback_reason: Some("All filesystem locations failed, using in-memory storage".to_string()),
    }
}

/// Ensure a directory is writable for RAG sidecar caches.
///
/// RAG caches have a special fallback pattern:
/// 1. Sidecar directory next to the source file
/// 2. Central cache with hashed path to avoid collisions
/// 3. Memory as last resort
///
/// # Arguments
/// * `source_file` - The source file being indexed
///
/// # Returns
/// A `WritableDir` containing the usable cache path.
pub async fn ensure_rag_cache_dir(source_file: &std::path::Path) -> WritableDir {
    // Try sidecar cache first
    let sidecar = get_rag_sidecar_cache_dir(source_file);
    if test_write_access(&sidecar).await {
        println!("[Paths] Using sidecar RAG cache: {:?}", sidecar);
        return WritableDir {
            path: sidecar,
            is_fallback: false,
            fallback_reason: None,
        };
    }

    println!(
        "[Paths] Sidecar cache not writable for {:?}, trying central cache",
        source_file
    );

    // Fall back to central cache with hashed path
    let central = get_central_rag_cache_dir().join(hash_path(&sidecar));
    if test_write_access(&central).await {
        println!("[Paths] Using central RAG cache: {:?}", central);
        return WritableDir {
            path: central,
            is_fallback: true,
            fallback_reason: Some(format!(
                "Source directory not writable, using central cache"
            )),
        };
    }

    println!(
        "[Paths] Central cache not writable: {:?}, using memory",
        central
    );

    // Last resort: memory
    WritableDir {
        path: PathBuf::from("memory://"),
        is_fallback: true,
        fallback_reason: Some("All cache locations failed, using in-memory storage".to_string()),
    }
}

/// Hash a path to a short string for use in central cache directories.
///
/// This creates a unique but deterministic subdirectory name for paths
/// that would otherwise collide in the central cache.
fn hash_path(path: &PathBuf) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_dir_not_empty() {
        let config_dir = get_config_dir();
        assert!(!config_dir.as_os_str().is_empty());
        assert!(config_dir.to_string_lossy().contains("plugable-chat"));
    }

    #[test]
    fn test_get_data_dir_not_empty() {
        let data_dir = get_data_dir();
        assert!(!data_dir.as_os_str().is_empty());
        assert!(data_dir.to_string_lossy().contains("plugable-chat"));
    }

    #[test]
    fn test_get_cache_dir_not_empty() {
        let cache_dir = get_cache_dir();
        assert!(!cache_dir.as_os_str().is_empty());
        assert!(cache_dir.to_string_lossy().contains("plugable-chat"));
    }

    #[test]
    fn test_hash_path_deterministic() {
        let path = PathBuf::from("/some/test/path");
        let hash1 = hash_path(&path);
        let hash2 = hash_path(&path);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_path_different_for_different_paths() {
        let path1 = PathBuf::from("/some/test/path");
        let path2 = PathBuf::from("/some/other/path");
        let hash1 = hash_path(&path1);
        let hash2 = hash_path(&path2);
        assert_ne!(hash1, hash2);
    }

    #[tokio::test]
    async fn test_ensure_writable_dir_primary_success() {
        let temp_dir = std::env::temp_dir().join(format!("plugable-test-{}", Uuid::new_v4()));
        let result = ensure_writable_dir(temp_dir.clone(), "test").await;
        
        assert!(!result.is_fallback);
        assert_eq!(result.path, temp_dir);
        
        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir).await;
    }
}
