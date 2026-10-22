//! `SdkBackend` — the `foundry-local-sdk` 1.2.0 implementation of [`FoundryBackend`].
//!
//! Uses the SDK's bundled runtime (newer than the brew CLI): a process-global
//! `FoundryLocalManager` singleton starts an embedded OpenAI-compatible web service and
//! manages the model lifecycle. Chat runs through the SDK's typed `ChatClient`, and we
//! reproduce the exact `String` stream the legacy pipeline emits (text deltas + native
//! tool calls re-serialized as `<tool_call>{…}</tool_call>` text) so all downstream
//! reasoning/tool parsing is unchanged.
#![allow(dead_code)] // constructed/wired into the actor in the next increment (Phase 3 cont.)

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessage,
    ChatCompletionRequestUserMessage, ChatCompletionTool, ChatCompletionTools, FunctionCall, FunctionObject,
};
use foundry_local_sdk::{ChatToolChoice, DeviceType, FoundryLocalConfig, FoundryLocalManager};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use super::super::context_budget;
use super::{
    BackendError, ChatStreamOutcome, ChatStreamRequest, ConnectionInfo, FoundryBackend, ProgressSink,
    RawModel,
};
use crate::protocol::{CachedModel, CatalogModel, CatalogModelRuntime, ChatMessage, ModelFamily};

/// True only when WebGPU is the sole non-CPU execution provider. Windows hosts list WebGPU
/// alongside CUDA/DirectML, and there the vision-model bug does not apply.
fn webgpu_is_only_gpu_ep<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let (mut webgpu, mut other_gpu) = (false, false);
    for n in names {
        let n = n.to_lowercase();
        if n.contains("webgpu") {
            webgpu = true;
        } else if !n.contains("cpu") {
            other_gpu = true;
        }
    }
    webgpu && !other_gpu
}

/// Key used by the version-scoped incompatible-models blocklist under the SDK runtime.
/// Distinct from the CLI's `foundry --version` (e.g. `0.8.119`), so a model blocklisted on
/// the old runtime is automatically re-evaluated here. Keep in step with the `foundry-local-sdk`
/// pin in Cargo.toml (`sdk_version_key_matches_cargo_pin` fails if they drift): changing the key
/// is what makes a model blocklisted on an older SDK get a fresh chance.
///
/// The Core is part of the key because it is what decides whether a model loads: models blocklisted
/// while Core 1.2.3 never registered a GPU provider must get a fresh chance on Core 1.2.4
/// (`foundry-core-version.txt`; `sdk_version_key_matches_cargo_pin` keeps both halves honest).
pub const FOUNDRY_SDK_VERSION_KEY: &str = "sdk-1.2.3+core-1.2.4";

/// Process-global manager singleton (matches the SDK's own `create()` singleton contract).
static MANAGER: OnceLock<&'static FoundryLocalManager> = OnceLock::new();

fn get_or_create_manager(
    cache_dir: &Path,
    library_dir: Option<&Path>,
) -> Result<&'static FoundryLocalManager, String> {
    if let Some(m) = MANAGER.get() {
        return Ok(*m);
    }
    let mut config = FoundryLocalConfig::new("plugable-chat")
        .model_cache_dir(cache_dir.to_string_lossy().to_string());
    // In a packaged build the SDK's compile-time OUT_DIR (`FOUNDRY_NATIVE_DIR`) doesn't
    // exist on the user's machine, so point the native-core loader at the libs we bundle
    // into the app instead. In dev this is `None` and the SDK falls back to its OUT_DIR.
    // `library_path` maps to the SDK's `FoundryLocalCorePath` (a directory containing
    // `Microsoft.AI.Foundry.Local.Core.<ext>` plus its onnxruntime siblings).
    if let Some(dir) = library_dir {
        config = config.library_path(dir.to_string_lossy().to_string());
    }
    // The engine's own log is the only record of why it skipped a provider; left at the SDK
    // default it went somewhere support never sees. It is attached to "Copy details for support".
    if !cfg!(test) {
        config = config
            .logs_dir(crate::app_log::engine_log_dir().to_string_lossy().to_string())
            .log_level(foundry_local_sdk::LogLevel::Info);
    }
    // SDK v2 bootstraps the Windows App SDK for unpackaged apps by default; 1.2.x does not, and
    // without it the Windows ML provider catalog can come back empty. Opt-in until a real eGPU
    // machine shows whether 1.2.3's core honours it.
    if std::env::var("PLUGABLE_FOUNDRY_WINML_BOOTSTRAP").map(|v| v.trim() == "1").unwrap_or(false) {
        crate::app_log::log("gpu", "Windows App SDK bootstrap requested (PLUGABLE_FOUNDRY_WINML_BOOTSTRAP=1)");
        config = config.additional_setting("Bootstrap", "true");
    }
    let manager = FoundryLocalManager::create(config).map_err(|e| format!("SDK create failed: {e}"))?;
    let _ = MANAGER.set(manager);
    Ok(*MANAGER.get().expect("manager just set"))
}

pub struct SdkBackend {
    manager: &'static FoundryLocalManager,
    // Interior mutability so the whole trait is `&self` — lets the actor call these from
    // its many `&self` handlers without threading `&mut`.
    base_url: Mutex<Option<String>>,
    started: AtomicBool,
}

impl SdkBackend {
    /// Construct the backend, reusing the existing on-disk cache (so already-downloaded
    /// models are found without re-download). Does NOT start the web service yet.
    ///
    /// `library_dir`, when `Some`, is the directory holding the bundled native runtime
    /// libraries (used in packaged builds); `None` lets the SDK use its compile-time
    /// OUT_DIR (dev builds). See [`get_or_create_manager`].
    pub fn new(cache_dir: &Path, library_dir: Option<&Path>) -> Result<Self, String> {
        let manager = get_or_create_manager(cache_dir, library_dir)?;
        Ok(Self {
            manager,
            base_url: Mutex::new(None),
            started: AtomicBool::new(false),
        })
    }

    /// Last-known base URL of the embedded OpenAI-compatible service, if started.
    pub fn base_url(&self) -> Option<String> {
        self.base_url.lock().ok().and_then(|g| g.clone())
    }

    /// The concrete model that downloading `id` will fetch, and its size in bytes when the
    /// catalog knows it. Falls back to `id` itself; never fails.
    pub async fn download_info(&self, id: &str) -> (String, Option<u64>) {
        let catalog = self.list_catalog().await;
        match pick_catalog_entry(&catalog, id) {
            Some(m) => (
                m.name.clone(),
                (m.file_size_mb > 0).then(|| m.file_size_mb * 1024 * 1024),
            ),
            None => (id.to_string(), None),
        }
    }

    /// Resolve a `Model` handle from a variant id (`qwen3.5-4b-cuda-gpu:4`), an alias, or a short
    /// name such as `qwen3.5-4b` or `phi-4-mini-instruct`.
    ///
    /// `get_model_variant` only accepts a full variant id, so a short name failed with
    /// "Unknown variant id" and the first-run download of the default model (and the download
    /// of the fallback) could not work in SDK mode. A short name now resolves to the variant
    /// best suited to this machine: GPU before CPU, CUDA before other GPU providers, variants
    /// flagged incompatible last, then the smallest download.
    async fn model(&self, id: &str) -> Result<std::sync::Arc<foundry_local_sdk::Model>, String> {
        let catalog = self.manager.catalog();
        let variant_err = match catalog.get_model_variant(id).await {
            Ok(m) => return Ok(m),
            Err(e) => e.to_string(),
        };
        if let Ok(m) = catalog.get_model(id).await {
            return Ok(m);
        }

        let wanted = id.to_lowercase();
        let mut candidates: Vec<CatalogModel> = self
            .list_catalog()
            .await
            .into_iter()
            .filter(|m| {
                m.name.to_lowercase().contains(&wanted) || m.alias.to_lowercase().contains(&wanted)
            })
            .collect();
        candidates.sort_by_key(variant_preference_key);
        match candidates.first() {
            Some(best) => catalog
                .get_model_variant(&best.name)
                .await
                .map_err(|e| format!("get_model_variant('{}') failed: {e}", best.name)),
            None => Err(format!(
                "get_model_variant('{id}') failed: {variant_err}; no catalog model matches '{id}'"
            )),
        }
    }
}

