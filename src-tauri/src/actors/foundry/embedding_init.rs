//! CPU embedding model start-up, shared by the first-run path and the `retry_embedding_init`
//! command.
//!
//! The model (BGE-Base-EN-v1.5, a few hundred MB) is downloaded from Hugging Face on first run.
//! hf-hub honours `HTTPS_PROXY` / `ALL_PROXY` and `HF_ENDPOINT` from the environment, so a
//! corporate proxy needs no extra code here.

use crate::crash_handler::SuppressCrashDialogGuard;
use crate::test_state::{self, Phase};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::RwLock;

pub type SharedEmbeddingModel = Arc<RwLock<Option<Arc<TextEmbedding>>>>;

const ATTEMPTS: u32 = 3;
const BASE_DELAY: Duration = Duration::from_secs(2);

static INIT_RUNNING: AtomicBool = AtomicBool::new(false);

/// Run `f` up to `attempts` times, sleeping `base << attempt` between failures (never after
/// the last one). Blocking: call from `spawn_blocking`.
pub fn retry_blocking<T, E: std::fmt::Debug>(
    attempts: u32,
    base: Duration,
    mut f: impl FnMut(u32) -> Result<T, E>,
) -> Result<T, E> {
    let mut attempt = 0;
    loop {
        match f(attempt) {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if attempt >= attempts {
                    return Err(e);
                }
                println!("FoundryActor: embedding init attempt {attempt} failed: {e:?}");
                std::thread::sleep(base * (1u32 << (attempt - 1)));
            }
        }
    }
}

/// Delete leftovers of an interrupted download (partial blobs, lock files, empty files) so a
/// retry starts clean instead of tripping over them. Returns how many files were removed.
pub fn clear_stale_download_files(root: &std::path::Path) -> usize {
    fn walk(dir: &std::path::Path, removed: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.is_dir() {
                walk(&path, removed);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
            let stale = name.ends_with(".lock")
                || name.ends_with(".part")
                || name.ends_with(".incomplete")
                || (meta.is_file() && meta.len() == 0);
            if stale && std::fs::remove_file(&path).is_ok() {
                *removed += 1;
            }
        }
    }
    let mut removed = 0;
    walk(root, &mut removed);
    removed
}

/// Turn the raw download/initialisation error into a message that names the likely cause, then
/// keeps the original text for support. The raw text alone ("Failed to retrieve
/// onnx/model.onnx") does not say whether the network, the disk or a proxy is at fault.
pub fn describe_embedding_error(raw: &str) -> String {
    let l = raw.to_ascii_lowercase();
    let cause = if l.contains("os error 112") || l.contains("no space left") || l.contains("not enough space") || l.contains("disk full") {
        "There is not enough free disk space to download the search model. Free some space and try again."
    } else if l.contains("certificate") || l.contains("tls") || l.contains("ssl") {
        "A security certificate problem blocked the download (a proxy or firewall may be inspecting traffic)."
    } else if l.contains("proxy") {
        "The configured proxy could not be reached. Check the HTTPS_PROXY setting or your network."
    } else if l.contains("dns") || l.contains("failed to lookup") || l.contains("resolve") || l.contains("os error 11001") || l.contains("no such host") {
        "Could not reach huggingface.co: the name did not resolve. Check your internet connection or firewall."
    } else if l.contains("connection refused") || l.contains("os error 10061") || l.contains("timed out") || l.contains("os error 10060")
        || l.contains("connection reset") || l.contains("network") || l.contains("unreachable") || l.contains("connect")
    {
        "Could not connect to huggingface.co to download the search model. Check your internet connection or firewall."
    } else if l.contains("permission denied") || l.contains("access is denied") || l.contains("os error 5") || l.contains("read-only") {
        "Plugable Chat could not write the search model to its data folder (access denied)."
    } else {
        "The search model could not be downloaded or loaded."
    };
    format!("{cause} Details: {raw}")
}

fn emit_progress(app: &AppHandle, message: &str, is_complete: bool, error: bool) {
    let mut payload = json!({ "message": message, "is_complete": is_complete });
    if error {
        payload["error"] = json!(true);
    }
    let _ = app.emit("embedding-init-progress", payload);
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Unknown panic".to_string()
    }
}

/// Load the CPU embedding model into `shared`. Returns the user-facing error on failure.
///
/// A second concurrent call returns an error instead of starting a second download. Success
/// also fills in embeddings for chats that were saved while the model was unavailable.
pub async fn init_cpu_embedding_model(app: AppHandle, shared: SharedEmbeddingModel) -> Result<(), String> {
    if shared.read().await.is_some() {
        return Ok(());
    }
    if INIT_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("The embedding model is already loading.".to_string());
    }
    let result = init_inner(&app, &shared).await;
    INIT_RUNNING.store(false, Ordering::SeqCst);
    result
}

