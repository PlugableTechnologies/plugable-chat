//! Backend abstraction for Foundry Local integration.
//!
//! Two implementations sit behind [`FoundryBackend`]:
//! - `CliHttpBackend` — legacy: the `foundry` CLI + raw HTTP to the local service.
//! - `SdkBackend` — `foundry-local-sdk` 1.2.0 (bundled newer runtime, typed API).
//!
//! Selected at runtime via `AppSettings.foundry_backend` / the `PLUGABLE_FOUNDRY_BACKEND`
//! env override. The actor retains all POLICY (retry/restart, the version-keyed
//! incompatible-models blocklist, fallback UX, the GPU mutex, and capability inference);
//! the backend performs only I/O and returns these neutral DTOs — so neither impl leaks
//! `reqwest` or SDK types up into the actor.
#![allow(dead_code)] // wired incrementally during the CLI→SDK migration (Phases 2–3)

pub mod ep_ladder;
pub mod sdk;

#[cfg(test)]
mod core_pin_tests;
#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod fake_tests;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use crate::protocol::{CachedModel, CatalogModel, ChatMessage, ModelFamily, OpenAITool};

/// Connection/endpoint info discovered from the running service.
#[derive(Debug, Clone, Default)]
pub struct ConnectionInfo {
    /// Base URL of the OpenAI-compatible endpoint (e.g. `http://127.0.0.1:PORT`), if up.
    pub base_url: Option<String>,
    /// Execution providers Foundry has registered.
    pub registered_eps: Vec<String>,
    /// All valid execution providers on this system (feeds `has_gpu_eps`).
    pub valid_eps: Vec<String>,
}

/// Minimal model descriptor the actor's capability-inference loop consumes
/// (it only reads `id` and `tags`).
#[derive(Debug, Clone)]
pub struct RawModel {
    pub id: String,
    pub tags: Vec<String>,
}

/// Everything the backend needs to run one chat turn. The actor resolves every
/// capability/policy decision up front and hands the backend a fully-specified request.
#[derive(Debug, Clone)]
pub struct ChatStreamRequest {
    pub model: String,
    pub family: ModelFamily,
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<OpenAITool>>,
    pub use_native_tools: bool,
    pub supports_reasoning: bool,
    pub supports_reasoning_effort: bool,
    pub reasoning_effort: String,
    /// Legacy CLI/HTTP only: use `/v1/responses` instead of `/v1/chat/completions`.
    /// Ignored by the SDK backend (it uses the typed `ChatClient`).
    pub use_responses_api: bool,
}

/// Classified backend error so the actor can keep its retry / blocklist / fallback policy
/// backend-agnostic. `Server` covers deterministic-per-(model,runtime) failures at EITHER
/// load OR generation (the SDK surfaces qwen3.5's WebGPU bug at generation, not load).
#[derive(Debug, Clone)]
pub enum BackendError {
    /// 4xx-equivalent: client/request problem.
    Client(String),
    /// 5xx-equivalent / runtime exception — deterministic, blocklist-worthy.
    Server(String),
    /// Transport/connection problem (timeout, refused) — retryable, not blocklist-worthy.
    Connection(String),
    /// The operation was cancelled (user stop, SDK-side cancellation). Says nothing about
    /// the model, so it never feeds the blocklist or triggers a fallback.
    Cancelled(String),
}

impl BackendError {
    pub fn message(&self) -> &str {
        match self {
            BackendError::Client(m)
            | BackendError::Server(m)
            | BackendError::Connection(m)
            | BackendError::Cancelled(m) => m,
        }
    }
    pub fn is_server(&self) -> bool {
        matches!(self, BackendError::Server(_))
    }
    pub fn is_cancelled(&self) -> bool {
        matches!(self, BackendError::Cancelled(_))
    }

    /// Classify a failure raised while GENERATING (after the model loaded). Only a genuine
    /// runtime exception is `Server`; request-shape problems and cancellations are not the
    /// model's fault and must not blocklist it (rc14: a 17537-token request against an
    /// 8192-token context, and "Operation was cancelled", both marked models incompatible).
    pub fn classify_generation(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        let lower = msg.to_lowercase();
        if lower.contains("operation was cancel") || lower.contains("operationcanceledexception") {
            BackendError::Cancelled(msg)
        } else if is_context_overflow(&lower)
            || is_out_of_memory(&lower)
            || lower.contains("system.argumentexception")
            || lower.contains("max_tokens")
        {
            // Running out of memory on a long prompt says nothing about the model: the same model
            // answers a shorter one (rc15, A10G: "bad allocation" at ~18,000 input tokens).
            BackendError::Client(msg)
        } else if lower.contains("connection refused")
            || lower.contains("timed out")
            || lower.contains("timeout")
        {
            BackendError::Connection(msg)
        } else {
            BackendError::Server(msg)
        }
    }

