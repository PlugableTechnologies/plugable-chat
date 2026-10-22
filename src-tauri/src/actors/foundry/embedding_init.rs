//! CPU embedding model start-up, shared by the first-run path and the `retry_embedding_init`
//! command.
//!
//! The model (BGE-Base-EN-v1.5, a few hundred MB) is downloaded from Hugging Face on first run.
//! hf-hub honours `HTTPS_PROXY` / `ALL_PROXY` and `HF_ENDPOINT` from the environment, so a
//! corporate proxy needs no extra code here.

use crate::crash_handler::SuppressCrashDialogGuard;
use crate::actors::startup_actor::StartupMsg;
use crate::protocol::ResourceStatus;
use crate::test_state::{self, Phase};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::RwLock;

pub type SharedEmbeddingModel = Arc<RwLock<Option<Arc<TextEmbedding>>>>;

/// Five tries with 3, 6, 12 and 24 second waits (45 s in all). Three tries over six seconds was
/// not enough on a first start, when the search model competes with the chat-model download and
/// the GPU component download for the same connection (Bob's red row on first launch only).
const ATTEMPTS: u32 = 5;
const BASE_DELAY: Duration = Duration::from_secs(3);
const MAX_DELAY: Duration = Duration::from_secs(30);

static INIT_RUNNING: AtomicBool = AtomicBool::new(false);

/// Why every attempt failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptFailure {
    pub attempts: u32,
    pub last_error: String,
    /// At least one attempt panicked (a native library such as ONNX Runtime failed to load).
    pub panicked: bool,
}

/// Run `f` up to `attempts` times, sleeping `base << n` (capped) between failures and never after
/// the last one. A panic inside one attempt counts as that attempt failing: it used to end the
/// whole retry loop on the first try, so a library that was still being unpacked or scanned by
/// antivirus could never be waited out. Blocking: call from `spawn_blocking`.
pub fn run_attempts<T>(
    attempts: u32,
    base: Duration,
    mut f: impl FnMut(u32) -> Result<T, String>,
) -> Result<T, AttemptFailure> {
    let mut panicked = false;
    let mut last_error = String::new();
    for attempt in 0..attempts.max(1) {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(attempt))) {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(e)) => last_error = e,
            Err(payload) => {
                panicked = true;
                last_error = format!("a native library failed while loading: {}", panic_message(payload.as_ref()));
            }
        }
        crate::app_log::log("embedding", &format!("attempt {} of {} failed: {}", attempt + 1, attempts, last_error));
        if attempt + 1 < attempts {
            std::thread::sleep((base * (1u32 << attempt.min(8))).min(MAX_DELAY));
        }
    }
    Err(AttemptFailure { attempts: attempts.max(1), last_error, panicked })
}

/// A model that loads can still be unusable (truncated file, wrong shape). Reject output that
/// could not be a real embedding.
pub fn validate_embeddings(out: &[Vec<f32>]) -> Result<(), String> {
    let first = out.first().ok_or("the model returned no embedding for the test sentence")?;
    if first.is_empty() {
        return Err("the model returned an empty embedding".to_string());
    }
    if first.iter().any(|v| !v.is_finite()) {
        return Err("the model returned non-numeric values".to_string());
    }
    if first.iter().all(|v| *v == 0.0) {
        return Err("the model returned an all-zero embedding".to_string());
    }
    Ok(())
}

/// Remove everything inside `root` (not `root` itself) so a corrupt model is downloaded afresh.
pub fn clear_model_cache(root: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else { return 0 };
    let mut removed = 0;
    for e in entries.flatten() {
        let path = e.path();
        let ok = if path.is_dir() { std::fs::remove_dir_all(&path).is_ok() } else { std::fs::remove_file(&path).is_ok() };
        if ok {
            removed += 1;
        }
    }
    removed
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
        // Say so: a Retry that returns silently leaves its row on "Retrying..." for ever.
        emit_progress(&app, ALREADY_LOADED_MESSAGE, true, false);
        report_status(&app, ResourceStatus::Ready).await;
        return Ok(());
    }
    if INIT_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("The search model is still loading. Wait for it to finish.".to_string());
    }
    report_status(&app, ResourceStatus::Initializing).await;
    let result = init_inner(&app, &shared).await;
    INIT_RUNNING.store(false, Ordering::SeqCst);
    report_status(
        &app,
        match &result {
            Ok(()) => ResourceStatus::Ready,
            Err(message) => ResourceStatus::Failed { message: message.clone() },
        },
    )
    .await;
    result
}