/// The catalog entry a short name, alias or variant id will download, with its size in bytes.
/// Mirrors the choice `SdkBackend::model` makes so the size shown matches what is fetched.
fn pick_catalog_entry<'a>(catalog: &'a [CatalogModel], id: &str) -> Option<&'a CatalogModel> {
    let wanted = id.to_lowercase();
    if let Some(exact) = catalog.iter().find(|m| m.name.to_lowercase() == wanted) {
        return Some(exact);
    }
    let mut candidates: Vec<&CatalogModel> = catalog
        .iter()
        .filter(|m| m.name.to_lowercase().contains(&wanted) || m.alias.to_lowercase().contains(&wanted))
        .collect();
    candidates.sort_by_key(|m| variant_preference_key(m));
    candidates.into_iter().next()
}

/// Sort key for choosing among variants of one model: lower is better.
fn variant_preference_key(m: &CatalogModel) -> (bool, u8, u8, u64) {
    let ep = m.runtime.execution_provider.to_lowercase();
    let provider_rank = if ep.contains("cuda") {
        0
    } else if ep.contains("tensorrt") || ep.contains("dml") || ep.contains("directml") {
        1
    } else if ep.contains("webgpu") {
        2
    } else {
        3
    };
    // GPU, then CPU (runs anywhere), then NPU/other (only useful on hardware we don't probe).
    // rc14 let the smallest download win, which picked a 2.2 GB OpenVINO NPU build on a PC
    // without an NPU.
    let device_rank = match m.runtime.device_type.as_str() {
        "GPU" => 0,
        "CPU" => 1,
        _ => 2,
    };
    (m.incompatible, device_rank, provider_rank, m.file_size_mb as u64)
}

/// How often a long provider download or registration logs that it is still waiting.
const EP_REGISTRATION_HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// What `SdkBackend::register_execution_providers` did.
///
/// A provider in `failed` is normal on hardware that does not support it (CUDA on a machine
/// without an NVIDIA GPU, for example) and must never stop the app from starting.
#[derive(Debug, Clone, Default)]
pub struct EpRegistrationSummary {
    /// Registered before this call (a previous run's downloads are still on disk).
    pub already_registered: Vec<String>,
    /// Downloaded (if needed) and registered by this call.
    pub registered: Vec<String>,
    /// Could not be registered.
    pub failed: Vec<String>,
    /// Why each provider in `failed` (or discovery itself) failed, classified for the UI.
    pub failures: Vec<crate::gpu_diagnostics::EpFailure>,
    /// The caller cancelled the download.
    pub cancelled: bool,
    pub seconds: f64,
}

/// Provider names to ask the engine for, each once, in the order it listed them.
///
/// On a machine with two adapters (Pat: Intel Iris Xe plus an RTX 5080 over Thunderbolt) the
/// engine lists some providers twice. Passing the duplicates back is the one input that differed
/// from the single-GPU A10G where registration works.
pub fn unique_missing(eps: &[crate::gpu_assessment::EpObservation]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for e in eps.iter().filter(|e| !e.is_registered) {
        if !names.contains(&e.name) {
            names.push(e.name.clone());
        }
    }
    names
}

/// Turn the engine's answer into what we record. `(registered, failed, failures)`.
///
/// The engine can return `success: false` with nothing registered and nothing failed. That used to
/// count as "nothing to report": no failure, no retry, no log line, and the card blamed the
/// hardware. Every provider we asked for is accounted for now.
pub fn interpret_registration(
    requested: &[String],
    success: bool,
    status: &str,
    registered: Vec<String>,
    failed: Vec<String>,
    cancelled: bool,
    elapsed_ms: u128,
) -> (Vec<String>, Vec<String>, Vec<crate::gpu_diagnostics::EpFailure>) {
    let detail = if status.trim().is_empty() {
        "registration failed (no detail from the runtime)".to_string()
    } else {
        status.trim().to_string()
    };
    let mut failures: Vec<crate::gpu_diagnostics::EpFailure> =
        failed.iter().map(|ep| ep_failure(ep, &detail, cancelled)).collect();
    let mut failed = failed;
    let unaccounted: Vec<&String> = requested
        .iter()
        .filter(|ep| !registered.contains(ep) && !failed.contains(ep))
        .collect();
    if !unaccounted.is_empty() && !cancelled && (!success || registered.is_empty()) {
        let message = format!(
            "the engine returned after {elapsed_ms} ms without registering or rejecting it (success: {success}, status: {})",
            if status.trim().is_empty() { "empty" } else { status.trim() }
        );
        for ep in unaccounted {
            failures.push(crate::gpu_diagnostics::EpFailure {
                ep: ep.clone(),
                kind: crate::gpu_diagnostics::EpFailureKind::EngineDeclined,
                message: message.clone(),
            });
            failed.push(ep.clone());
        }
    }
    (registered, failed, failures)
}

/// A cancel request means the user stopped it, whatever text the runtime returned.
fn ep_failure(ep: &str, message: &str, cancelled: bool) -> crate::gpu_diagnostics::EpFailure {
    let mut failure = crate::gpu_diagnostics::make_failure(ep, message);
    if cancelled {
        failure.kind = crate::gpu_diagnostics::EpFailureKind::Cancelled;
    }
    failure
}

/// The real engine behind the registration ladder: one `download_and_register_eps` call per
/// attempt, with the shared progress callback, cancel flag and a heartbeat in the log.
pub struct SdkEngine {
    manager: &'static FoundryLocalManager,
    progress: Arc<std::sync::Mutex<Box<dyn FnMut(&str, f64) + Send>>>,
    /// The latest (provider, percent), so the heartbeat can say where a long wait is stuck.
    latest_progress: Arc<std::sync::Mutex<(String, f64)>>,
    cancel: Arc<AtomicBool>,
}

impl SdkEngine {
    fn report_progress(&self, ep: &str, percent: f64) {
        if let Ok(mut latest) = self.latest_progress.lock() {
            *latest = (ep.to_string(), percent);
        }
        if let Ok(mut progress) = self.progress.lock() {
            (progress)(ep, percent);
        }
    }
}