    /// Classify a failure raised while LOADING. A load failure is deterministic per
    /// (model, runtime), so it stays `Server` unless it was a cancellation or transport issue.
    pub fn classify_load(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        match Self::classify_generation(msg.clone()) {
            e @ (BackendError::Cancelled(_) | BackendError::Connection(_)) => e,
            _ => BackendError::Server(msg),
        }
    }

    /// True when the request itself asked for more tokens than the model's context holds.
    pub fn is_context_overflow(&self) -> bool {
        matches!(self, BackendError::Client(m) if is_context_overflow(&m.to_lowercase()))
    }

    /// True when the engine ran out of memory building the request (the prompt is too long for this
    /// computer's graphics memory), or when we already turned such a failure into the plain-language message.
    pub fn is_out_of_memory(&self) -> bool {
        matches!(self, BackendError::Client(m) if is_out_of_memory(&m.to_lowercase()))
    }
}

fn is_context_overflow(lower: &str) -> bool {
    lower.contains("maximum context length") || lower.contains("exceeds the model's maximum")
}

/// Allocation failures as the engines word them: ONNX Runtime / GenAI ("bad allocation",
/// "Failed to allocate"), CUDA and DirectML ("out of memory", `E_OUTOFMEMORY`), and our own message.
fn is_out_of_memory(lower: &str) -> bool {
    [
        "bad allocation",
        "bad_alloc",
        "out of memory",
        "failed to allocate",
        "e_outofmemory",
        "memory allocation",
        "cudaerrormemoryallocation",
        OOM_MARKER,
    ]
    .iter()
    .any(|p| lower.contains(p))
}

/// Part of the plain-language message shown when a conversation cannot fit in graphics memory.
const OOM_MARKER: &str = "too long for this computer's graphics memory";

#[cfg(test)]
mod error_class_tests {
    use super::BackendError as E;

    const CTX: &str = "command execution error: Error: System.ArgumentException: This request requires 17537 total tokens (1153 input + 16384 output), which exceeds the model's maximum context length of 8192 tokens. Please reduce 'max_tokens'or shorten your input... (Foundry runtime sdk-1.2.3+core-1.2.4)";
    const CANCEL: &str = "command execution error: Operation was cancelled: The operation was canceled. (Foundry runtime sdk-1.2.3+core-1.2.4)";

    #[test]
    fn context_overflow_is_client_not_server() {
        let e = E::classify_generation(CTX);
        assert!(matches!(e, E::Client(_)));
        assert!(e.is_context_overflow());
        assert!(!e.is_server());
    }

    #[test]
    fn cancellation_is_not_server() {
        for m in [CANCEL, "The OPERATION WAS CANCELED", "OperationCanceledException: x"] {
            let e = E::classify_generation(m);
            assert!(!e.is_server(), "{m}");
        }
        assert!(E::classify_generation(CANCEL).is_cancelled());
        assert!(E::classify_load(CANCEL).is_cancelled());
    }

    #[test]
    fn ort_runtime_failure_stays_server() {
        let e = E::classify_generation("ORT runtime exception: WebGPU buffer failure");
        assert!(e.is_server());
        assert!(E::classify_load("Failed to load model: EP not available").is_server());
    }

    #[test]
    fn transport_errors_are_connection() {
        assert!(matches!(E::classify_generation("Connection refused"), E::Connection(_)));
        assert!(matches!(E::classify_load("request timed out"), E::Connection(_)));
    }

    #[test]
    fn load_failure_with_argument_exception_is_still_server() {
        assert!(E::classify_load("System.ArgumentException: bad model path").is_server());
    }
}

/// Budget scales tried in order: the full window, then a tighter one.
pub const OVERFLOW_RETRY_SCALES: [f64; 2] = [1.0, 0.6];

/// After an out-of-memory failure, the retry plans for this share of the input that just failed.
pub const MEMORY_RETRY_SHARE: f64 = 0.6;
/// Never plan below this many input tokens because of a memory failure: below it the failure is
/// not about the conversation's length.
const MIN_MEMORY_CEILING: u64 = 1024;

/// How a request is fitted for one attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// Share of the model's context window to plan against (1.0 = all of it).
    pub scale: f64,
    /// Hard ceiling on input tokens, learned from a memory failure earlier in this run.
    pub input_ceiling: Option<u64>,
}

static MEMORY_CEILINGS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> = std::sync::OnceLock::new();

/// The input size (tokens) this run found too large for the model on this machine, if any. Held for
/// the life of the process only: free memory differs from one start to the next.
pub fn memory_ceiling(model: &str) -> Option<u64> {
    MEMORY_CEILINGS.get().and_then(|m| m.lock().ok()).and_then(|m| m.get(model).copied())
}

fn note_memory_ceiling(model: &str, failed_input_tokens: u64) -> u64 {
    let ceiling = ((failed_input_tokens as f64 * MEMORY_RETRY_SHARE) as u64).max(MIN_MEMORY_CEILING);
    if let Ok(mut m) = MEMORY_CEILINGS.get_or_init(Default::default).lock() {
        let e = m.entry(model.to_string()).or_insert(ceiling);
        *e = (*e).min(ceiling);
        return *e;
    }
    ceiling
}