async fn init_inner(app: &AppHandle, shared: &SharedEmbeddingModel) -> Result<(), String> {
    emit_progress(app, "Initializing CPU embedding model...", false, false);
    test_state::record(Phase::Embedding, test_state::status::STARTED, "initializing");

    // fastembed defaults to a cwd-relative `.fastembed_cache`; for a per-machine install cwd is
    // Program Files, which a standard user cannot write, so the model download fails with
    // "Failed to retrieve onnx/model.onnx". Pin it to a writable per-user dir.
    let cache = crate::paths::ensure_writable_dir(crate::paths::get_embedding_cache_dir(), "fastembed").await;
    let cache_path = cache.path.clone();
    // hf-hub reads HF_HOME; keep any other hub use inside the same writable tree.
    std::env::set_var("HF_HOME", cache_path.join("hf"));

    // catch_unwind: ORT initialisation panics when a native DLL is missing, and embedding is
    // optional, so that must become an error message rather than a crash.
    let joined = tokio::task::spawn_blocking(move || {
        let _guard = SuppressCrashDialogGuard::new();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retry_blocking(ATTEMPTS, BASE_DELAY, |attempt| {
                if attempt > 0 {
                    let n = clear_stale_download_files(&cache_path);
                    println!("FoundryActor: cleared {n} stale download file(s) before retrying the embedding model");
                }
                let options = InitOptions::new(EmbeddingModel::BGEBaseENV15)
                    .with_cache_dir(cache_path.clone())
                    .with_show_download_progress(true);
                TextEmbedding::try_new(options)
            })
        }))
    })
    .await;

    let failure = match joined {
        Ok(Ok(Ok(model))) => {
            println!("FoundryActor: CPU embedding model loaded successfully");
            *shared.write().await = Some(Arc::new(model));
            emit_progress(app, "CPU embedding model loaded (GPU model loads on-demand)", true, false);
            test_state::record(Phase::Embedding, test_state::status::OK, "loaded");
            if let Some(handles) = app.try_state::<crate::app_state::ActorHandles>() {
                let vector_tx = handles.vector_tx.clone();
                let model = Arc::clone(shared);
                tauri::async_runtime::spawn(async move {
                    crate::chat_persistence::backfill_embeddings(&vector_tx, &model).await;
                });
            }
            return Ok(());
        }
        Ok(Ok(Err(e))) => {
            println!("FoundryActor ERROR: Failed to load CPU embedding model: {e:?}");
            // `{e:#}` includes the whole cause chain, which is where the real reason lives.
            format!("Failed to load CPU embedding model. {}", describe_embedding_error(&format!("{e:#}")))
        }
        Ok(Err(panic_payload)) => {
            let msg = panic_message(panic_payload.as_ref());
            println!("FoundryActor ERROR: ONNX Runtime initialization panicked: {msg}");
            format!("ONNX Runtime unavailable: {msg}. Embedding features disabled.")
        }
        Err(e) => {
            println!("FoundryActor ERROR: CPU embedding model init task failed: {e:?}");
            "CPU embedding model initialization task failed".to_string()
        }
    };
    emit_progress(app, &failure, true, true);
    test_state::record(Phase::Embedding, test_state::status::FAILED, &failure);
    Err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_until_success() {
        let mut calls = 0;
        let r: Result<u32, String> = retry_blocking(3, Duration::from_millis(1), |_| {
            calls += 1;
            if calls < 3 { Err(format!("fail {calls}")) } else { Ok(calls) }
        });
        assert_eq!(r, Ok(3));
    }

    #[test]
    fn gives_up_after_the_attempt_limit_with_the_last_error_and_without_a_final_sleep() {
        let mut calls = 0;
        let started = std::time::Instant::now();
        let r: Result<(), String> = retry_blocking(3, Duration::from_millis(40), |_| {
            calls += 1;
            Err(format!("fail {calls}"))
        });
        assert_eq!(r, Err("fail 3".to_string()));
        assert_eq!(calls, 3);
        // sleeps are 40ms + 80ms; a trailing 160ms sleep would push this past 270ms
        assert!(started.elapsed() < Duration::from_millis(270));
    }

    #[test]
    fn one_attempt_never_sleeps() {
        let r: Result<(), &str> = retry_blocking(1, Duration::from_secs(60), |_| Err("x"));
        assert_eq!(r, Err("x"));
    }

    #[test]
    fn errors_name_their_cause() {
        let d = |s: &str| describe_embedding_error(s);
        assert!(d("Failed to retrieve onnx/model.onnx: dns error: failed to lookup address").contains("did not resolve"));
        assert!(d("request error: Connection refused (os error 10061)").contains("Could not connect"));
        assert!(d("io error: There is not enough space on the disk. (os error 112)").contains("free disk space"));
        assert!(d("Access is denied. (os error 5)").contains("access denied"));
        assert!(d("invalid peer certificate: UnknownIssuer").contains("certificate"));
        assert!(d("proxy connect failed").contains("proxy"));
        // always keeps the raw text for support
        assert!(d("something odd").ends_with("Details: something odd"));
    }

    #[test]
    fn stale_download_files_are_removed_and_real_files_kept() {
        let dir = std::env::temp_dir().join(format!("pc-stale-{}", std::process::id()));
        let blobs = dir.join("models--x").join("blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::write(blobs.join("abc.lock"), b"x").unwrap();
        std::fs::write(blobs.join("def.sync.part"), b"partial").unwrap();
        std::fs::write(blobs.join("empty"), b"").unwrap();
        std::fs::write(blobs.join("good"), b"model bytes").unwrap();
        assert_eq!(clear_stale_download_files(&dir), 3);
        assert!(blobs.join("good").exists());
        assert!(!blobs.join("abc.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panic_payloads_become_text() {
        let p = std::panic::catch_unwind(|| panic!("boom")).unwrap_err();
        assert_eq!(panic_message(p.as_ref()), "boom");
    }
}
