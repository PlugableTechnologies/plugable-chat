//! On-demand install of the MCP Database Toolbox (`toolbox` / `toolbox.exe`).
//!
//! The built-in Chicago crimes demo database (source id `embedded-demo`) is served by Google's
//! MCP Database Toolbox (https://github.com/googleapis/genai-toolbox, Apache-2.0). The binary is
//! 119-232 MB depending on platform, so the installer does not bundle it. Instead the app
//! downloads one pinned release into the per-user data directory and verifies its SHA-256 before
//! making it executable. `find_toolbox_binary()` in `settings.rs` looks at `installed_toolbox_path()`
//! and at the locations returned by `bundled_toolbox_candidate_paths()` (next to the app's
//! resources), so a future bundled build needs no further code changes.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Pinned toolbox release. Bump together with the hashes below and `Install-McpToolbox` in
/// `scripts/windows-requirements.ps1`.
pub const TOOLBOX_VERSION: &str = "0.24.0";

/// Shown when the demo source is enabled but no toolbox binary can be found.
pub const TOOLBOX_MISSING_MESSAGE: &str = "The Chicago Crimes demo database needs the MCP Database Toolbox, which is not installed. \
Open Settings > Databases and choose \"Download toolbox\" (a one-time, checksum-verified download), \
or install it from https://github.com/googleapis/genai-toolbox and set its path.";

/// One downloadable toolbox build: where to fetch it, its expected hash, and its size for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolboxDownloadSpec {
    pub url: &'static str,
    pub sha256_hex: &'static str,
    pub size_bytes: u64,
    pub file_name: &'static str,
}

/// Download spec for the platform this binary runs on; `None` when upstream ships no build for it.
pub fn toolbox_download_spec() -> Option<ToolboxDownloadSpec> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some(ToolboxDownloadSpec {
        url: "https://storage.googleapis.com/genai-toolbox/v0.24.0/windows/amd64/toolbox.exe",
        sha256_hex: "6bcd6c288e44d5710e41623ee98716a27aa759693e00d4b22cf7a6c35e4a76d9",
        size_bytes: 216_080_384,
        file_name: "toolbox.exe",
    });
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some(ToolboxDownloadSpec {
        url: "https://storage.googleapis.com/genai-toolbox/v0.24.0/darwin/arm64/toolbox",
        sha256_hex: "b281d52fae066ae225dbdbc444c2f6e8ce466eb1bca298179cfc1e1a35cb1680",
        size_bytes: 118_777_906,
        file_name: "toolbox",
    });
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some(ToolboxDownloadSpec {
        url: "https://storage.googleapis.com/genai-toolbox/v0.24.0/darwin/amd64/toolbox",
        sha256_hex: "d3989a008b2896eb49da2b9b60fb1f33b958e175a3b5532e46fae1e344a0045d",
        size_bytes: 123_818_075,
        file_name: "toolbox",
    });
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some(ToolboxDownloadSpec {
        url: "https://storage.googleapis.com/genai-toolbox/v0.24.0/linux/amd64/toolbox",
        sha256_hex: "6de0f5c7e3b8e0749dec0ec78199207ce450dbf0cebe41404a6822780b0abd24",
        size_bytes: 232_295_432,
        file_name: "toolbox",
    });
    #[allow(unreachable_code)]
    None
}

fn toolbox_file_name() -> &'static str {
    if cfg!(windows) {
        "toolbox.exe"
    } else {
        "toolbox"
    }
}

/// Where the app installs the downloaded toolbox: `<data dir>/tools/toolbox/<version>/toolbox[.exe]`.
pub fn installed_toolbox_path() -> PathBuf {
    crate::paths::get_data_dir()
        .join("tools")
        .join("toolbox")
        .join(TOOLBOX_VERSION)
        .join(toolbox_file_name())
}

/// Locations next to the running app where a bundled toolbox would live, given the exe's directory.
pub fn bundled_toolbox_candidate_paths_for(exe_dir: &Path) -> Vec<PathBuf> {
    let name = toolbox_file_name();
    vec![
        exe_dir.join(name),
        exe_dir.join("resources").join(name),
        exe_dir.join("resources").join("toolbox").join(name),
        // macOS bundle: Contents/MacOS/../Resources
        exe_dir.join("..").join("Resources").join(name),
        exe_dir.join("..").join("Resources").join("toolbox").join(name),
        // Linux packages: /usr/lib/<app>/ next to /usr/bin/<app>
        exe_dir.join("..").join("lib").join("plugable-chat").join(name),
    ]
}

/// Bundled-toolbox candidate locations relative to the current executable.
pub fn bundled_toolbox_candidate_paths() -> Vec<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(bundled_toolbox_candidate_paths_for))
        .unwrap_or_default()
}

