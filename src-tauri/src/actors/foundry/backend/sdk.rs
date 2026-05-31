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
use std::sync::{Mutex, OnceLock};

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

use super::{
    BackendError, ChatStreamOutcome, ChatStreamRequest, ConnectionInfo, FoundryBackend, ProgressSink,
    RawModel,
};
use crate::protocol::{CachedModel, CatalogModel, CatalogModelRuntime, ChatMessage, ModelFamily};

/// Key used by the version-scoped incompatible-models blocklist under the SDK runtime.
/// Distinct from the CLI's `foundry --version` (e.g. `0.8.119`), so a model blocklisted on
/// the old runtime is automatically re-evaluated here.
pub const FOUNDRY_SDK_VERSION_KEY: &str = "sdk-1.2.0";

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

    /// Resolve a `Model` handle by its unique id (e.g. `qwen3.5-0.8b-generic-gpu:2`).
    async fn model(&self, id: &str) -> Result<std::sync::Arc<foundry_local_sdk::Model>, String> {
        self.manager
            .catalog()
            .get_model_variant(id)
            .await
            .map_err(|e| format!("get_model_variant('{id}') failed: {e}"))
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
        let (registered_eps, valid_eps) = match self.manager.discover_eps() {
            Ok(eps) => {
                let valid: Vec<String> = eps.iter().map(|e| e.name.clone()).collect();
                let registered: Vec<String> = eps
                    .iter()
                    .filter(|e| e.is_registered)
                    .map(|e| e.name.clone())
                    .collect();
                (registered, valid)
            }
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
            .manager
            .discover_eps()
            .ok()
            .map(|eps| eps.iter().any(|e| e.name.to_lowercase().contains("webgpu")))
            .unwrap_or(false);
        match self.manager.catalog().get_models().await {
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
                        Some("May not run on this Mac — vision models can fail on the WebGPU runtime (upstream onnxruntime-genai bug)".to_string())
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
        m.load().await.map_err(|e| BackendError::Server(e.to_string()))
    }

    async fn unload(&self, model: &str) -> Result<(), String> {
        let m = self.model(model).await?;
        m.unload().await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn download(&self, model: &str, progress: ProgressSink) -> Result<(), String> {
        let m = self.model(model).await?;
        m.download(Some(progress)).await.map_err(|e| e.to_string())
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
        let mut outcome = ChatStreamOutcome::default();
        let mut cancel = cancel;

        // Resolve + ensure loaded.
        let model = match self.model(&req.model).await {
            Ok(m) => m,
            Err(e) => {
                outcome.error = Some(BackendError::Server(e));
                return outcome;
            }
        };
        if !model.is_loaded().await.unwrap_or(false) {
            if let Err(e) = model.load().await {
                outcome.error = Some(BackendError::Server(e.to_string()));
                return outcome;
            }
        }

        // Build the client with family-specific tuning (mirrors request_builder.rs).
        // NOTE: the SDK chat builder exposes no `reasoning_effort` (Phi/Generic) or
        // `repetition_penalty` (Granite); those tuning hints are dropped on this backend.
        let mut client = model.create_chat_client();
        match req.family {
            ModelFamily::GptOss => {
                client = client.max_tokens(16384).temperature(0.7);
            }
            ModelFamily::Phi => {
                if req.supports_reasoning && req.supports_reasoning_effort {
                    client = client.max_tokens(8192);
                } else {
                    client = client.max_tokens(16384);
                }
            }
            ModelFamily::Gemma => {
                client = client.max_tokens(8192).temperature(0.7).top_k(40);
            }
            ModelFamily::Granite => {
                client = client.max_tokens(8192).temperature(0.7);
            }
            ModelFamily::Generic => {
                if req.supports_reasoning && req.supports_reasoning_effort {
                    client = client.max_tokens(8192);
                } else {
                    client = client.max_tokens(16384);
                }
            }
        }

        // Map messages and tools.
        let messages = map_messages(&req.messages);
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
                outcome.error = Some(BackendError::Server(e.to_string()));
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
                            outcome.error = Some(BackendError::Server(e.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ChatMessage;

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

        let cached = backend.list_cached().await;
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
            let outcome = backend.chat_stream(req, tx, cancel_rx).await;
            let mut text = String::new();
            while let Ok(chunk) = rx.try_recv() {
                text.push_str(&chunk);
            }
            let ok = outcome.error.is_none() && !text.trim().is_empty();
            eprintln!(
                "[test]   {id}: {} (chars={}, err={:?})",
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
}