pub const ALREADY_LOADED_MESSAGE: &str = "CPU embedding model loaded (GPU model loads on-demand)";

/// Keep the start-up coordinator's view of the embedding model true (it stayed `pending` for ever).
async fn report_status(app: &AppHandle, status: ResourceStatus) {
    if let Some(handles) = app.try_state::<crate::app_state::ActorHandles>() {
        let _ = handles.startup_tx.send(StartupMsg::ReportEmbeddingStatus { status }).await;
    }
}

/// The user-facing text after every attempt failed. Always starts with "Failed to load CPU
/// embedding model." (the UI and the test scripts key on it) and says how many tries were made.
pub fn format_failure(f: &AttemptFailure) -> String {
    let described = describe_embedding_error(&f.last_error);
    let lib = if f.panicked { " A native library failed to load; if this persists, reinstall the Visual C++ runtime." } else { "" };
    format!("Failed to load CPU embedding model after {} attempts. {described}{lib}", f.attempts)
}

async fn init_inner(app: &AppHandle, shared: &SharedEmbeddingModel) -> Result<(), String> {
    emit_progress(app, "Initializing CPU embedding model...", false, false);
    test_state::record(Phase::Embedding, test_state::status::STARTED, "initializing");

    // fastembed defaults to a cwd-relative `.fastembed_cache`; for a per-machine install cwd is
    // Program Files, which a standard user cannot write, so the model download fails with
    // "Failed to retrieve onnx/model.onnx". Pin it to a writable per-user dir.
    let cache = crate::paths::ensure_writable_dir(crate::paths::get_embedding_cache_dir(), "fastembed").await;
    let cache_path = cache.path.clone();
    // hf-hub reads HF_HOME. It is set once at launch (see `lib.rs`), before any thread exists:
    // changing the environment while other threads run is not safe. Only a fallback folder, chosen
    // because the preferred one is not writable, differs from that and needs it set here.
    let wanted_home = cache_path.join("hf");
    if std::env::var_os("HF_HOME").map(std::path::PathBuf::from).as_deref() != Some(wanted_home.as_path()) {
        std::env::set_var("HF_HOME", &wanted_home);
    }

    let attempt_cache = cache_path.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let _guard = SuppressCrashDialogGuard::new();
        run_attempts(ATTEMPTS, BASE_DELAY, |attempt| {
            if attempt > 0 {
                let n = clear_stale_download_files(&attempt_cache);
                crate::app_log::log("embedding", &format!("cleared {n} stale download file(s) before attempt {}", attempt + 1));
            }
            let options = InitOptions::new(EmbeddingModel::BGEBaseENV15)
                .with_cache_dir(attempt_cache.clone())
                .with_show_download_progress(true);
            // `{e:#}` includes the whole cause chain, which is where the real reason lives.
            let model = TextEmbedding::try_new(options).map_err(|e| format!("{e:#}"))?;
            // Loading is not working: embed a sentence before declaring success.
            let check = model.embed(vec!["plugable chat self-check"], None).map_err(|e| format!("{e:#}")).and_then(|out| validate_embeddings(&out));
            if let Err(why) = check {
                let n = clear_model_cache(&attempt_cache);
                return Err(format!("the downloaded model failed its self-check ({why}); removed {n} cached item(s) to download it again"));
            }
            Ok(model)
        })
    })
    .await;

    let failure = match joined {
        Ok(Ok(model)) => {
            crate::app_log::log("embedding", "CPU embedding model loaded and verified");
            *shared.write().await = Some(Arc::new(model));
            emit_progress(app, ALREADY_LOADED_MESSAGE, true, false);
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
        Ok(Err(f)) => format_failure(&f),
        Err(e) => {
            crate::app_log::log("embedding", &format!("init task failed: {e:?}"));
            format!("Failed to load CPU embedding model. The search model start-up task stopped unexpectedly. Details: {e}")
        }
    };
    crate::app_log::log("embedding", &format!("giving up: {failure}"));
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
        let r = run_attempts(3, Duration::from_millis(1), |_| {
            calls += 1;
            if calls < 3 { Err(format!("fail {calls}")) } else { Ok(calls) }
        });
        assert_eq!(r, Ok(3));
    }

    #[test]
    fn gives_up_after_the_attempt_limit_with_the_last_error_and_without_a_final_sleep() {
        let mut calls = 0;
        let started = std::time::Instant::now();
        let r: Result<(), AttemptFailure> = run_attempts(3, Duration::from_millis(150), |_| {
            calls += 1;
            Err(format!("fail {calls}"))
        });
        let f = r.unwrap_err();
        assert_eq!((f.attempts, f.last_error.as_str(), f.panicked), (3, "fail 3", false));
        // sleeps are 150ms + 300ms = 450ms; a trailing 600ms sleep would make it 1050ms. The limit sits between
        // them with room on both sides: a loaded CI runner (macOS arm64 failed the old 150ms margin) can stall by hundreds of ms.
        assert!(started.elapsed() < Duration::from_millis(900), "took {:?}: a final sleep after the last attempt?", started.elapsed());
    }

    #[test]
    fn one_attempt_never_sleeps() {
        let started = std::time::Instant::now();
        let r: Result<(), AttemptFailure> = run_attempts(1, Duration::from_secs(60), |_| Err("x".into()));
        assert_eq!(r.unwrap_err().last_error, "x");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    // R8 (Bob): a panic on the first attempt ended every retry.
    #[test]
    fn r8_a_panic_on_the_first_attempt_is_retried() {
        let mut calls = 0;
        let r = run_attempts(3, Duration::from_millis(1), |attempt| {
            calls += 1;
            if attempt == 0 {
                panic!("onnxruntime.dll could not be loaded");
            }
            Ok(attempt)
        });
        assert_eq!(r, Ok(1));
        assert_eq!(calls, 2);
    }

    #[test]
    fn r8_when_every_attempt_panics_the_text_says_so() {
        let f = run_attempts::<()>(2, Duration::from_millis(1), |_| panic!("DLL load failed")).unwrap_err();
        assert!(f.panicked && f.last_error.contains("DLL load failed"), "{f:?}");
        let text = format_failure(&f);
        assert!(text.starts_with("Failed to load CPU embedding model") && text.contains("2 attempts") && text.contains("Visual C++"), "{text}");
    }

    // R6 (Bob): the first-start budget was about six seconds.
    #[test]
    fn r6_the_retry_budget_outlasts_a_busy_first_start() {
        let total: Duration = (0..ATTEMPTS - 1).map(|n| (BASE_DELAY * (1u32 << n)).min(MAX_DELAY)).sum();
        assert!(total >= Duration::from_secs(30), "budget {total:?}");
    }

    // R7 (Bob): a Retry that finds the model already loaded must still say "done".
    #[test]
    fn r7_already_loaded_is_reported_as_complete_not_silent() {
        assert!(ALREADY_LOADED_MESSAGE.contains("loaded"));
    }

    #[test]
    fn embeddings_that_cannot_be_real_are_rejected() {
        assert!(validate_embeddings(&[vec![0.1, -0.2, 0.3]]).is_ok());
        assert!(validate_embeddings(&[]).is_err());
        assert!(validate_embeddings(&[vec![]]).is_err());
        assert!(validate_embeddings(&[vec![0.0, 0.0]]).is_err());
        assert!(validate_embeddings(&[vec![f32::NAN, 1.0]]).is_err());
    }

    #[test]
    fn a_corrupt_model_cache_is_emptied_but_its_folder_stays() {
        let dir = std::env::temp_dir().join(format!("pc-clear-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("models--x").join("blobs")).unwrap();
        std::fs::write(dir.join("models--x").join("blobs").join("b"), b"x").unwrap();
        std::fs::write(dir.join("loose"), b"x").unwrap();
        assert_eq!(clear_model_cache(&dir), 2);
        assert!(dir.exists() && std::fs::read_dir(&dir).unwrap().count() == 0);
        let _ = std::fs::remove_dir_all(&dir);
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
}
