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

pub mod sdk;

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
}

impl BackendError {
    pub fn message(&self) -> &str {
        match self {
            BackendError::Client(m) | BackendError::Server(m) | BackendError::Connection(m) => m,
        }
    }
    pub fn is_server(&self) -> bool {
        matches!(self, BackendError::Server(_))
    }
}

/// Outcome of a streamed chat turn. Text deltas are delivered out-of-band via the `sink`.
#[derive(Debug, Default)]
pub struct ChatStreamOutcome {
    pub cancelled: bool,
    pub error: Option<BackendError>,
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
    async fn download(&self, model: &str, progress: ProgressSink) -> Result<(), String>;
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