/// Lowercase hex SHA-256 of a file, read in chunks so a 200+ MB binary is not held in memory.
pub async fn sha256_hex_of_file(path: &Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Progress payload for the `toolbox-install-progress` event.
#[derive(Debug, Clone, Serialize)]
pub struct ToolboxInstallProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub is_complete: bool,
    pub error: Option<String>,
}

/// Download `url` to `destination`, verifying its SHA-256 against `expected_sha256_hex`.
///
/// The bytes go to `<destination>.download` and are hashed while streaming. Only a file whose hash
/// matches is renamed into place (and marked executable on Unix), so a truncated or tampered
/// download never ends up at a path `find_toolbox_binary()` would return.
pub async fn download_toolbox_verified(
    url: &str,
    expected_sha256_hex: &str,
    destination: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let partial_path = destination.with_extension("download");

    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Toolbox download failed: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("Toolbox download failed: HTTP {}", response.status()));
    }
    let total_bytes = response.content_length().unwrap_or(0);

    let mut file = tokio::fs::File::create(&partial_path)
        .await
        .map_err(|e| format!("Failed to create {}: {}", partial_path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut downloaded_bytes: u64 = 0;
    let mut response = response;

    let stream_result: Result<(), String> = async {
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("Toolbox download interrupted: {}", e))?
        {
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Failed to write {}: {}", partial_path.display(), e))?;
            downloaded_bytes += chunk.len() as u64;
            on_progress(downloaded_bytes, total_bytes);
        }
        file.flush()
            .await
            .map_err(|e| format!("Failed to flush {}: {}", partial_path.display(), e))
    }
    .await;
    drop(file);

    if let Err(e) = stream_result {
        let _ = tokio::fs::remove_file(&partial_path).await;
        return Err(e);
    }

    let actual_sha256_hex = hex_lower(&hasher.finalize());
    if !actual_sha256_hex.eq_ignore_ascii_case(expected_sha256_hex) {
        let _ = tokio::fs::remove_file(&partial_path).await;
        return Err(format!(
            "Toolbox checksum mismatch (expected {}, got {}); the download was discarded.",
            expected_sha256_hex, actual_sha256_hex
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&partial_path, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| format!("Failed to mark toolbox executable: {}", e))?;
    }

    // Windows cannot rename over an existing file
    let _ = tokio::fs::remove_file(destination).await;
    tokio::fs::rename(&partial_path, destination)
        .await
        .map_err(|e| format!("Failed to move toolbox into place: {}", e))?;
    Ok(destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// Serve `body` once from an ephemeral local port and return the URL.
    async fn serve_body_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let toolbox_test_server_addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request_buffer = [0u8; 2048];
            let _ = socket.read(&mut request_buffer).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        });
        format!("http://{}/toolbox", toolbox_test_server_addr)
    }

    fn sha256_hex_of_bytes(bytes: &[u8]) -> String {
        hex_lower(&Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn download_installs_file_when_checksum_matches() {
        let body = b"pretend toolbox binary".to_vec();
        let url = serve_body_once(body.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("tools").join("toolbox");

        let installed = download_toolbox_verified(&url, &sha256_hex_of_bytes(&body), &destination, |_, _| {})
            .await
            .unwrap();

        assert_eq!(installed, destination);
        assert_eq!(std::fs::read(&destination).unwrap(), body);
        assert!(!destination.with_extension("download").exists());
        assert_eq!(
            sha256_hex_of_file(&destination).await.unwrap(),
            sha256_hex_of_bytes(&body)
        );
    }

    #[tokio::test]
    async fn download_rejects_and_discards_file_on_checksum_mismatch() {
        let url = serve_body_once(b"tampered".to_vec()).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("toolbox");

        let err = download_toolbox_verified(&url, &"0".repeat(64), &destination, |_, _| {})
            .await
            .unwrap_err();

        assert!(err.contains("checksum mismatch"), "unexpected error: {err}");
        assert!(!destination.exists());
        assert!(!destination.with_extension("download").exists());
    }

    #[test]
    fn bundled_candidates_cover_resources_next_to_the_app() {
        let exe_dir = Path::new("app").join("bin");
        let candidates = bundled_toolbox_candidate_paths_for(&exe_dir);
        let name = toolbox_file_name();
        assert!(candidates.contains(&exe_dir.join(name)));
        assert!(candidates.contains(&exe_dir.join("resources").join(name)));
        assert!(candidates.contains(&exe_dir.join("..").join("Resources").join(name)));
    }

    #[test]
    fn pinned_specs_are_well_formed() {
        if let Some(spec) = toolbox_download_spec() {
            assert!(spec.url.contains(&format!("/v{}/", TOOLBOX_VERSION)));
            assert_eq!(spec.sha256_hex.len(), 64);
            assert!(spec.sha256_hex.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(spec.size_bytes > 0);
        }
    }
}