#[async_trait::async_trait]
impl super::ep_ladder::RegistrationEngine for SdkEngine {
    async fn attempt(&mut self, names: Option<Vec<String>>) -> super::ep_ladder::EngineAnswer {
        use super::ep_ladder::EngineAnswer;
        let started = std::time::Instant::now();
        let mut builder = self.manager.download_and_register_eps_builder();
        if let Some(names) = &names {
            builder = builder.names(names.clone());
        }
        let latest_writer = Arc::clone(&self.latest_progress);
        let progress = Arc::clone(&self.progress);
        let registration = builder
            .progress(move |ep: &str, percent: f64| {
                if let Ok(mut latest) = latest_writer.lock() {
                    *latest = (ep.to_string(), percent);
                }
                if let Ok(mut cb) = progress.lock() {
                    (cb)(ep, percent);
                }
            })
            .cancel(Arc::clone(&self.cancel))
            .run();
        tokio::pin!(registration);
        let mut heartbeat_interval = tokio::time::interval(EP_REGISTRATION_HEARTBEAT_INTERVAL);
        heartbeat_interval.tick().await; // the first tick completes immediately
        let result = loop {
            tokio::select! {
                result = &mut registration => break result,
                _ = heartbeat_interval.tick() => {
                    let (ep, percent) = self.latest_progress
                        .lock()
                        .map(|latest| latest.clone())
                        .unwrap_or_default();
                    crate::app_log::log("gpu", &format!(
                        "still registering providers after {}s (latest: {} {:.0}%, cancel requested: {})",
                        started.elapsed().as_secs(),
                        ep,
                        percent,
                        self.cancel.load(Ordering::SeqCst)
                    ));
                }
            }
        };
        let requested = names.as_ref().map(|n| n.join(", ")).unwrap_or_else(|| "all providers".to_string());
        match result {
            Ok(r) => {
                crate::app_log::log(
                    "gpu",
                    &format!(
                        "engine answered after {} ms for [{}]: success={}, status={:?}, registered=[{}], failed=[{}]",
                        started.elapsed().as_millis(),
                        requested,
                        r.success,
                        r.status,
                        r.registered_eps.join(", "),
                        r.failed_eps.join(", ")
                    ),
                );
                EngineAnswer {
                    success: r.success,
                    status: r.status,
                    registered: r.registered_eps,
                    failed: r.failed_eps,
                    error: None,
                    elapsed_ms: started.elapsed().as_millis(),
                }
            }
            Err(e) => {
                crate::app_log::log("gpu", &format!("provider registration for [{requested}] failed: {e}"));
                EngineAnswer { error: Some(e.to_string()), elapsed_ms: started.elapsed().as_millis(), ..Default::default() }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Probe access (`plugable-chat --ep-probe`, see ep_probe.rs)
// ---------------------------------------------------------------------------------------------

/// Create the engine for one probe process. `stock` is the SDK's own minimal configuration (only
/// the native library location), to tell whether anything in our configuration matters; otherwise
/// it is ours, including the `PLUGABLE_FOUNDRY_WINML_BOOTSTRAP` switch.
pub fn probe_manager(library_dir: Option<&Path>, stock: bool) -> Result<&'static FoundryLocalManager, String> {
    if !stock {
        let cache = std::env::temp_dir().join("plugable-chat-ep-probe-cache");
        let _ = std::fs::create_dir_all(&cache);
        return get_or_create_manager(&cache, library_dir);
    }
    let mut config = FoundryLocalConfig::new("ep-probe");
    if let Some(dir) = library_dir {
        config = config.library_path(dir.to_string_lossy().to_string());
    }
    FoundryLocalManager::create(config).map_err(|e| format!("SDK create failed: {e}"))
}

/// The engine's provider list as `(name, is_registered)`, repeats kept.
pub fn probe_discover(manager: &'static FoundryLocalManager) -> Result<Vec<(String, bool)>, String> {
    manager
        .discover_eps()
        .map(|eps| eps.iter().map(|e| (e.name.clone(), e.is_registered)).collect())
        .map_err(|e| e.to_string())
}

/// A registration engine for the probe: no progress reporting, never cancelled.
pub fn probe_engine(manager: &'static FoundryLocalManager) -> SdkEngine {
    SdkEngine {
        manager,
        progress: Arc::new(std::sync::Mutex::new(Box::new(|_: &str, _: f64| {}))),
        latest_progress: Arc::new(std::sync::Mutex::new((String::new(), 0.0))),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

impl SdkBackend {
    /// Ask the engine which execution providers exist and keep its raw answer (preflight report,
    /// GPU assessment). Every caller goes through here: an early return elsewhere used to lose
    /// what the engine said, which is how "no GPU models" could not be explained.
    fn observed_eps(&self) -> crate::gpu_assessment::EpDiscovery {
        crate::preflight::discover_eps_with_seam(|| {
            self.manager
                .discover_eps()
                .map(|eps| {
                    eps.iter()
                        .map(|e| crate::gpu_assessment::EpObservation { name: e.name.clone(), is_registered: e.is_registered })
                        .collect()
                })
                .map_err(|e| e.to_string())
        })
    }

    /// Discover the execution providers valid for this machine and register any that are not
    /// yet registered, downloading their packages the first time.
    ///
    /// Microsoft's SDK reference calls this "Explicit EP Management" (`discover_eps` +
    /// `download_and_register_eps`). Registration is per process and the SDK does not do it on
    /// its own, so without this step the catalog offers only CPU model variants and GPU models
    /// fail to load ("requires the CUDAExecutionProvider ... not available").
    ///
    /// First run downloads about 1.5 GB (measured: 4 min 39 s on a T4 box); later runs find the
    /// files already on disk and finish in about 3 s. `progress` receives `(provider, percent)`;
    /// setting `cancel` stops a download in flight.
    pub async fn register_execution_providers<F>(
        &self,
        progress: F,
        cancel: Arc<AtomicBool>,
    ) -> EpRegistrationSummary
    where
        F: FnMut(&str, f64) + Send + 'static,
    {
        let started = std::time::Instant::now();
        let mut summary = EpRegistrationSummary::default();

        let eps = match self.observed_eps() {
            Ok(eps) => eps,
            Err(e) => {
                crate::app_log::log("gpu", &format!("provider discovery failed: {e}"));
                // Without discovery there is no provider name to blame, so the failure is
                // recorded against the step itself; the UI still shows the cause and a retry.
                summary
                    .failures
                    .push(crate::gpu_diagnostics::make_failure("discovery", &e));
                summary.seconds = started.elapsed().as_secs_f64();
                return summary;
            }
        };
        summary.already_registered = eps
            .iter()
            .filter(|e| e.is_registered)
            .map(|e| e.name.clone())
            .collect();
        let missing = unique_missing(&eps);
        if missing.is_empty() {
            return summary;
        }
        // Unattended tests stage the engine's answer; downloading real packages for names that
        // were invented by the test would only fail and hide what the test is checking.
        if crate::preflight::eps_seam_active() {
            match crate::preflight::registration_seam() {
                Some(Ok(staged)) => {
                    crate::app_log::log("gpu", &format!("test registration answer in use for {missing:?}: {staged:?}"));
                    let (registered, failed, failures) = interpret_registration(
                        &missing,
                        staged.success,
                        &staged.status,
                        staged.registered,
                        staged.failed,
                        false,
                        started.elapsed().as_millis(),
                    );
                    summary.registered = registered;
                    summary.failed = failed;
                    summary.failures = failures;
                }
                Some(Err(e)) => summary.failures.push(crate::gpu_diagnostics::make_failure("test seam", &e)),
                None => crate::app_log::log("gpu", &format!("test provider list in use: not registering {missing:?}")),
            }
            summary.seconds = started.elapsed().as_secs_f64();
            return summary;
        }

        // The engine's own unregistered list, repeats kept: the ladder's raw step needs it.
        let raw_missing: Vec<String> = eps.iter().filter(|e| !e.is_registered).map(|e| e.name.clone()).collect();
        crate::app_log::log("gpu", &format!("registering providers: {}", missing.join(", ")));

        let mut engine = SdkEngine {
            manager: self.manager,
            progress: Arc::new(std::sync::Mutex::new(Box::new(progress))),
            latest_progress: Arc::new(std::sync::Mutex::new((missing[0].clone(), 0.0))),
            cancel: Arc::clone(&cancel),
        };
        engine.report_progress(&missing[0], 0.0);
        let outcome = super::ep_ladder::run_ladder(&mut engine, &raw_missing, &cancel).await;
        summary.cancelled = cancel.load(Ordering::SeqCst);
        summary.registered = outcome.registered;
        summary.failed = outcome.failed;
        summary.failures = outcome.failures;
        summary.seconds = started.elapsed().as_secs_f64();
        // Ask again so "registered nothing" is observed, not inferred from the result object.
        let _ = self.observed_eps();
        crate::app_log::log(
            "gpu",
            &format!(
                "registration done in {:.1}s: registered=[{}], failed=[{}]{}",
                summary.seconds,
                summary.registered.join(", "),
                summary.failures.iter().map(|f| format!("{} ({:?})", f.ep, f.kind)).collect::<Vec<_>>().join(", "),
                if summary.cancelled { ", cancelled" } else { "" }
            ),
        );
        summary
    }
}

#[async_trait::async_trait]
impl FoundryBackend for SdkBackend {
    async fn ensure_service(&self) -> Result<(), String> {
        if !self.started.load(Ordering::SeqCst) {
            self.manager
                .start_web_service()
                .await
                .map_err(|e| format!("start_web_service failed: {e}"))?;
            self.started.store(true, Ordering::SeqCst);
        }
        // Refresh base_url after start (urls() is empty until the service is up).
        if self.base_url().is_none() {
            if let Ok(urls) = self.manager.urls() {
                let url = urls.into_iter().find(|u| u.starts_with("http"));
                if let Ok(mut g) = self.base_url.lock() {
                    *g = url;
                }
            }
        }
        Ok(())
    }

    async fn stop_service(&self) -> Result<(), String> {
        let res = self
            .manager
            .stop_web_service()
            .await
            .map_err(|e| format!("stop_web_service failed: {e}"));
        self.started.store(false, Ordering::SeqCst);
        if let Ok(mut g) = self.base_url.lock() {
            *g = None;
        }
        res
    }

    async fn restart_service(&self) -> Result<(), String> {
        let _ = self.stop_service().await;
        self.ensure_service().await
    }

    async fn connection_info(&self) -> ConnectionInfo {
        let _ = self.ensure_service().await;
        let (registered_eps, valid_eps) = match self.observed_eps() {
            Ok(eps) => {
                let valid: Vec<String> = eps.iter().map(|e| e.name.clone()).collect();
                let registered: Vec<String> = eps
                    .iter()
                    .filter(|e| e.is_registered)
                    .map(|e| e.name.clone())
                    .collect();
                (registered, valid)
            }
            // Recorded by observed_eps: the assessment reports it instead of showing "no GPU".
            Err(_) => (Vec::new(), Vec::new()),
        };
        ConnectionInfo {
            base_url: self.base_url(),
            registered_eps,
            valid_eps,
        }
    }

    async fn list_models(&self) -> Vec<RawModel> {
        match self.manager.catalog().get_cached_models().await {
            Ok(models) => models
                .iter()
                .map(|m| RawModel {
                    id: m.id().to_string(),
                    // The actor infers capabilities primarily from the id; surface the SDK's
                    // tool-calling capability as a tag for parity with the CLI tag path.
                    tags: match m.supports_tool_calling() {
                        Some(true) => vec!["supportsToolCalling".to_string()],
                        _ => Vec::new(),
                    },
                })
                .collect(),
            Err(e) => {
                println!("[SdkBackend] list_models (get_cached_models) failed: {e}");
                Vec::new()
            }
        }
    }

    async fn list_catalog(&self) -> Vec<CatalogModel> {
        // Vision-language models fail *generation* on the WebGPU execution provider
        // (onnxruntime-genai buffer-aliasing bug) — confirmed on Mac M3 where WebGPU is the
        // only GPU EP. Text models and CPU vision variants work. So we proactively flag
        // `vision-language-chat` + GPU models as incompatible ONLY when the runtime's GPU EP
        // is WebGPU — keeping CPU vision and (on CUDA/DirectML hosts) GPU vision installable.
        let gpu_ep_is_webgpu = self
            .observed_eps()
            .ok()
            .map(|eps| webgpu_is_only_gpu_ep(eps.iter().map(|e| e.name.as_str())))
            .unwrap_or(false);
        let listed = match self.manager.catalog().get_models().await {
            Ok(models) => {
                let gpu_models: Vec<String> = models
                    .iter()
                    .filter(|m| {
                        m.info()
                            .runtime
                            .as_ref()
                            .map(|r| device_type_str(&r.device_type) == "GPU")
                            .unwrap_or_else(|| infer_device_type_from_id(&m.info().id) == "GPU")
                    })
                    .map(|m| {
                        let ep = m.info().runtime.as_ref().map(|r| r.execution_provider.clone()).unwrap_or_default();
                        format!("{} ({ep})", m.info().id)
                    })
                    .collect();
                let gpu = gpu_models.len();
                // With no accelerator registered a GPU variant still showed up on Pat's machine;
                // which one, and for which provider, says what the engine thinks it can run.
                if (1..=5).contains(&gpu) {
                    crate::app_log::log("gpu", &format!("GPU models listed: {}", gpu_models.join(", ")));
                }
                crate::preflight::record_catalog(&Ok(crate::gpu_assessment::CatalogCounts { gpu, cpu: models.len() - gpu }));
                Ok(models)
            }
            Err(e) => {
                crate::preflight::record_catalog(&Err(e.to_string()));
                Err(e)
            }
        };
        match listed {
            Ok(models) => models
                .iter()
                .map(|m| {
                    let info = m.info();
                    // Device type drives the Models-tab device filter (defaults to GPU). Prefer
                    // the SDK's structured runtime; fall back to inferring from the id suffix so
                    // models are never silently hidden by an empty device type.
                    let device_type = info
                        .runtime
                        .as_ref()
                        .map(|r| device_type_str(&r.device_type))
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| infer_device_type_from_id(&info.id));
                    let execution_provider = info
                        .runtime
                        .as_ref()
                        .map(|r| r.execution_provider.clone())
                        .unwrap_or_default();
                    let task = info.task.clone().unwrap_or_default();
                    // A compatibility *warning* (not a hard block): vision-language models can
                    // crash during generation on the WebGPU EP (upstream onnxruntime-genai bug).
                    // We still offer them — the UI shows a "may not run here" badge so the user
                    // can install and try at their own discretion.
                    let incompatible =
                        gpu_ep_is_webgpu && device_type == "GPU" && task == "vision-language-chat";
                    let incompatible_reason = if incompatible {
                        Some("May not run on this device — vision models can fail on the WebGPU runtime (upstream onnxruntime-genai bug)".to_string())
                    } else {
                        None
                    };
                    CatalogModel {
                        name: info.id.clone(),
                        display_name: info.display_name.clone().unwrap_or_else(|| info.alias.clone()),
                        alias: info.alias.clone(),
                        uri: info.uri.clone(),
                        version: info.version.to_string(),
                        file_size_mb: info.file_size_mb.unwrap_or(0),
                        license: info.license.clone().unwrap_or_default(),
                        task,
                        supports_tool_calling: info.supports_tool_calling.unwrap_or(false),
                        runtime: CatalogModelRuntime {
                            device_type,
                            execution_provider,
                        },
                        publisher: info.publisher.clone().unwrap_or_default(),
                        incompatible,
                        incompatible_reason,
                    }
                })
                .collect(),
            Err(e) => {
                println!("[SdkBackend] list_catalog (get_models) failed: {e}");
                Vec::new()
            }
        }
    }

    async fn list_cached(&self) -> Vec<CachedModel> {
        match self.manager.catalog().get_cached_models().await {
            Ok(models) => models
                .iter()
                .map(|m| CachedModel {
                    alias: m.alias().to_string(),
                    model_id: m.id().to_string(),
                    incompatible: false,
                    incompatible_reason: None,
                })
                .collect(),
            Err(e) => {
                println!("[SdkBackend] list_cached failed: {e}");
                Vec::new()
            }
        }
    }

    async fn list_loaded(&self) -> Vec<String> {
        match self.manager.catalog().get_loaded_models().await {
            Ok(models) => models.iter().map(|m| m.id().to_string()).collect(),
            Err(e) => {
                println!("[SdkBackend] list_loaded failed: {e}");
                Vec::new()
            }
        }
    }

    async fn load(&self, model: &str) -> Result<(), BackendError> {
        let m = self
            .model(model)
            .await
            .map_err(BackendError::Server)?;
        // A load failure here is deterministic per (model, runtime) — classify as Server so
        // the actor records it in the version-keyed blocklist.
        m.load().await.map_err(|e| BackendError::classify_load(e.to_string()))
    }

    async fn unload(&self, model: &str) -> Result<(), String> {
        let m = self.model(model).await?;
        m.unload().await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn download(
        &self,
        model: &str,
        progress: ProgressSink,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<(), String> {
        let m = self.model(model).await?;
        let mut builder = m.download_builder().progress(progress);
        if let Some(flag) = cancel {
            builder = builder.cancel(flag);
        }
        builder.run().await.map_err(|e| e.to_string())
    }

    async fn remove_cached(&self, model: &str) -> Result<(), String> {
        let m = self.model(model).await?;
        m.remove_from_cache().await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn chat_stream(
        &self,
        req: ChatStreamRequest,
        sink: UnboundedSender<String>,
        cancel: watch::Receiver<bool>,
    ) -> ChatStreamOutcome {
        super::with_overflow_retry(|scale| {
            self.chat_stream_attempt(req.clone(), sink.clone(), cancel.clone(), scale)
        })
        .await
    }

    fn runtime_version_key(&self) -> String {
        FOUNDRY_SDK_VERSION_KEY.to_string()
    }
}

/// Map the SDK device type to the string the Models-tab device filter compares against.
fn device_type_str(d: &DeviceType) -> &'static str {
    match d {
        DeviceType::CPU => "CPU",
        DeviceType::GPU => "GPU",
        DeviceType::NPU => "NPU",
        DeviceType::Invalid => "",
    }
}

/// Fallback device type inferred from the model id (e.g. `…-generic-gpu:2`) when the SDK
/// doesn't provide structured runtime info.
fn infer_device_type_from_id(id: &str) -> String {
    let lower = id.to_lowercase();
    if lower.contains("-gpu") || lower.contains("gpu:") {
        "GPU".to_string()
    } else if lower.contains("-npu") || lower.contains("npu:") {
        "NPU".to_string()
    } else if lower.contains("-cpu") || lower.contains("cpu:") {
        "CPU".to_string()
    } else {
        String::new()
    }
}

/// Map our `ChatMessage`s to async-openai request messages (the SDK's `ChatClient` input).
fn map_messages(messages: &[ChatMessage]) -> Vec<ChatCompletionRequestMessage> {
    messages
        .iter()
        .map(|msg| match msg.role.as_str() {
            "system" => ChatCompletionRequestSystemMessage::from(msg.content.clone()).into(),
            "tool" => ChatCompletionRequestToolMessage {
                content: msg.content.clone().into(),
                tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
            }
            .into(),
            "assistant" => {
                if let Some(tcs) = &msg.tool_calls {
                    ChatCompletionRequestAssistantMessage {
                        content: if msg.content.is_empty() {
                            None
                        } else {
                            Some(msg.content.clone().into())
                        },
                        tool_calls: Some(
                            tcs.iter()
                                .map(|tc| {
                                    ChatCompletionMessageToolCalls::Function(
                                        ChatCompletionMessageToolCall {
                                            id: tc.id.clone(),
                                            function: FunctionCall {
                                                name: tc.function.name.clone(),
                                                arguments: tc.function.arguments.clone(),
                                            },
                                        },
                                    )
                                })
                                .collect(),
                        ),
                        ..Default::default()
                    }
                    .into()
                } else {
                    ChatCompletionRequestAssistantMessage::from(msg.content.clone()).into()
                }
            }
            // Default everything else (incl. "user") to a user message.
            _ => ChatCompletionRequestUserMessage::from(msg.content.clone()).into(),
        })
        .collect()
}

impl SdkBackend {
    async fn chat_stream_attempt(
        &self,
        req: ChatStreamRequest,
        sink: UnboundedSender<String>,
        cancel: watch::Receiver<bool>,
        budget_scale: f64,
    ) -> ChatStreamOutcome {
        let mut outcome = ChatStreamOutcome::default();
        let mut cancel = cancel;

        // Resolve + ensure loaded.
        let model = match self.model(&req.model).await {
            Ok(m) => m,
            Err(e) => {
                outcome.error = Some(BackendError::classify_load(e));
                return outcome;
            }
        };
        if !model.is_loaded().await.unwrap_or(false) {
            if let Err(e) = model.load().await {
                outcome.error = Some(BackendError::classify_load(e.to_string()));
                return outcome;
            }
        }

        // Build the client with family-specific tuning (mirrors request_builder.rs).
        // NOTE: the SDK chat builder exposes no `reasoning_effort` (Phi/Generic) or
        // `repetition_penalty` (Granite); those tuning hints are dropped on this backend.
        let mut client = model.create_chat_client();
        let reasoning_capped = req.supports_reasoning && req.supports_reasoning_effort;
        let family_cap: u64 = match req.family {
            ModelFamily::GptOss | ModelFamily::Qwen => 16384,
            ModelFamily::Phi | ModelFamily::Generic => {
                if reasoning_capped { 8192 } else { 16384 }
            }
            ModelFamily::Gemma | ModelFamily::Granite => 8192,
        };
        match req.family {
            ModelFamily::GptOss | ModelFamily::Qwen | ModelFamily::Granite => {
                client = client.temperature(0.7);
            }
            ModelFamily::Gemma => {
                client = client.temperature(0.7).top_k(40);
            }
            ModelFamily::Phi | ModelFamily::Generic => {}
        }

        // Fit the request to this model's context window. Asking a small-context model for
        // the family's full output cap made the core reject the request, which used to be
        // misread as "model incompatible".
        // `budget_scale` < 1 on the retry after the core still rejected the size.
        let context_length = model.context_length();
        let plan = context_budget::plan_request(&req.messages, context_length, family_cap, budget_scale);
        let (messages_in, max_tokens) = (plan.messages, plan.max_tokens);
        println!(
            "[SdkBackend] max_tokens={max_tokens} (cap {family_cap}, input ~{}, dropped {}, context {:?}, scale {budget_scale})",
            plan.input_tokens, plan.dropped, context_length
        );
        crate::run_state::note_request(plan.input_tokens, max_tokens);
        client = client.max_tokens(max_tokens as u32);

        // Map messages and tools.
        let messages = map_messages(&messages_in);
        let tools: Option<Vec<ChatCompletionTools>> = if req.use_native_tools {
            req.tools.as_ref().map(|ts| {
                ts.iter()
                    .map(|t| {
                        ChatCompletionTools::Function(ChatCompletionTool {
                            function: FunctionObject {
                                name: t.function.name.clone(),
                                description: t.function.description.clone(),
                                parameters: t.function.parameters.clone(),
                                strict: None,
                            },
                        })
                    })
                    .collect()
            })
        } else {
            None
        };
        if tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
            client = client.tool_choice(ChatToolChoice::Auto);
        }

        let mut stream = match client
            .complete_streaming_chat(&messages, tools.as_deref())
            .await
        {
            Ok(s) => s,
            Err(e) => {
                outcome.error = Some(BackendError::classify_generation(e.to_string()));
                return outcome;
            }
        };

        // Reuse the existing accumulator so native tool calls become the identical
        // `<tool_call>{…}</tool_call>` text the downstream parsers already handle.
        let mut tool_calls = super::super::StreamingToolCalls::default();

        loop {
            if *cancel.borrow() {
                outcome.cancelled = true;
                break;
            }
            tokio::select! {
                biased;
                _ = cancel.changed() => { continue; }
                item = stream.next() => {
                    match item {
                        None => break,
                        Some(Ok(chunk)) => {
                            if let Some(choice) = chunk.choices.get(0) {
                                if let Some(content) = &choice.delta.content {
                                    if !content.is_empty() {
                                        let _ = sink.send(content.clone());
                                    }
                                }
                                if let Some(tcs) = &choice.delta.tool_calls {
                                    let vals: Vec<Value> = tcs
                                        .iter()
                                        .map(|c| {
                                            let (name, args) = match &c.function {
                                                Some(f) => (f.name.clone(), f.arguments.clone()),
                                                None => (None, None),
                                            };
                                            json!({
                                                "index": c.index,
                                                "id": c.id,
                                                "function": { "name": name, "arguments": args },
                                            })
                                        })
                                        .collect();
                                    tool_calls.process_streaming_tool_call_delta(&vals);
                                }
                            }
                        }
                        Some(Err(e)) => {
                            outcome.error = Some(BackendError::classify_generation(e.to_string()));
                            break;
                        }
                    }
                }
            }
        }

        // Emit accumulated native tool calls in the exact legacy text format.
        if !tool_calls.is_empty() {
            for call in tool_calls.into_parsed_calls() {
                let text = format!(
                    "\n<tool_call>{{\"name\": \"{}\", \"arguments\": {}}}</tool_call>\n",
                    call.tool,
                    serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
                );
                let _ = sink.send(text);
            }
        }

        outcome
    }
}

#[cfg(test)]
mod tests {
    fn variant(name: &str, device: &str, ep: &str, size: u64) -> CatalogModel {
        CatalogModel {
            name: name.to_string(),
            display_name: String::new(),
            alias: "phi-4-mini-instruct".to_string(),
            uri: String::new(),
            version: String::new(),
            file_size_mb: size,
            license: String::new(),
            task: String::new(),
            supports_tool_calling: false,
            runtime: CatalogModelRuntime {
                device_type: device.to_string(),
                execution_provider: ep.to_string(),
            },
            publisher: String::new(),
            incompatible: false,
            incompatible_reason: None,
        }
    }

    #[test]
    fn rc14_picked_the_smallest_npu_build_on_a_pc_without_an_npu() {
        let mut v = vec![
            variant("phi-4-mini-instruct-openvino-npu:4", "NPU", "OpenVINOExecutionProvider", 2200),
            variant("phi-4-mini-instruct-generic-cpu:5", "CPU", "CPUExecutionProvider", 4900),
        ];
        v.sort_by_key(variant_preference_key);
        assert_eq!(v[0].name, "phi-4-mini-instruct-generic-cpu:5");
    }

    #[test]
    fn gpu_beats_cpu_beats_npu_and_size_only_breaks_ties() {
        let mut v = vec![
            variant("npu", "NPU", "OpenVINOExecutionProvider", 100),
            variant("cpu", "CPU", "CPUExecutionProvider", 100),
            variant("gpu-big", "GPU", "WebGpuExecutionProvider", 9000),
            variant("gpu-small", "GPU", "WebGpuExecutionProvider", 8000),
        ];
        v.sort_by_key(variant_preference_key);
        let order: Vec<&str> = v.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(order, vec!["gpu-small", "gpu-big", "cpu", "npu"]);
    }

    use crate::gpu_diagnostics::EpFailureKind;

    fn obs(name: &str, reg: bool) -> crate::gpu_assessment::EpObservation {
        crate::gpu_assessment::EpObservation { name: name.into(), is_registered: reg }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // R17 (Pat, rc12): two adapters made the engine list providers twice; each is asked for once.
    #[test]
    fn r17_duplicate_provider_names_are_requested_once_in_order() {
        let eps = vec![
            obs("CUDAExecutionProvider", false),
            obs("WebGpuExecutionProvider", false),
            obs("OpenVINOExecutionProvider", false),
            obs("WebGpuExecutionProvider", false),
            obs("CPUExecutionProvider", true),
            obs("OpenVINOExecutionProvider", false),
        ];
        assert_eq!(
            super::unique_missing(&eps),
            names(&["CUDAExecutionProvider", "WebGpuExecutionProvider", "OpenVINOExecutionProvider"])
        );
    }

    // R18 (Pat, rc12): `success: false` with empty lists used to record nothing at all.
    #[test]
    fn r18_an_empty_refusal_is_an_engine_declined_failure_for_every_provider() {
        let asked = names(&["CUDAExecutionProvider", "WebGpuExecutionProvider"]);
        let (registered, failed, failures) =
            super::interpret_registration(&asked, false, "", vec![], vec![], false, 9);
        assert!(registered.is_empty());
        assert_eq!(failed, asked);
        assert_eq!(failures.len(), 2);
        assert!(failures.iter().all(|f| f.kind == EpFailureKind::EngineDeclined));
        assert!(failures[0].message.contains("9 ms") && failures[0].message.contains("success: false"));
        assert!(!crate::actors::foundry::gpu_registration::should_retry(&super::EpRegistrationSummary {
            failures,
            ..Default::default()
        }));
    }

    #[test]
    fn a_partial_success_does_not_blame_the_engine_for_the_rest() {
        let asked = names(&["CUDAExecutionProvider", "OpenVINOExecutionProvider"]);
        let (registered, failed, failures) = super::interpret_registration(
            &asked,
            true,
            "ok",
            names(&["CUDAExecutionProvider"]),
            vec![],
            false,
            40_000,
        );
        assert_eq!(registered, names(&["CUDAExecutionProvider"]));
        assert!(failed.is_empty() && failures.is_empty());
    }

    #[test]
    fn named_failures_keep_their_classified_reason() {
        let asked = names(&["CUDAExecutionProvider"]);
        let (_, failed, failures) = super::interpret_registration(
            &asked,
            false,
            "HTTP 503",
            vec![],
            names(&["CUDAExecutionProvider"]),
            false,
            1200,
        );
        assert_eq!(failed, asked);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, EpFailureKind::Network);
    }

    #[test]
    fn a_cancel_is_not_reported_as_the_engine_declining() {
        let asked = names(&["CUDAExecutionProvider"]);
        let (_, failed, failures) = super::interpret_registration(&asked, false, "", vec![], vec![], true, 5);
        assert!(failed.is_empty() && failures.is_empty());
    }

    #[test]
    fn webgpu_warning_only_when_webgpu_is_the_only_gpu_ep() {
        use super::webgpu_is_only_gpu_ep as f;
        assert!(f(["CPUExecutionProvider", "WebGpuExecutionProvider"].into_iter()));
        assert!(!f(["WebGpuExecutionProvider", "CUDAExecutionProvider"].into_iter()));
        assert!(!f(["DmlExecutionProvider", "WebGpuExecutionProvider"].into_iter()));
        assert!(!f(["CPUExecutionProvider"].into_iter()));
    }

    #[test]
    fn download_info_picks_the_variant_the_download_will_use() {
        let cpu = catalog_entry("qwen3.5-4b-cpu:1", "CPU", "CPUExecutionProvider", 3000, false);
        let gpu = catalog_entry("qwen3.5-4b-cuda-gpu:4", "GPU", "CUDAExecutionProvider", 2500, false);
        let catalog = vec![cpu, gpu];
        assert_eq!(pick_catalog_entry(&catalog, "qwen3.5-4b").unwrap().name, "qwen3.5-4b-cuda-gpu:4");
        assert_eq!(pick_catalog_entry(&catalog, "QWEN3.5-4B-CPU:1").unwrap().name, "qwen3.5-4b-cpu:1");
        assert!(pick_catalog_entry(&catalog, "phi-4").is_none());
    }

    #[test]
    fn sdk_version_key_matches_cargo_pin() {
        let cargo_toml = include_str!("../../../../Cargo.toml");
        let pinned = cargo_toml
            .lines()
            .find(|l| l.trim_start().starts_with("foundry-local-sdk"))
            .and_then(|l| l.split('"').nth(1))
            .expect("foundry-local-sdk pin in Cargo.toml");
        let core = include_str!("../../../../foundry-core-version.txt").trim();
        assert_eq!(FOUNDRY_SDK_VERSION_KEY, format!("sdk-{pinned}+core-{core}"));
    }

    use super::*;
    use crate::protocol::ChatMessage;

    fn catalog_entry(name: &str, device: &str, ep: &str, size_mb: u64, incompatible: bool) -> CatalogModel {
        CatalogModel {
            name: name.to_string(),
            runtime: CatalogModelRuntime {
                device_type: device.to_string(),
                execution_provider: ep.to_string(),
            },
            display_name: String::new(),
            alias: String::new(),
            uri: String::new(),
            version: String::new(),
            file_size_mb: size_mb,
            license: String::new(),
            task: "chat-completion".to_string(),
            supports_tool_calling: false,
            publisher: String::new(),
            incompatible,
            incompatible_reason: None,
        }
    }

    /// A short name like `qwen3.5-4b` must pick the variant best suited to the machine:
    /// GPU before CPU, CUDA before WebGPU, known-incompatible variants last, then the smaller download.
    #[test]
    fn variant_preference_orders_gpu_cuda_first() {
        let mut variants = vec![
            catalog_entry("qwen3.5-4b-generic-cpu:3", "CPU", "CPUExecutionProvider", 3000, false),
            catalog_entry("qwen3.5-4b-generic-gpu:4", "GPU", "WebGpuExecutionProvider", 4000, false),
            catalog_entry("qwen3.5-4b-cuda-gpu:4", "GPU", "CUDAExecutionProvider", 4200, false),
        ];
        variants.sort_by_key(variant_preference_key);
        let order: Vec<&str> = variants.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(order, ["qwen3.5-4b-cuda-gpu:4", "qwen3.5-4b-generic-gpu:4", "qwen3.5-4b-generic-cpu:3"]);

        // A variant flagged incompatible loses even to a CPU build.
        let mut flagged = vec![
            catalog_entry("m-cuda-gpu:1", "GPU", "CUDAExecutionProvider", 1000, true),
            catalog_entry("m-generic-cpu:1", "CPU", "CPUExecutionProvider", 1000, false),
        ];
        flagged.sort_by_key(variant_preference_key);
        assert_eq!(flagged[0].name, "m-generic-cpu:1");
    }

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn infers_device_type_from_id() {
        assert_eq!(infer_device_type_from_id("qwen3.5-0.8b-generic-gpu:2"), "GPU");
        assert_eq!(infer_device_type_from_id("phi-4-mini-generic-cpu:1"), "CPU");
        assert_eq!(infer_device_type_from_id("some-npu-model:1"), "NPU");
        assert_eq!(infer_device_type_from_id("mystery-model"), "");
    }

    #[test]
    fn maps_roles_to_request_messages() {
        // Pure mapping test (no runtime): system/user/assistant/tool all map without panic.
        let msgs = vec![
            ChatMessage { role: "system".into(), content: "sys".into(), system_prompt: None, tool_calls: None, tool_call_id: None },
            user("hi"),
            ChatMessage { role: "assistant".into(), content: "ok".into(), system_prompt: None, tool_calls: None, tool_call_id: None },
            ChatMessage { role: "tool".into(), content: "result".into(), system_prompt: None, tool_calls: None, tool_call_id: Some("call_1".into()) },
        ];
        let mapped = map_messages(&msgs);
        assert_eq!(mapped.len(), 4);
    }

    /// Verifies the catalog maps device type (the Models-tab GPU filter regression). Ignored
    /// by default (needs the SDK + catalog network refresh).
    ///   cargo test --lib backend::sdk::tests::sdk_backend_catalog_has_device_type -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn sdk_backend_catalog_has_device_type() {
        let home = dirs::home_dir().expect("home");
        let cache = home.join(".foundry").join("cache");
        let backend = SdkBackend::new(&cache, None).expect("new");
        backend.ensure_service().await.expect("service");
        let catalog = backend.list_catalog().await;
        eprintln!("[test] catalog has {} models", catalog.len());
        assert!(!catalog.is_empty(), "catalog should not be empty");
        let with_device = catalog
            .iter()
            .filter(|m| !m.runtime.device_type.is_empty())
            .count();
        eprintln!(
            "[test] {} of {} have a device_type; sample: {:?}",
            with_device,
            catalog.len(),
            catalog.iter().take(3).map(|m| (&m.name, &m.runtime.device_type)).collect::<Vec<_>>()
        );
        assert!(with_device > 0, "expected at least one model with a device_type");

        // Install-compatibility: on a WebGPU-only GPU (Mac M3), vision-language models are
        // flagged with a compatibility WARNING (still installable, UI badges them), while
        // text models carry no warning.
        let qwen35_vision: Vec<_> = catalog
            .iter()
            .filter(|m| m.name.to_lowercase().contains("qwen3.5") && m.task == "vision-language-chat")
            .collect();
        if !qwen35_vision.is_empty() {
            assert!(
                qwen35_vision.iter().all(|m| m.incompatible),
                "qwen3.5 vision models should carry a compat warning on a WebGPU GPU: {:?}",
                qwen35_vision.iter().map(|m| (&m.name, m.incompatible)).collect::<Vec<_>>()
            );
        }
        // Text qwen variants (chat-completion) must have no warning.
        for m in catalog.iter().filter(|m| {
            m.name.to_lowercase().contains("qwen") && m.task == "chat-completion"
        }) {
            assert!(!m.incompatible, "text qwen should carry no warning: {}", m.name);
        }
        eprintln!(
            "[test] warned (installable-with-badge) models: {:?}",
            catalog.iter().filter(|m| m.incompatible).map(|m| &m.name).collect::<Vec<_>>()
        );
    }

    /// End-to-end through the trait against a real cached model. Ignored by default
    /// (needs the bundled runtime + a cached chat model). Run:
    ///   cargo test --lib backend::sdk::tests::sdk_backend_chat_roundtrip -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn sdk_backend_chat_roundtrip() {
        let home = dirs::home_dir().expect("home");
        let cache = home.join(".foundry").join("cache");
        let backend = SdkBackend::new(&cache, None).expect("new");
        backend.ensure_service().await.expect("service");
        // GPU providers are registered per process and are not registered by default on
        // Windows; without this a GPU model fails to load ("requires the CUDAExecutionProvider").
        register_all_eps(&backend).await;

        // Pick a cached non-qwen3.5 chat model (qwen3.5 generation hits the upstream WebGPU bug).
        let cached = backend.list_cached().await;
        let model = cached
            .iter()
            .find(|m| !m.model_id.to_lowercase().contains("qwen3.5"))
            .expect("a non-qwen3.5 cached model")
            .model_id
            .clone();
        eprintln!("[test] using model {model}");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let req = ChatStreamRequest {
            model: model.clone(),
            family: ModelFamily::Phi,
            messages: vec![user("In one short sentence, say hello.")],
            tools: None,
            use_native_tools: false,
            supports_reasoning: false,
            supports_reasoning_effort: false,
            reasoning_effort: "medium".into(),
            use_responses_api: false,
        };

        let outcome = backend.chat_stream(req, tx, cancel_rx).await;
        let mut text = String::new();
        while let Ok(chunk) = rx.try_recv() {
            text.push_str(&chunk);
        }
        eprintln!("[test] outcome={outcome:?} text={text:?}");
        assert!(outcome.error.is_none(), "chat errored: {:?}", outcome.error);
        assert!(!text.trim().is_empty(), "expected non-empty streamed text");
    }

    /// Models that are known to load but fail *generation* on the current SDK runtime
    /// (e.g. qwen3.5's onnxruntime-genai WebGPU validation bug). A failure here is tolerated
    /// rather than red — but any OTHER model failing is a real regression.
    fn is_known_incompatible(model_id: &str) -> bool {
        model_id.to_lowercase().contains("qwen3.5")
    }

    /// Prompt EVERY cached model through the SDK backend and assert each responds — except
    /// known-incompatible models (tolerated). Catches regressions where a working model breaks.
    /// Ignored by default (needs the SDK runtime + cached models). Run:
    ///   cargo test --lib backend::sdk::tests::sdk_backend_prompts_every_cached_model -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn sdk_backend_prompts_every_cached_model() {
        let home = dirs::home_dir().expect("home");
        let cache = home.join(".foundry").join("cache");
        let backend = SdkBackend::new(&cache, None).expect("new");
        backend.ensure_service().await.expect("service");
        // GPU providers are registered per process and are not registered by default on
        // Windows; without this a GPU model fails to load ("requires the CUDAExecutionProvider").
        register_all_eps(&backend).await;

        let mut cached = backend.list_cached().await;
        // Optional substring filter (e.g. `PLUGABLE_TEST_MODEL_FILTER=qwen3.5`) to re-run a subset.
        if let Ok(filter) = std::env::var("PLUGABLE_TEST_MODEL_FILTER") {
            let filter = filter.to_lowercase();
            cached.retain(|m| m.model_id.to_lowercase().contains(&filter));
        }
        assert!(!cached.is_empty(), "no cached models to test");
        eprintln!("[test] prompting {} cached model(s)", cached.len());

        let mut successes = 0usize;
        let mut tolerated: Vec<String> = Vec::new();
        let mut unexpected: Vec<String> = Vec::new();

        for m in &cached {
            let id = m.model_id.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let req = ChatStreamRequest {
                model: id.clone(),
                family: ModelFamily::Generic,
                messages: vec![user("In one short sentence, say hello.")],
                tools: None,
                use_native_tools: false,
                supports_reasoning: false,
                supports_reasoning_effort: false,
                reasoning_effort: "medium".into(),
                use_responses_api: false,
            };
            let started = std::time::Instant::now();
            let outcome = backend.chat_stream(req, tx, cancel_rx).await;
            let elapsed_secs = started.elapsed().as_secs_f32();
            let mut text = String::new();
            while let Ok(chunk) = rx.try_recv() {
                text.push_str(&chunk);
            }
            let ok = outcome.error.is_none() && !text.trim().is_empty();
            eprintln!(
                "[test]   {id}: {} (chars={}, elapsed={elapsed_secs:.0}s, err={:?})",
                if ok { "OK" } else { "FAILED" },
                text.trim().len(),
                outcome.error.as_ref().map(|e| e.message())
            );
            if ok {
                successes += 1;
            } else if is_known_incompatible(&id) {
                tolerated.push(id);
            } else {
                unexpected.push(format!(
                    "{id}: err={:?} text={:?}",
                    outcome.error.as_ref().map(|e| e.message()),
                    text.trim()
                ));
            }
        }

        eprintln!(
            "[test] summary: {successes} ok, {} tolerated(known-incompatible) {:?}, {} unexpected",
            tolerated.len(),
            tolerated,
            unexpected.len()
        );
        assert!(successes > 0, "no cached model produced a response");
        assert!(
            unexpected.is_empty(),
            "models that should work failed to respond: {unexpected:?}"
        );
    }

    /// Report the execution providers, download and register any that are not yet
    /// registered, and return what the catalog then offers.
    async fn register_all_eps(backend: &SdkBackend) {
        // The production method, so the tests exercise the code the app runs.
        let summary = backend
            .register_execution_providers(
                |ep, percent| {
                    if percent >= 100.0 {
                        eprintln!("[gpu] {ep} downloaded");
                    }
                },
                Arc::new(AtomicBool::new(false)),
            )
            .await;
        eprintln!("[gpu] execution providers: {summary:?}");
    }

    /// GPU test box: with the GPU execution providers registered, the catalog must offer
    /// GPU model variants. On a machine where Foundry cannot use the GPU every variant is
    /// CPU and this fails, which is the signal the box is not testing what it should.
    /// Run: cargo test --lib backend::sdk::tests::sdk_backend_gpu_execution_providers -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn sdk_backend_gpu_execution_providers() {
        let home = dirs::home_dir().expect("home");
        let backend = SdkBackend::new(&home.join(".foundry").join("cache"), None).expect("new");
        backend.ensure_service().await.expect("service");
        register_all_eps(&backend).await;

        let catalog = backend.list_catalog().await;
        let gpu: Vec<_> = catalog.iter().filter(|m| m.runtime.device_type == "GPU").collect();
        let mut providers: Vec<_> = gpu.iter().map(|m| m.runtime.execution_provider.clone()).collect();
        providers.sort();
        providers.dedup();
        eprintln!(
            "[gpu] {} of {} catalog models are GPU variants; execution providers: {providers:?}",
            gpu.len(),
            catalog.len()
        );
        assert!(!gpu.is_empty(), "the catalog offers no GPU model variants");
    }

    /// GPU test box: download one small GPU chat model so the round-trip tests have
    /// something to run. PLUGABLE_TEST_MODEL overrides the choice.
    /// Run: cargo test --lib backend::sdk::tests::sdk_backend_download_gpu_chat_model -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn sdk_backend_download_gpu_chat_model() {
        let home = dirs::home_dir().expect("home");
        let backend = SdkBackend::new(&home.join(".foundry").join("cache"), None).expect("new");
        backend.ensure_service().await.expect("service");
        register_all_eps(&backend).await;

        let name = match std::env::var("PLUGABLE_TEST_MODEL") {
            Ok(n) if !n.is_empty() => n,
            _ => backend
                .list_catalog()
                .await
                .iter()
                .filter(|m| {
                    m.runtime.device_type == "GPU"
                        && m.task == "chat-completion"
                        && !m.name.to_lowercase().contains("qwen3.5")
                })
                .min_by_key(|m| m.file_size_mb)
                .map(|m| m.name.clone())
                .expect("a GPU chat-completion model in the catalog"),
        };
        eprintln!("[gpu] downloading {name}");
        let started = std::time::Instant::now();
        let mut last = -10.0_f64;
        backend
            .download(
                &name,
                Box::new(move |p| {
                    if p - last >= 10.0 || p >= 100.0 {
                        eprintln!("[gpu] download {p:.0}%");
                        last = p;
                    }
                }),
                None,
            )
            .await
            .expect("download");
        eprintln!("[gpu] downloaded {name} in {:.0}s", started.elapsed().as_secs_f64());
        assert!(
            backend.list_cached().await.iter().any(|m| m.model_id == name),
            "{name} is not in the cache after download"
        );
    }
}
