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

/// How patient the downloader is. The defaults suit a 100-230 MB file over a home or
/// corporate link; tests use small values.
#[derive(Debug, Clone)]
pub struct DownloadPolicy {
    /// Attempts in total (the first try plus retries). Interrupted attempts resume.
    pub max_attempts: u32,
    /// Wait before the second attempt; doubles each time.
    pub base_delay: std::time::Duration,
    pub connect_timeout: std::time::Duration,
    /// Longest silence tolerated mid-download before the attempt counts as failed.
    pub read_timeout: std::time::Duration,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: std::time::Duration::from_secs(2),
            connect_timeout: std::time::Duration::from_secs(30),
            read_timeout: std::time::Duration::from_secs(60),
        }
    }
}

/// HTTP client with timeouts. Proxies come from the environment (`HTTPS_PROXY`, `HTTP_PROXY`,
/// `ALL_PROXY`, `NO_PROXY`) and, on Windows, the system proxy settings: reqwest's default
/// behaviour, so a corporate proxy needs no configuration here. TLS uses the operating
/// system's certificate store, so a proxy's company root certificate is trusted too.
fn build_http_client(policy: &DownloadPolicy) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(policy.connect_timeout)
        .read_timeout(policy.read_timeout)
        .build()
        .map_err(|e| format!("Could not set up the download connection: {}", e))
}

/// Outcome of one download attempt that did not complete.
enum AttemptError {
    /// Worth another try (network drop, timeout, server busy); the partial file is kept so the
    /// next attempt resumes from it.
    Retryable(String),
    /// Retrying cannot help (not found, disk error).
    Fatal(String),
}

fn status_is_retryable(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// One pass: request the bytes after what is already in `partial_path` and append them.
/// `Ok(())` means the server had nothing more to send; the caller verifies the checksum.
async fn download_attempt(
    client: &reqwest::Client,
    url: &str,
    partial_path: &Path,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), AttemptError> {
    let existing = tokio::fs::metadata(partial_path).await.map(|m| m.len()).unwrap_or(0);
    let mut request = client.get(url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={}-", existing));
    }
    let mut response = request
        .send()
        .await
        .map_err(|e| AttemptError::Retryable(format!("Toolbox download failed: {}", e)))?;

    let status = response.status();
    let (resume_from, total_bytes) = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        (existing, existing + response.content_length().unwrap_or(0))
    } else if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The partial file already holds everything the server has (or is longer than it);
        // the checksum decides which.
        return Ok(());
    } else if status.is_success() {
        // A plain 200 means the server ignored the range: start over.
        (0, response.content_length().unwrap_or(0))
    } else if status_is_retryable(status) {
        return Err(AttemptError::Retryable(format!("Toolbox download failed: HTTP {}", status)));
    } else {
        return Err(AttemptError::Fatal(format!("Toolbox download failed: HTTP {}", status)));
    };

    let mut file = if resume_from > 0 {
        tokio::fs::OpenOptions::new().append(true).open(partial_path).await
    } else {
        tokio::fs::File::create(partial_path).await
    }
    .map_err(|e| AttemptError::Fatal(format!("Failed to open {}: {}", partial_path.display(), e)))?;

    let mut downloaded_bytes = resume_from;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                file.write_all(&chunk).await.map_err(|e| {
                    AttemptError::Fatal(format!("Failed to write {}: {}", partial_path.display(), e))
                })?;
                downloaded_bytes += chunk.len() as u64;
                on_progress(downloaded_bytes, total_bytes);
            }
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush().await;
                return Err(AttemptError::Retryable(format!("Toolbox download interrupted: {}", e)));
            }
        }
    }
    file.flush()
        .await
        .map_err(|e| AttemptError::Fatal(format!("Failed to flush {}: {}", partial_path.display(), e)))
}

/// Download `url` to `destination`, verifying its SHA-256 against `expected_sha256_hex`.
///
/// Uses the default [`DownloadPolicy`]; see [`download_toolbox_with_policy`].
pub async fn download_toolbox_verified(
    url: &str,
    expected_sha256_hex: &str,
    destination: &Path,
    on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    download_toolbox_with_policy(url, expected_sha256_hex, destination, &DownloadPolicy::default(), on_progress).await
}