/// Forget what a run learned (tests, and after a model is unloaded).
pub fn forget_memory_ceilings() {
    if let Some(m) = MEMORY_CEILINGS.get() {
        if let Ok(mut m) = m.lock() {
            m.clear();
        }
    }
}

/// Runs `attempt(budget)`. Two kinds of failure on a request that was too big get one more try
/// instead of failing the turn, because nothing was generated:
/// * the core says it exceeds the context window (our estimate differs from its): plan against a tighter window;
/// * the engine runs out of memory: plan against a smaller input, 60% of the one that just failed, and
///   remember that ceiling for later turns so the conversation keeps going on this computer.
/// If the retry also runs out of memory the user gets a plain message, and the model is not blamed.
pub async fn with_overflow_retry<F, Fut>(model: &str, attempt: F) -> ChatStreamOutcome
where
    F: Fn(Budget) -> Fut,
    Fut: std::future::Future<Output = ChatStreamOutcome>,
{
    let first = attempt(Budget { scale: OVERFLOW_RETRY_SCALES[0], input_ceiling: memory_ceiling(model) }).await;
    if first.error.as_ref().map_or(false, |e| e.is_context_overflow()) {
        println!("[Backend] Context overflow; retrying once with a tighter budget");
        return attempt(Budget { scale: OVERFLOW_RETRY_SCALES[1], input_ceiling: memory_ceiling(model) }).await;
    }
    if first.error.as_ref().map_or(false, |e| e.is_out_of_memory()) && first.input_tokens > 0 {
        let ceiling = note_memory_ceiling(model, first.input_tokens);
        println!(
            "[Backend] Out of memory at about {} input tokens; retrying once with at most {ceiling}",
            first.input_tokens
        );
        let mut second = attempt(Budget { scale: OVERFLOW_RETRY_SCALES[0], input_ceiling: Some(ceiling) }).await;
        if let Some(e) = second.error.take() {
            second.error = Some(if e.is_out_of_memory() { oom_message(&e, first.input_tokens) } else { e });
        }
        return second;
    }
    first
}

fn oom_message(original: &BackendError, input_tokens: u64) -> BackendError {
    BackendError::Client(format!(
        "This conversation is {OOM_MARKER} (about {input_tokens} tokens of history). Start a new chat to continue, or ask in a shorter chat. Details: {}",
        original.message()
    ))
}

/// Outcome of a streamed chat turn. Text deltas are delivered out-of-band via the `sink`.
#[derive(Debug, Default)]
pub struct ChatStreamOutcome {
    pub cancelled: bool,
    pub error: Option<BackendError>,
    /// Estimated input tokens of the request that was sent (0 when it never got that far); the memory retry plans from it.
    pub input_tokens: u64,
}

/// Progress callback for downloads (percentage, 0.0..=100.0).
pub type ProgressSink = Box<dyn FnMut(f64) + Send>;

/// The operations the model-gateway actor needs from a Foundry integration, regardless
/// of whether it's backed by the CLI/HTTP path or the SDK.
#[async_trait::async_trait]
pub trait FoundryBackend: Send + Sync {
    /// Ensure the service is running; refresh and return connection info.
    /// `&self` (impls use interior mutability) so the actor can call from `&self` contexts.
    async fn ensure_service(&self) -> Result<(), String>;
    async fn stop_service(&self) -> Result<(), String>;
    async fn restart_service(&self) -> Result<(), String>;
    async fn connection_info(&self) -> ConnectionInfo;

    async fn list_models(&self) -> Vec<RawModel>;
    async fn list_catalog(&self) -> Vec<CatalogModel>;
    async fn list_cached(&self) -> Vec<CachedModel>;
    async fn list_loaded(&self) -> Vec<String>;

    /// Load a model. A `Server` error here is deterministic and should feed the blocklist.
    async fn load(&self, model: &str) -> Result<(), BackendError>;
    async fn unload(&self, model: &str) -> Result<(), String>;
    /// `cancel`: when set to true mid-download, the download stops and returns an error.
    async fn download(
        &self,
        model: &str,
        progress: ProgressSink,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<(), String>;
    async fn remove_cached(&self, model: &str) -> Result<(), String>;

    /// Stream one chat turn: push text deltas — and, for native tool calls, the same
    /// `<tool_call>{…}</tool_call>` text the current pipeline emits — into `sink`; honor
    /// `cancel`. All downstream reasoning/tool parsing runs on that text, so both backends
    /// only need to reproduce the identical `String` stream.
    async fn chat_stream(
        &self,
        req: ChatStreamRequest,
        sink: UnboundedSender<String>,
        cancel: watch::Receiver<bool>,
    ) -> ChatStreamOutcome;

    /// Key for the version-scoped incompatible-models blocklist
    /// (`CliHttp` → e.g. `"0.8.119"`; `Sdk` → e.g. `"sdk-1.2.0"`).
    fn runtime_version_key(&self) -> String;
}