/// Download with timeouts, resume and retry, then verify the checksum.
///
/// Bytes go to `<destination>.download`. An interrupted attempt keeps what it has and the next
/// one asks the server only for the rest (HTTP Range), including after an app restart. When
/// all bytes are in, the file is hashed; a mismatch discards it and starts over from zero (a
/// corrupt resume must not loop forever). Only a file whose hash matches is renamed into place
/// (and marked executable on Unix), so a truncated or tampered download never ends up at a
/// path `find_toolbox_binary()` would return.
pub async fn download_toolbox_with_policy(
    url: &str,
    expected_sha256_hex: &str,
    destination: &Path,
    policy: &DownloadPolicy,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let partial_path = destination.with_extension("download");
    let client = build_http_client(policy)?;

    let attempts = policy.max_attempts.max(1);
    let mut last_error = String::from("Toolbox download failed");
    for attempt in 1..=attempts {
        if attempt > 1 {
            let delay = policy.base_delay * (1u32 << (attempt - 2).min(6));
            println!("[ToolboxInstall] retrying in {:?} (attempt {}/{}): {}", delay, attempt, attempts, last_error);
            tokio::time::sleep(delay).await;
        }
        match download_attempt(&client, url, &partial_path, &mut on_progress).await {
            Ok(()) => {
                let actual_sha256_hex = sha256_hex_of_file(&partial_path).await?;
                if actual_sha256_hex.eq_ignore_ascii_case(expected_sha256_hex) {
                    return install_verified_file(&partial_path, destination).await;
                }
                let _ = tokio::fs::remove_file(&partial_path).await;
                last_error = format!(
                    "Toolbox checksum mismatch (expected {}, got {}); the download was discarded.",
                    expected_sha256_hex, actual_sha256_hex
                );
            }
            Err(AttemptError::Retryable(e)) => last_error = e,
            Err(AttemptError::Fatal(e)) => return Err(e),
        }
    }
    Err(last_error)
}

async fn install_verified_file(partial_path: &Path, destination: &Path) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(partial_path, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| format!("Failed to mark toolbox executable: {}", e))?;
    }

    // Windows cannot rename over an existing file
    let _ = tokio::fs::remove_file(destination).await;
    tokio::fs::rename(partial_path, destination)
        .await
        .map_err(|e| format!("Failed to move toolbox into place: {}", e))?;
    Ok(destination.to_path_buf())
}

// ---------------------------------------------------------------------------
// Automatic install
// ---------------------------------------------------------------------------

/// True when an enabled source is served by the toolbox binary the app manages (the embedded
/// demo database). Other sources bring their own command.
pub fn needs_app_managed_toolbox(config: &crate::settings::DatabaseToolboxConfig) -> bool {
    config
        .sources
        .iter()
        .any(|s| s.enabled && crate::settings::is_embedded_demo_source(&s.id))
}

/// Download progress as the `toolbox-download-progress` event payload.
pub fn download_progress_payload(file: &str, downloaded: u64, total: u64) -> serde_json::Value {
    let percent = if total == 0 { 0 } else { (downloaded.saturating_mul(100) / total).min(100) };
    serde_json::json!({ "percent": percent, "file": file })
}

fn download_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}

static DOWNLOAD_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn last_download_error_cell() -> &'static std::sync::Mutex<Option<String>> {
    static CELL: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
    CELL.get_or_init(Default::default)
}

/// Why the most recent automatic download failed, if it did and nothing has succeeded since.
pub fn last_download_error() -> Option<String> {
    last_download_error_cell().lock().ok().and_then(|g| g.clone())
}

pub fn download_in_progress() -> bool {
    DOWNLOAD_RUNNING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Download the pinned toolbox into the app data directory, emitting `toolbox-download-progress`
/// (`{percent, file}`) and the older `toolbox-install-progress` the Databases tab listens to.
///
/// Concurrent callers queue behind one another. With `skip_if_present`, a caller that was
/// waiting returns the toolbox the first caller installed instead of downloading it again.
pub async fn install_pinned_toolbox(app: &tauri::AppHandle, skip_if_present: bool) -> Result<PathBuf, String> {
    use crate::test_state::{record, status, Phase};
    use tauri::Emitter;

    let spec = toolbox_download_spec().ok_or_else(|| {
        "No toolbox download is available for this platform; install it manually from https://github.com/googleapis/genai-toolbox".to_string()
    })?;
    let _guard = download_lock().lock().await;
    if skip_if_present {
        if let Some(found) = crate::settings::find_toolbox_binary() {
            return Ok(PathBuf::from(found));
        }
    }
    DOWNLOAD_RUNNING.store(true, std::sync::atomic::Ordering::SeqCst);
    record(Phase::Toolbox, status::STARTED, spec.file_name);
    let _ = app.emit("toolbox-download-progress", download_progress_payload(spec.file_name, 0, spec.size_bytes));

    let progress_app = app.clone();
    let mut last_percent = u64::MAX;
    let result = download_toolbox_verified(
        spec.url,
        spec.sha256_hex,
        &installed_toolbox_path(),
        move |downloaded_bytes, total_bytes| {
            let total = if total_bytes > 0 { total_bytes } else { spec.size_bytes };
            let percent = downloaded_bytes * 100 / total.max(1);
            if percent != last_percent {
                last_percent = percent;
                let _ = progress_app.emit(
                    "toolbox-download-progress",
                    download_progress_payload(spec.file_name, downloaded_bytes, total),
                );
                let _ = progress_app.emit(
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
    let _ = app.emit(
        "toolbox-install-progress",
        ToolboxInstallProgress {
            downloaded_bytes: spec.size_bytes,
            total_bytes: spec.size_bytes,
            is_complete,
            error: error.clone(),
        },
    );
    match &result {
        Ok(_) => {
            let _ = app.emit("toolbox-download-progress", download_progress_payload(spec.file_name, spec.size_bytes, spec.size_bytes));
            record(Phase::Toolbox, status::OK, spec.file_name);
            if let Ok(mut cell) = last_download_error_cell().lock() {
                *cell = None;
            }
        }
        Err(e) => {
            let _ = app.emit(
                "toolbox-download-progress",
                serde_json::json!({ "percent": 0, "file": spec.file_name, "error": e }),
            );
            record(Phase::Toolbox, status::FAILED, e);
            if let Ok(mut cell) = last_download_error_cell().lock() {
                *cell = Some(e.clone());
            }
        }
    }
    DOWNLOAD_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    result
}

/// The toolbox path if one exists, otherwise download the pinned build.
pub async fn ensure_toolbox_installed(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(found) = crate::settings::find_toolbox_binary() {
        return Ok(PathBuf::from(found));
    }
    install_pinned_toolbox(app, true).await
}

/// Start the download in the background when enabled sources need the toolbox and none exists.
/// Returns whether a download was started.
pub fn spawn_download_if_needed(app: &tauri::AppHandle, config: &crate::settings::DatabaseToolboxConfig) -> bool {
    if !needs_app_managed_toolbox(config)
        || toolbox_download_spec().is_none()
        || download_in_progress()
        || crate::settings::find_toolbox_binary().is_some()
    {
        return false;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = ensure_toolbox_installed(&app).await {
            println!("[ToolboxInstall] automatic download failed: {e}");
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// What the test server does for each successive connection.
    #[derive(Clone)]
    enum Reply {
        /// Full body, 200.
        Full,
        /// Advertise the full length but close the socket after this many body bytes.
        CutAfter(usize),
        /// Honour the Range header with 206.
        Range,
        Status(u16),
    }

    /// Serve `body`, answering connection N with `script[N]` (the last entry repeats).
    /// Returns the URL and the request heads seen, in order.
    async fn serve(body: Vec<u8>, script: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_server = seen.clone();
        tokio::spawn(async move {
            let mut n = 0usize;
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let reply = script[n.min(script.len() - 1)].clone();
                n += 1;
                let mut buf = [0u8; 4096];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..read]).to_string();
                seen_by_server.lock().unwrap().push(head.clone());
                let range_start = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|r| r.trim_end_matches('-').trim().to_string()))
                    .and_then(|r| r.parse::<usize>().ok());
                match reply {
                    Reply::Full | Reply::CutAfter(_) => {
                        let cut = if let Reply::CutAfter(c) = reply { c.min(body.len()) } else { body.len() };
                        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = socket.write_all(header.as_bytes()).await;
                        let _ = socket.write_all(&body[..cut]).await;
                    }
                    Reply::Range => {
                        let start = range_start.unwrap_or(0);
                        let rest = &body[start..];
                        let header = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            rest.len(), start, body.len() - 1, body.len()
                        );
                        let _ = socket.write_all(header.as_bytes()).await;
                        let _ = socket.write_all(rest).await;
                    }
                    Reply::Status(code) => {
                        let header = format!("HTTP/1.1 {} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", code);
                        let _ = socket.write_all(header.as_bytes()).await;
                    }
                }
                let _ = socket.shutdown().await;
            }
        });
        (format!("http://{}/toolbox", addr), seen)
    }

    fn fast_policy(max_attempts: u32) -> DownloadPolicy {
        DownloadPolicy {
            max_attempts,
            base_delay: std::time::Duration::from_millis(5),
            connect_timeout: std::time::Duration::from_secs(5),
            read_timeout: std::time::Duration::from_secs(5),
        }
    }

    fn sha256_hex_of_bytes(bytes: &[u8]) -> String {
        hex_lower(&Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn download_installs_file_when_checksum_matches() {
        let body = b"pretend toolbox binary".to_vec();
        let (url, _) = serve(body.clone(), vec![Reply::Full]).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("tools").join("toolbox");

        let installed = download_toolbox_with_policy(&url, &sha256_hex_of_bytes(&body), &destination, &fast_policy(1), |_, _| {})
            .await
            .unwrap();

        assert_eq!(installed, destination);
        assert_eq!(std::fs::read(&destination).unwrap(), body);
        assert!(!destination.with_extension("download").exists());
        assert_eq!(sha256_hex_of_file(&destination).await.unwrap(), sha256_hex_of_bytes(&body));
    }

    #[tokio::test]
    async fn download_rejects_and_discards_file_on_checksum_mismatch() {
        let (url, _) = serve(b"tampered".to_vec(), vec![Reply::Full]).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("toolbox");

        let err = download_toolbox_with_policy(&url, &"0".repeat(64), &destination, &fast_policy(1), |_, _| {})
            .await
            .unwrap_err();

        assert!(err.contains("checksum mismatch"), "unexpected error: {err}");
        assert!(!destination.exists());
        assert!(!destination.with_extension("download").exists());
    }

    #[tokio::test]
    async fn an_interrupted_download_resumes_with_a_range_request() {
        let body: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let (url, seen) = serve(body.clone(), vec![Reply::CutAfter(1200), Reply::Range]).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("toolbox");
        let mut last_progress = (0, 0);

        download_toolbox_with_policy(&url, &sha256_hex_of_bytes(&body), &destination, &fast_policy(3), |d, t| last_progress = (d, t))
            .await
            .unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), body);
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].to_ascii_lowercase().contains("range:"));
        assert!(requests[1].to_ascii_lowercase().contains("range: bytes=1200-"), "{}", requests[1]);
        assert_eq!(last_progress, (5000, 5000));
    }

    #[tokio::test]
    async fn a_busy_server_is_retried() {
        let body = b"the real toolbox bytes".to_vec();
        let (url, seen) = serve(body.clone(), vec![Reply::Status(503), Reply::Full]).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("toolbox");
        download_toolbox_with_policy(&url, &sha256_hex_of_bytes(&body), &destination, &fast_policy(3), |_, _| {})
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(std::fs::read(&destination).unwrap(), body);
    }

    #[tokio::test]
    async fn a_server_that_ignores_range_restarts_the_file() {
        let body = b"0123456789abcdef".to_vec();
        // The server ignores Range (plain 200), so a stale partial must be replaced, not appended to.
        let (url, _) = serve(body.clone(), vec![Reply::Full]).await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("toolbox");
        std::fs::write(destination.with_extension("download"), b"garbage that is not a prefix").unwrap();

        download_toolbox_with_policy(&url, &sha256_hex_of_bytes(&body), &destination, &fast_policy(2), |_, _| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), body);
    }

    #[tokio::test]
    async fn not_found_fails_at_once_without_retrying() {
        let (url, seen) = serve(Vec::new(), vec![Reply::Status(404)]).await;
        let dir = tempfile::tempdir().unwrap();
        let err = download_toolbox_with_policy(&url, "00", &dir.path().join("toolbox"), &fast_policy(4), |_, _| {})
            .await
            .unwrap_err();
        assert!(err.contains("404"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn gives_up_after_the_attempt_limit_on_server_errors() {
        let (url, seen) = serve(Vec::new(), vec![Reply::Status(503)]).await;
        let dir = tempfile::tempdir().unwrap();
        let err = download_toolbox_with_policy(&url, "00", &dir.path().join("toolbox"), &fast_policy(3), |_, _| {})
            .await
            .unwrap_err();
        assert!(err.contains("503"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn an_unreachable_server_is_an_error_not_a_hang() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let err = download_toolbox_with_policy(&format!("http://{addr}/x"), "00", &dir.path().join("t"), &fast_policy(2), |_, _| {})
            .await
            .unwrap_err();
        assert!(err.contains("Toolbox download failed"), "{err}");
    }

    #[test]
    fn progress_payload_is_percent_and_file() {
        assert_eq!(download_progress_payload("toolbox.exe", 50, 200), serde_json::json!({"percent": 25, "file": "toolbox.exe"}));
        assert_eq!(download_progress_payload("t", 5, 0)["percent"], 0);
        assert_eq!(download_progress_payload("t", 500, 200)["percent"], 100);
    }

    #[test]
    fn only_an_enabled_demo_source_needs_the_app_toolbox() {
        use crate::settings::{default_demo_database_source, DatabaseToolboxConfig};
        let mut demo = default_demo_database_source();
        let off = DatabaseToolboxConfig { enabled: true, sources: vec![demo.clone()] };
        assert!(!needs_app_managed_toolbox(&off));
        demo.enabled = true;
        let on = DatabaseToolboxConfig { enabled: true, sources: vec![demo] };
        assert!(needs_app_managed_toolbox(&on));
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
