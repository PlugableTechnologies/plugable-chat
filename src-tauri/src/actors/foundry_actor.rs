use crate::crash_handler::SuppressCrashDialogGuard;
use crate::is_verbose_logging_enabled;
use crate::protocol::{
    CachedModel, CatalogModel, ChatMessage, FoundryMsg, FoundryServiceStatus, ModelFamily,
    ModelInfo, ModelState, OpenAITool, ParsedToolCall, ReasoningFormat, ToolFormat,
};
use crate::app_state::{GpuResourceGuard, LoggingPersistence, SettingsState};
use crate::settings;
use serde::Deserialize;
use crate::settings::ChatFormatName;
use crate::tool_adapters::parse_combined_tool_name;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

// =============================================================================
// GPU EMBEDDING DISABLED - The following ort imports are commented out.
// To re-enable GPU embeddings, uncomment these imports and the corresponding
// code in ensure_gpu_embedding_model_loaded(), plus the ort dependencies in Cargo.toml.
// =============================================================================
// use fastembed::ExecutionProviderDispatch;
// #[cfg(target_os = "macos")]
// use ort::execution_providers::CoreMLExecutionProvider;
// #[cfg(not(target_os = "macos"))]
// use ort::execution_providers::{CUDAExecutionProvider, DirectMLExecutionProvider};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::process::Command;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{sleep, timeout};

/// Target embedding dimension (must match LanceDB schema)
const EMBEDDING_DIM: usize = 768;

/// Default fallback model to use when no model is specified or when errors occur.
/// This matches the phi-4-mini-instruct model that is auto-downloaded on first launch.
const DEFAULT_FALLBACK_MODEL: &str = "phi-4-mini-instruct";

/// Find the foundry CLI executable, checking PATH first then common installation locations.
/// This provides a fallback for production builds where PATH may not include the foundry binary.
fn find_foundry_binary() -> String {
    // First try PATH using which/where (will work after fix_macos_path_env() on macOS, or natively on Windows)
    #[cfg(windows)]
    let which_result = std::process::Command::new("where.exe")
        .arg("foundry")
        .output();

    #[cfg(not(windows))]
    let which_result = std::process::Command::new("which")
        .arg("foundry")
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

/// Accumulator for OpenAI-style streaming tool calls.
///
/// In the OpenAI streaming format, tool calls arrive incrementally:
/// - First chunk contains `id`, `type`, and `function.name`
/// - Subsequent chunks contain `function.arguments` fragments
/// - Multiple tool calls are indexed by their `index` field
#[derive(Default)]
struct StreamingToolCalls {
    /// Map of index -> (id, name, accumulated_arguments)
    calls: HashMap<usize, (String, String, String)>,
}

impl StreamingToolCalls {
    /// Process a delta.tool_calls array from a streaming chunk
    fn process_delta(&mut self, tool_calls: &[Value]) {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            let entry = self
                .calls
                .entry(index)
                .or_insert_with(|| (String::new(), String::new(), String::new()));

            // First chunk has id/name
            if let Some(id) = tc["id"].as_str() {
                entry.0 = id.to_string();
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                entry.1 = name.to_string();
            }
            // Accumulate arguments (streamed incrementally)
            if let Some(args) = tc["function"]["arguments"].as_str() {
                entry.2.push_str(args);
            }
        }
    }

    /// Check if any tool calls have been accumulated
    fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Convert accumulated tool calls to ParsedToolCall format
    fn into_parsed_calls(self) -> Vec<ParsedToolCall> {
        let mut result = Vec::new();

        // Sort by index to maintain order
        let mut indexed: Vec<_> = self.calls.into_iter().collect();
        indexed.sort_by_key(|(idx, _)| *idx);

        for (_index, (tool_call_id, name, arguments_str)) in indexed {
            // Skip entries without a name (incomplete)
            if name.is_empty() {
                continue;
            }

            // Parse the accumulated arguments JSON
            let arguments = if arguments_str.is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&arguments_str).unwrap_or_else(|e| {
                    println!(
                        "[StreamingToolCalls] Failed to parse arguments for {}: {}",
                        name, e
                    );
                    println!("[StreamingToolCalls] Raw arguments: {}", arguments_str);
                    Value::Object(serde_json::Map::new())
                })
            };

            // Parse the tool name (may be "server___tool" format)
            let (server, tool) = parse_combined_tool_name(&name);

            // Build raw representation for display
            let raw = format!(
                "<tool_call>{{\"name\": \"{}\", \"arguments\": {}}}</tool_call>",
                name,
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            );

            // Include the native tool call ID if present
            let id = if tool_call_id.is_empty() {
                None
            } else {
                Some(tool_call_id)
            };

            result.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw,
                id,
            });
        }

        result
    }
}

/// Build a chat request body with model-family-specific parameters
fn build_chat_request_body(
    model: &str,
    family: ModelFamily,
    messages: &[ChatMessage],
    tools: &Option<Vec<OpenAITool>>,
    use_native_tools: bool,
    supports_reasoning: bool,
    supports_reasoning_effort: bool,
    reasoning_effort: &str,
    use_responses_api: bool,
) -> Value {
    let mut body = if use_responses_api {
        json!({
            "model": model,
            "input": map_messages_to_responses_input(messages),
            "stream": true,
        })
    } else {
        json!({
            "model": model,
            "messages": messages,
            "stream": true,
        })
    };

    // Note: EP (execution provider) parameter is not passed to completions
    // as it didn't work reliably. Foundry will auto-select the best EP.

    // Add model-family-specific parameters
    match family {
        ModelFamily::GptOss => {
            // GPT-OSS models: standard OpenAI-compatible parameters
            body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                json!(16384);
            body["temperature"] = json!(0.7);

            if use_native_tools {
                if let Some(tool_list) = tools {
                    body["tools"] = json!(tool_list);
                }
            }
        }
        ModelFamily::Phi => {
            // Phi models: may support reasoning_effort
            if supports_reasoning && supports_reasoning_effort {
                println!(
                    "[FoundryActor] Phi model with reasoning, using effort: {}",
                    reasoning_effort
                );
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(8192);
                body["reasoning_effort"] = json!(reasoning_effort);
                // Note: Reasoning models typically don't use tools in the same request
            } else {
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(16384);
                if use_native_tools {
                    if let Some(tool_list) = tools {
                        body["tools"] = json!(tool_list);
                    }
                }
            }
        }
        ModelFamily::Gemma => {
            // Gemma models: support temperature and top_k
            body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                json!(8192);
            body["temperature"] = json!(0.7);
            // Gemma supports top_k which is useful for controlling randomness
            body["top_k"] = json!(40);

            if use_native_tools {
                // Gemma may use a different tool format, but Foundry handles this
                if let Some(tool_list) = tools {
                    body["tools"] = json!(tool_list);
                }
            }
        }
        ModelFamily::Granite => {
            // IBM Granite models: support repetition_penalty
            body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                json!(8192);
            body["temperature"] = json!(0.7);
            // Granite models benefit from repetition penalty
            body["repetition_penalty"] = json!(1.05);

            if supports_reasoning {
                // Granite reasoning models use <|thinking|> tags internally
                println!("[FoundryActor] Granite model with reasoning support");
            }

            if use_native_tools {
                if let Some(tool_list) = tools {
                    body["tools"] = json!(tool_list);
                }
            }
        }
        ModelFamily::Generic => {
            // Generic/unknown models: use safe defaults
            if supports_reasoning && supports_reasoning_effort {
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(8192);
                body["reasoning_effort"] = json!(reasoning_effort);
            } else {
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(16384);
                if use_native_tools {
                    if let Some(tool_list) = tools {
                        body["tools"] = json!(tool_list);
                    }
                }
            }
        }
    }

    body
}

/// Convert OpenAI chat messages into Responses API input blocks (text-only)
fn map_messages_to_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| {
            json!({
                "role": msg.role,
                "content": [
                    {
                        "type": "text",
                        "text": msg.content
                    }
                ]
            })
        })
        .collect()
}

/// Extract streamed text from either Chat Completions or Responses API payloads.
fn extract_stream_text(json: &Value, use_responses_api: bool) -> Option<String> {
    // Chat Completions delta string form
    if let Some(content) = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("content"))
    {
        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        } else if let Some(parts) = content.as_array() {
            let mut buf = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(text);
                } else if let Some(text) = part.as_str() {
                    buf.push_str(text);
                }
            }
            if !buf.is_empty() {
                return Some(buf);
            }
        }
    }

    if use_responses_api {
        // Responses API event shapes (best-effort)
        let candidates = [
            json.get("output_text_delta")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            json.pointer("/delta/output_text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            json.pointer("/response/output_text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ];

        for cand in candidates {
            if let Some(text) = cand {
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }

        if let Some(delta_obj) = json.get("delta") {
            if let Some(text) = delta_obj
                .get("output_text_delta")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(text.to_string());
            }

            if let Some(output_arr) = delta_obj.get("output").and_then(|v| v.as_array()) {
                let mut buf = String::new();
                for entry in output_arr {
                    if let Some(content_arr) = entry.get("content").and_then(|c| c.as_array()) {
                        for part in content_arr {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                buf.push_str(text);
                            }
                        }
                    }
                }
                if !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_messages_to_responses_input_wraps_text() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "hi there".to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let input = map_messages_to_responses_input(&messages);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["text"], "hi there");
    }

    #[test]
    fn extract_stream_text_handles_chat_delta_string() {
        let payload = json!({"choices":[{"delta":{"content":"hello"}}]});
        let extracted = extract_stream_text(&payload, false);
        assert_eq!(extracted.as_deref(), Some("hello"));
    }

    #[test]
    fn extract_stream_text_handles_responses_delta() {
        let payload = json!({"type":"response.output_text.delta","output_text_delta":"hello-resp"});
        let extracted = extract_stream_text(&payload, true);
        assert_eq!(extracted.as_deref(), Some("hello-resp"));
    }
}

/// Result of parsing `foundry service status` output
struct ServiceStatus {
    port: Option<u16>,
    registered_eps: Vec<String>,
    valid_eps: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FoundryModel {
    id: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FoundryModelsResponse {
    data: Vec<FoundryModel>,
}

pub struct ModelGatewayActor {
    foundry_msg_rx: mpsc::Receiver<FoundryMsg>,
    port: Option<u16>,
    model_id: Option<String>,
    /// Current state machine state for model management
    model_state: ModelState,
    available_models: Vec<String>,
    model_info: Vec<ModelInfo>,
    app_handle: AppHandle,
    /// GPU-accelerated embedding model for background RAG indexing
    shared_gpu_embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    /// CPU-only embedding model for search during chat (avoids LLM eviction)
    shared_cpu_embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    /// Execution Providers successfully registered by Foundry
    registered_eps: Vec<String>,
    /// All valid Execution Providers available on this system
    valid_eps: Vec<String>,
    /// Shared logging persistence (prompts, tools)
    logging_persistence: Arc<LoggingPersistence>,
    /// Shared HTTP client for all Foundry API requests (connection pooling)
    http_client: reqwest::Client,
    /// GPU resource guard for serializing GPU operations (prevents memory contention)
    gpu_guard: Arc<GpuResourceGuard>,
}

impl ModelGatewayActor {
    pub fn new(
        foundry_msg_rx: mpsc::Receiver<FoundryMsg>,
        app_handle: AppHandle,
        shared_gpu_embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
        shared_cpu_embedding_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
        logging_persistence: Arc<LoggingPersistence>,
        gpu_guard: Arc<GpuResourceGuard>,
    ) -> Self {
        // Create HTTP client with connection pooling optimized for local service
        let http_client = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(2)
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            foundry_msg_rx,
            port: None,
            model_id: None,
            model_state: ModelState::Initializing,
            available_models: Vec::new(),
            model_info: Vec::new(),
            app_handle,
            shared_gpu_embedding_model,
            shared_cpu_embedding_model,
            registered_eps: Vec::new(),
            valid_eps: Vec::new(),
            logging_persistence,
            http_client,
            gpu_guard,
        }
    }

    fn model_supports_responses(model_id: &str) -> bool {
        let lower = model_id.to_lowercase();
        // Heuristic: gpt-oss models expose /v1/responses in Foundry Local (see model card)
        lower.contains("gpt-oss")
    }

    /// Check if Foundry reports GPU execution providers are available
    #[allow(dead_code)] // May be useful for future GPU detection logic
    fn has_gpu_eps(valid_eps: &[String]) -> bool {
        for ep in valid_eps {
            let ep_lower = ep.to_lowercase();
            if ep_lower.contains("cuda")
                || ep_lower.contains("tensorrt")
                || ep_lower.contains("coreml")
                || ep_lower.contains("openvino")
                || ep_lower.contains("directml")
                || ep_lower.contains("rocm")
            {
                return true;
            }
        }
        false
    }

    /// Pre-warm the HTTP connection pool by making a lightweight request to Foundry.
    /// This ensures the first chat completion doesn't pay the connection establishment cost.
    async fn prewarm_http_connection(&self) {
        if let Some(port) = self.port {
            let url = format!("http://127.0.0.1:{}/openai/status", port);
            println!("FoundryActor: Pre-warming HTTP connection to {}", url);
            let start = std::time::Instant::now();
            match self
                .http_client
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(_) => {
                    println!(
                        "FoundryActor: HTTP connection pre-warmed in {:?}",
                        start.elapsed()
                    );
                }
                Err(e) => {
                    println!(
                        "FoundryActor: ⚠️ Failed to pre-warm connection (non-fatal): {:?}",
                        e
                    );
                    println!(
                        "FoundryActor: Pre-warm connection error details - url: {}, is_timeout: {}, is_connect: {}",
                        url, e.is_timeout(), e.is_connect()
                    );
                }
            }
        }
    }

    /// Pre-load a model into VRAM to reduce time-to-first-token.
    /// This is fire-and-forget - we don't wait for completion.
    /// The model load happens in the background and will be ready for the first chat.
    /// 
    /// IMPORTANT: Acquires GPU mutex to prevent contention with embedding operations.
    fn prewarm_model_in_background(&self, model_name: String) {
        if let Some(port) = self.port {
            let client = self.http_client.clone();
            let app_handle = self.app_handle.clone();
            let gpu_guard = self.gpu_guard.clone();
            
            // Spawn a background task to load the model
            tokio::spawn(async move {
                // Acquire GPU mutex to prevent contention with embedding operations
                let _gpu_lock = gpu_guard.mutex.lock().await;
                *gpu_guard.current_operation.write().await = Some(format!("Pre-warming LLM: {}", model_name));
                
                // Emit GPU status event
                let _ = app_handle.emit("gpu-status", json!({
                    "operation": "prewarm",
                    "model": &model_name,
                    "status": "started"
                }));
                
                let encoded_name = urlencoding::encode(&model_name);
                let url = format!(
                    "http://127.0.0.1:{}/openai/load/{}?ttl=0",
                    port, encoded_name
                );
                println!(
                    "FoundryActor: 🔥 Pre-warming model {} (loading into VRAM)...",
                    model_name
                );
                let start = std::time::Instant::now();
                
                // Emit status so UI knows what's happening
                let _ = app_handle.emit(
                    "chat-stream-status",
                    serde_json::json!({
                        "phase": "prewarming",
                        "message": format!("Loading {} into memory...", model_name)
                    }),
                );
                
                match client
                    .get(&url)
                    .timeout(Duration::from_secs(120)) // 2 minute timeout for loading
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        let elapsed = start.elapsed();
                        println!(
                            "FoundryActor: ✅ Model {} pre-warmed in {:?}",
                            model_name,
                            elapsed
                        );
                        // Clear the loading status in UI
                        let _ = app_handle.emit(
                            "chat-stream-status",
                            serde_json::json!({
                                "phase": "prewarm_complete",
                                "message": format!("{} ready", model_name)
                            }),
                        );
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        println!(
                            "FoundryActor: ⚠️ Model pre-warm returned status {} for {}: {}",
                            status, model_name, body
                        );
                        println!(
                            "FoundryActor: Pre-warm request URL was: {}",
                            url
                        );
                    }
                    Err(e) => {
                        // This is non-fatal - the model will be loaded on first use
                        println!(
                            "FoundryActor: ⚠️ Model pre-warm failed (non-fatal): {} - {:?}",
                            model_name, e
                        );
                        println!(
                            "FoundryActor: Pre-warm error details - kind: {:?}, url: {}, is_timeout: {}, is_connect: {}",
                            e.status(), url, e.is_timeout(), e.is_connect()
                        );
                    }
                }
                
                // Clear GPU operation status and emit completion
                *gpu_guard.current_operation.write().await = None;
                let _ = app_handle.emit("gpu-status", json!({
                    "operation": "prewarm",
                    "model": &model_name,
                    "status": "completed"
                }));
            });
        }
    }

    // ============ Model State Machine Methods ============

    /// Transition to a new model state and emit event to frontend
    fn transition_model_state(&mut self, new_state: ModelState) {
        use std::io::Write;
        let old_state = std::mem::replace(&mut self.model_state, new_state.clone());
        
        println!(
            "[ModelStateMachine] Transition: {:?} -> {:?}",
            old_state, new_state
        );
        let _ = std::io::stdout().flush();
        
        self.emit_model_state_changed();
    }

    /// Emit the current model state to the frontend
    fn emit_model_state_changed(&self) {
        use std::time::{SystemTime, UNIX_EPOCH};
        
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        let event_payload = serde_json::json!({
            "state": &self.model_state,
            "timestamp": timestamp
        });
        
        if let Err(e) = self.app_handle.emit("model-state-changed", event_payload) {
            eprintln!("[ModelStateMachine] Failed to emit state change: {:?}", e);
        }
    }

    /// Transition to Ready state with the given model
    fn transition_to_ready(&mut self, model_id: String) {
        // Also update the legacy model_id field for compatibility
        self.model_id = Some(model_id.clone());
        self.transition_model_state(ModelState::Ready { model_id });
    }

    /// Transition to Error state
    fn transition_to_error(&mut self, message: String, last_model: Option<String>) {
        self.transition_model_state(ModelState::Error { message, last_model });
    }

    /// Transition to ServiceUnavailable state
    fn transition_to_service_unavailable(&mut self, message: String) {
        self.transition_model_state(ModelState::ServiceUnavailable { message });
    }

    /// Transition to ServiceRestarting state
    fn transition_to_service_restarting(&mut self) {
        self.transition_model_state(ModelState::ServiceRestarting);
    }

    /// Transition to Reconnecting state
    fn transition_to_reconnecting(&mut self) {
        self.transition_model_state(ModelState::Reconnecting);
    }

    /// Lazily load the GPU embedding model on demand for RAG indexing.
    /// 
    /// NOTE: GPU EMBEDDING DISABLED - This function now always returns an error.
    /// The callers (rag.rs, database.rs) will fall back to CPU embedding.
    /// 
    /// To re-enable GPU embeddings:
    /// 1. Uncomment the ort dependencies in Cargo.toml
    /// 2. Uncomment the ort imports at the top of this file
    /// 3. Restore this function's original implementation (see git history or comments below)
    #[allow(dead_code)]
    async fn ensure_gpu_embedding_model_loaded(&self) -> Result<Arc<TextEmbedding>, String> {
        // GPU embedding is disabled - always return error to trigger CPU fallback
        println!("FoundryActor: GPU embedding is disabled, falling back to CPU");
        Err("GPU embedding is disabled (ONNX dependency removed). Using CPU embedding.".to_string())
        
        // =========================================================================
        // ORIGINAL GPU EMBEDDING CODE - Uncomment to restore GPU embedding support
        // =========================================================================
        // 
        // // NOTE: We intentionally do NOT cache the GPU embedding model.
        // // Previously, caching caused hangs because:
        // // 1. GPU model was loaded and cached
        // // 2. LLM prewarm evicted the embedding model's GPU resources  
        // // 3. Calling embed() on the stale cached model hung indefinitely
        // // By always loading fresh, we ensure GPU resources are properly allocated.
        //
        // // Need to load the model
        // println!("╔══════════════════════════════════════════════════════════════╗");
        // println!("║  LOADING GPU EMBEDDING MODEL (on-demand for RAG)            ║");
        // println!("╚══════════════════════════════════════════════════════════════╝");
        //
        // let start = std::time::Instant::now();
        // 
        // // Emit progress event
        // let _ = self.app_handle.emit("embedding-init-progress", serde_json::json!({
        //     "message": "Loading GPU embedding model for RAG indexing...",
        //     "is_complete": false
        // }));
        //
        // // Build GPU execution providers
        // // Use catch_unwind to handle panics from ORT initialization (e.g., missing DLLs on Windows)
        // // Also suppress the crash dialog since we handle failures gracefully
        // let gpu_result = tokio::task::spawn_blocking(move || {
        //     // Suppress crash dialog for ORT initialization - this is optional and we handle failures gracefully
        //     let _guard = SuppressCrashDialogGuard::new();
        //     
        //     std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        //         let mut options = InitOptions::new(EmbeddingModel::BGEBaseENV15);
        //         options.show_download_progress = true;
        //         
        //         let mut eps: Vec<ExecutionProviderDispatch> = Vec::new();
        //         
        //         // On macOS, use CoreML (Metal GPU acceleration)
        //         #[cfg(target_os = "macos")]
        //         {
        //             eps.push(CoreMLExecutionProvider::default().into());
        //             println!("FoundryActor: GPU embedding model using CoreML (Metal)");
        //         }
        //         
        //         // On Windows/Linux, try CUDA and DirectML
        //         #[cfg(not(target_os = "macos"))]
        //         {
        //             // Try CUDA first, then DirectML
        //             eps.push(CUDAExecutionProvider::default().into());
        //             eps.push(DirectMLExecutionProvider::default().into());
        //             println!("FoundryActor: GPU embedding model trying CUDA/DirectML");
        //         }
        //         
        //         if !eps.is_empty() {
        //             options.execution_providers = eps;
        //         }
        //
        //         TextEmbedding::try_new(options)
        //     }))
        // })
        // .await
        // .map_err(|e| format!("GPU embedding model init task panicked: {}", e))?;
        //
        // // Handle panic from catch_unwind
        // let gpu_result = match gpu_result {
        //     Ok(inner_result) => inner_result,
        //     Err(panic_payload) => {
        //         let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
        //             s.to_string()
        //         } else if let Some(s) = panic_payload.downcast_ref::<String>() {
        //             s.clone()
        //         } else {
        //             "Unknown panic".to_string()
        //         };
        //         return Err(format!("ONNX Runtime panic (missing DLL?): {}", panic_msg));
        //     }
        // };
        // 
        // let gpu_result = gpu_result
        //     .map_err(|e| format!("Failed to load GPU embedding model: {}", e))?;
        //
        // let elapsed = start.elapsed();
        // println!(
        //     "FoundryActor: GPU embedding model loaded in {:.2}s",
        //     elapsed.as_secs_f64()
        // );
        //
        // // NOTE: We intentionally do NOT cache the model to avoid stale GPU state issues.
        // // Each RAG indexing operation gets a fresh model with properly allocated GPU resources.
        // let model = Arc::new(gpu_result);
        //
        // // Emit completion
        // let _ = self.app_handle.emit("embedding-init-progress", serde_json::json!({
        //     "message": "GPU embedding model loaded",
        //     "is_complete": true
        // }));
        //
        // Ok(model)
    }

    pub async fn run(mut self) {
        println!("Initializing Foundry Local Manager via CLI...");
        
        // Emit initial Initializing state
        self.emit_model_state_changed();

        // Try to start the service or ensure it's running
        if let Err(e) = self.ensure_service_running().await {
            println!(
                "Warning: Failed to ensure Foundry service is running: {}",
                e
            );
            self.transition_to_service_unavailable(format!("Failed to start service: {}", e));
        }

        // Try to get the port and EPs with retries
        // Foundry may take time to start up, so we retry with exponential backoff
        // 8 retries × 1.5x backoff (2s initial, 10s max) = ~45 seconds max wait
        let connected = self.update_connection_info_with_retry(8, Duration::from_secs(2))
            .await;
        
        // If connection failed and we're still in Initializing, transition to ServiceUnavailable
        if !connected && matches!(self.model_state, ModelState::Initializing) {
            self.transition_to_service_unavailable("Could not connect to Foundry service".to_string());
        }

        // Pre-warm the HTTP connection pool so first chat request doesn't pay connection cost
        self.prewarm_http_connection().await;

        // Pre-warm the SELECTED model (from settings or fallback), NOT the first model!
        // The model_id is set by update_connection_info() which respects settings.
        if let Some(selected_model) = self.model_id.clone() {
            println!("FoundryActor: Pre-warming selected model: {}", selected_model);
            self.prewarm_model_in_background(selected_model);
        } else {
            println!("FoundryActor: No model selected yet, skipping pre-warm");
        }

        // Initialize CPU embedding model at startup for search during chat.
        // GPU embedding model is loaded on-demand when RAG indexing is requested,
        // to avoid GPU memory contention with the LLM at startup.
        println!("FoundryActor: Initializing CPU embedding model (BGE-Base-EN-v1.5)...");
        println!("FoundryActor: GPU embedding model will be loaded on-demand for RAG indexing");

        let shared_cpu_model = Arc::clone(&self.shared_cpu_embedding_model);
        let app_handle_clone = self.app_handle.clone();
        
        // Initialize CPU embedding model in a separate task to avoid blocking the actor message loop
        tokio::spawn(async move {
            let _ = app_handle_clone.emit("embedding-init-progress", json!({
                "message": "Initializing CPU embedding model...",
                "is_complete": false
            }));

            // Initialize CPU model (no GPU execution providers - pure CPU)
            // Use catch_unwind to handle panics from ORT initialization (e.g., missing DLLs on Windows)
            // Also suppress the crash dialog since this is an optional feature
            let cpu_result = tokio::task::spawn_blocking(move || {
                // Suppress crash dialog for ORT initialization - this is optional and we handle failures gracefully
                let _guard = SuppressCrashDialogGuard::new();
                
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut options = InitOptions::new(EmbeddingModel::BGEBaseENV15);
                    options.show_download_progress = true;
                    // Don't set any execution providers - defaults to CPU
                    println!("FoundryActor: CPU model - using CPU only (no GPU EPs configured)");
                    TextEmbedding::try_new(options)
                }))
            })
            .await;

            // Store CPU model result
            // Handle triple-nested Result from: spawn_blocking -> catch_unwind -> try_new
            match cpu_result {
                Ok(Ok(Ok(model))) => {
                    println!("FoundryActor: CPU embedding model loaded successfully");
                    let mut guard = shared_cpu_model.write().await;
                    *guard = Some(Arc::new(model));
                    let _ = app_handle_clone.emit("embedding-init-progress", json!({
                        "message": "CPU embedding model loaded (GPU model loads on-demand)",
                        "is_complete": true
                    }));
                }
                Ok(Ok(Err(e))) => {
                    println!("FoundryActor ERROR: ❌ Failed to load CPU embedding model: {:?}", e);
                    println!("FoundryActor: CPU embedding model load error details - check if the model file exists and is accessible");
                    let _ = app_handle_clone.emit("embedding-init-progress", json!({
                        "message": format!("Failed to load CPU embedding model: {}", e),
                        "is_complete": true,
                        "error": true
                    }));
                }
                Ok(Err(panic_payload)) => {
                    // ORT initialization panic - likely missing onnxruntime.dll on Windows
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic".to_string()
                    };
                    println!("FoundryActor ERROR: ❌ ONNX Runtime initialization panicked: {}", panic_msg);
                    println!("FoundryActor: This usually means onnxruntime.dll is missing on Windows.");
                    println!("FoundryActor: Embedding/search features will be unavailable.");
                    let _ = app_handle_clone.emit("embedding-init-progress", json!({
                        "message": format!("ONNX Runtime unavailable: {}. Embedding features disabled.", panic_msg),
                        "is_complete": true,
                        "error": true
                    }));
                }
                Err(e) => {
                    println!("FoundryActor ERROR: ❌ CPU embedding model init task failed: {:?}", e);
                    println!("FoundryActor: This may indicate an out-of-memory condition or incompatible hardware");
                    let _ = app_handle_clone.emit("embedding-init-progress", json!({
                        "message": "CPU embedding model initialization task failed",
                        "is_complete": true,
                        "error": true
                    }));
                }
            }
        });

        while let Some(msg) = self.foundry_msg_rx.recv().await {
            match msg {
                FoundryMsg::GetEmbedding { text, use_gpu, respond_to } => {
                    // Select the appropriate model based on use_gpu flag:
                    // - GPU model: For RAG indexing (background, can evict LLM)
                    // - CPU model: For search during chat (avoids LLM eviction)
                    let model_type = if use_gpu { "GPU" } else { "CPU" };
                    
                    // For GPU embeddings, acquire the GPU mutex to prevent contention
                    let _gpu_lock = if use_gpu {
                        *self.gpu_guard.current_operation.write().await = Some("GPU embedding inference".to_string());
                        Some(self.gpu_guard.mutex.lock().await)
                    } else {
                        None
                    };
                    
                    let model_guard = if use_gpu {
                        self.shared_gpu_embedding_model.read().await
                    } else {
                        self.shared_cpu_embedding_model.read().await
                    };
                    
                    if let Some(model) = model_guard.as_ref() {
                        let model_clone = Arc::clone(model);
                        let text_clone = text.clone();
                        drop(model_guard); // Release lock before blocking operation

                        println!(
                            "FoundryActor: Generating {} embedding (text len: {})",
                            model_type, text.len()
                        );
                        
                        let embed_start = std::time::Instant::now();

                        match tokio::task::spawn_blocking(move || {
                            model_clone.embed(vec![text_clone], None)
                        })
                        .await
                        {
                            Ok(Ok(embeddings)) => {
                                let embed_elapsed = embed_start.elapsed();
                                if let Some(embedding) = embeddings.into_iter().next() {
                                    println!(
                                        "FoundryActor: Generated {} embedding (dim: {}) in {:?}",
                                        model_type, embedding.len(), embed_elapsed
                                    );
                                    let _ = respond_to.send(embedding);
                                } else {
                                    println!("FoundryActor ERROR: Empty {} embedding result, using fallback", model_type);
                                    let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                                }
                            }
                            Ok(Err(e)) => {
                                println!("FoundryActor ERROR: ❌ {} embedding generation failed: {:?}", model_type, e);
                                println!("FoundryActor: Embedding error details - text_len: {}, model_type: {}", text.len(), model_type);
                                let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                            }
                            Err(e) => {
                                println!("FoundryActor ERROR: ❌ {} embedding task panicked: {:?}", model_type, e);
                                println!("FoundryActor: This may indicate an out-of-memory condition for {} model", model_type);
                                let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                            }
                        }
                    } else {
                        println!(
                            "FoundryActor WARNING: {} embedding model not loaded, using fallback",
                            model_type
                        );
                        let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                    }
                    
                    // Clear GPU operation status if we held the lock
                    if use_gpu {
                        *self.gpu_guard.current_operation.write().await = None;
                    }
                }
                FoundryMsg::RewarmCurrentModel { respond_to } => {
                    // Re-warm the currently selected LLM model after GPU-intensive operations
                    // (like RAG indexing that may have evicted the model from GPU memory)
                    if let Some(model_id) = &self.model_id {
                        println!("FoundryActor: Re-warming model after GPU operation: {}", model_id);
                        self.prewarm_model_in_background(model_id.clone());
                    } else {
                        println!("FoundryActor: No model selected, skipping re-warm");
                    }
                    let _ = respond_to.send(());
                }
                FoundryMsg::GetGpuEmbeddingModel { respond_to } => {
                    // Acquire GPU mutex to prevent contention with LLM operations
                    let _gpu_lock = self.gpu_guard.mutex.lock().await;
                    *self.gpu_guard.current_operation.write().await = Some("Loading GPU embedding model".to_string());
                    
                    let _ = self.app_handle.emit("gpu-status", json!({
                        "operation": "embed_load",
                        "status": "started"
                    }));
                    
                    // Lazy-load GPU embedding model on demand for RAG indexing
                    // This avoids GPU memory contention at startup and ensures
                    // the model is fresh when needed (not stale from LLM eviction)
                    let result = self.ensure_gpu_embedding_model_loaded().await;
                    
                    *self.gpu_guard.current_operation.write().await = None;
                    let _ = self.app_handle.emit("gpu-status", json!({
                        "operation": "embed_load",
                        "status": "completed"
                    }));
                    
                    let _ = respond_to.send(result);
                }
                FoundryMsg::GetModels { respond_to } => {
                    if self.port.is_none() {
                        // Only retry if service is unreachable (not if models list is empty)
                        self.update_connection_info_with_retry(3, Duration::from_secs(1))
                            .await;
                    }
                    let _ = respond_to.send(self.available_models.clone());
                }
                FoundryMsg::GetModelInfo { respond_to } => {
                    if self.port.is_none() {
                        // Only retry if service is unreachable (not if model info is empty)
                        self.update_connection_info_with_retry(3, Duration::from_secs(1))
                            .await;
                    }
                    let _ = respond_to.send(self.model_info.clone());
                }
                FoundryMsg::GetCachedModels { respond_to } => {
                    // Use REST API to get models (same as available_models)
                    // Convert to CachedModel format for compatibility
                    let cached: Vec<CachedModel> = self
                        .available_models
                        .iter()
                        .map(|model_id| CachedModel {
                            alias: model_id.clone(), // Use model_id as alias since REST API doesn't provide aliases
                            model_id: model_id.clone(),
                        })
                        .collect();
                    let _ = respond_to.send(cached);
                }
                FoundryMsg::SetModel {
                    model_id,
                    respond_to,
                } => {
                    // Get the current model before switching
                    let old_model_id = self.model_id.clone();
                    
                    // Skip if already on this model
                    if old_model_id.as_ref() == Some(&model_id) {
                        println!("FoundryActor: Model '{}' already selected, skipping switch", model_id);
                        let _ = respond_to.send(true);
                        continue;
                    }
                    
                    // Transition to SwitchingModel state
                    self.transition_model_state(ModelState::SwitchingModel {
                        from: old_model_id.clone(),
                        to: model_id.clone(),
                    });
                    
                    // Unload the previous model first to free VRAM before loading the new one.
                    // Without this, Foundry may try to load the new model while the old one is
                    // still in memory, causing VRAM pressure on systems with limited GPU memory.
                    if let (Some(old_model), Some(port)) = (&old_model_id, self.port) {
                        // Transition to UnloadingModel state
                        self.transition_model_state(ModelState::UnloadingModel {
                            model_id: old_model.clone(),
                            next_model: model_id.clone(),
                        });
                        
                        println!(
                            "FoundryActor: Unloading previous model '{}' before switching to '{}'",
                            old_model, model_id
                        );
                        
                        if let Err(e) = self.unload_model_impl(&self.http_client, port, old_model).await {
                            // Log but don't fail - the new model load may still succeed via Foundry's eviction
                            println!(
                                "FoundryActor: ⚠️ Failed to unload previous model (non-fatal): {}",
                                e
                            );
                        }
                    }
                    
                    // Transition to LoadingModel state
                    self.transition_model_state(ModelState::LoadingModel {
                        model_id: model_id.clone(),
                    });
                    
                    // Update model_id and transition to Ready
                    self.model_id = Some(model_id.clone());
                    self.transition_to_ready(model_id.clone());
                    
                    // Also emit the legacy event for backward compatibility
                    self.emit_model_selected(&model_id);
                    
                    // Pre-warm the model in the background to reduce time-to-first-token
                    self.prewarm_model_in_background(model_id);
                    
                    let _ = respond_to.send(true);
                }
                FoundryMsg::Chat {
                    model: requested_model,
                    chat_history_messages,
                    reasoning_effort,
                    native_tool_specs,
                    native_tool_calling_enabled,
                    chat_format_default,
                    chat_format_overrides,
                    respond_to,
                    mut stream_cancel_rx,
                } => {
                    // Clone GPU guard to avoid borrow conflicts with self
                    let gpu_guard = self.gpu_guard.clone();
                    
                    // Acquire GPU mutex for LLM inference - this serializes with embedding operations
                    let _gpu_lock = gpu_guard.mutex.lock().await;
                    *gpu_guard.current_operation.write().await = Some("LLM chat inference".to_string());
                    
                    let _ = self.app_handle.emit("gpu-status", json!({
                        "operation": "chat",
                        "status": "started"
                    }));
                    
                    // Emit status immediately when chat request is received
                    let _ = self.app_handle.emit(
                        "chat-stream-status",
                        json!({
                            "phase": "preparing",
                            "message": "Preparing model request..."
                        }),
                    );

                    // Check if we need to restart/reconnect
                    if self.port.is_none() || self.available_models.is_empty() {
                        println!("FoundryActor: No models found or port missing. Attempting to restart service...");
                        
                        // Transition to Reconnecting state to block prompts during reconnection
                        self.transition_to_reconnecting();

                        // First try to just reconnect (maybe service started in meantime)
                        if !self
                            .update_connection_info_with_retry(2, Duration::from_secs(1))
                            .await
                        {
                            // Still not working, restart the service
                            println!("FoundryActor: Quick reconnect failed, restarting service...");

                            // Restart service
                            if let Err(e) = self.restart_service().await {
                                println!("FoundryActor: ❌ Failed to restart service: {:?}", e);
                                println!("FoundryActor: Service restart error details - kind: {:?}", e.kind());
                                let _ = respond_to.send(format!("Error: Failed to restart local model service. Please ensure Foundry is installed: {}", e));
                                continue;
                            }

                            // Update info with longer retry
                            if !self
                                .update_connection_info_with_retry(5, Duration::from_secs(2))
                                .await
                            {
                                let _ = respond_to.send("Error: Could not connect to Foundry service after restart. Please check if Foundry is running.".to_string());
                                continue;
                            }
                        }
                    }

                    if let Some(port) = self.port {
                        // Use the model provided in the request (frontend is source of truth)
                        let model = requested_model.clone();
                        
                        // Check for desync between requested model and actor state
                        if let Some(ref actor_model) = self.model_id {
                            if actor_model != &model {
                                println!(
                                    "[FoundryActor] WARNING: Model desync detected! Request model='{}', Actor model='{}'. Using request model.",
                                    model, actor_model
                                );
                            }
                        } else {
                             println!(
                                "[FoundryActor] WARNING: Actor has no model_id set, using request model='{}'",
                                model
                            );
                        }

                        let desired_chat_format = chat_format_overrides
                            .get(&model)
                            .copied()
                            .unwrap_or(chat_format_default);
                        let supports_responses = Self::model_supports_responses(&model);
                        let effective_chat_format = if desired_chat_format == ChatFormatName::OpenaiResponses
                            && !supports_responses
                        {
                            println!(
                                "[FoundryActor] Model {} does not advertise Responses API; falling back to chat completions",
                                model
                            );
                            ChatFormatName::OpenaiCompletions
                        } else {
                            desired_chat_format
                        };
                        let use_responses_api =
                            matches!(effective_chat_format, ChatFormatName::OpenaiResponses);

                        let url = if use_responses_api {
                            format!("http://127.0.0.1:{}/v1/responses", port)
                        } else {
                            format!("http://127.0.0.1:{}/v1/chat/completions", port)
                        };
                        let verbose_logging = is_verbose_logging_enabled();

                        // Log incoming messages for debugging
                        println!(
                            "\n[FoundryActor] Received {} message(s){}",
                            chat_history_messages.len(),
                            if verbose_logging {
                                ":"
                            } else {
                                " (details suppressed; set LOG_VERBOSE=1 for previews)"
                            }
                        );
                        if verbose_logging {
                            for (i, msg) in chat_history_messages.iter().enumerate() {
                                let preview: String = msg.content.chars().take(100).collect();
                                println!(
                                    "  [{}] role={}, len={}, preview: {}...",
                                    i,
                                    msg.role,
                                    msg.content.len(),
                                    preview
                                );
                            }
                        } else {
                            let system_count = chat_history_messages
                                .iter()
                                .filter(|m| m.role == "system")
                                .count();
                            let user_count = chat_history_messages
                                .iter()
                                .filter(|m| m.role == "user")
                                .count();
                            let assistant_count = chat_history_messages
                                .iter()
                                .filter(|m| m.role == "assistant")
                                .count();
                            println!(
                                "[FoundryActor] Message roles | system={} | user={} | assistant={}",
                                system_count, user_count, assistant_count
                            );
                            if let Some(last_user) =
                                chat_history_messages.iter().rev().find(|m| m.role == "user")
                            {
                                let preview: String =
                                    last_user.content.chars().take(120).collect();
                                let truncated = last_user.content.len() > preview.len();
                                println!(
                                    "[FoundryActor] Latest user message (len={}): \"{}{}\"",
                                    last_user.content.len(),
                                    preview,
                                    if truncated { "..." } else { "" }
                                );
                            }
                        }

                        // For reasoning models, ensure we have a system message that instructs
                        // the model to provide a final answer after thinking
                        let mut messages = chat_history_messages.clone();
                        let has_system_msg = messages.iter().any(|m| m.role == "system");

                        println!("[FoundryActor] has_system_msg={}", has_system_msg);

                        if !has_system_msg {
                            // Prepend system message
                            println!(
                                "[FoundryActor] WARNING: No system message found, adding default!"
                            );
                            messages.insert(
                                0,
                                crate::protocol::ChatMessage {
                                    role: "system".to_string(),
                                    content: "You are a helpful AI assistant.".to_string(),
                                    system_prompt: None,
                                    tool_calls: None,
                                    tool_call_id: None,
                                },
                            );
                        } else {
                            // Log the actual system message being used
                            if let Some(sys_msg) = messages.iter().find(|m| m.role == "system") {
                                let content = sys_msg.content.clone();
                                self.log_with_diff(
                                    "FoundryActor:SystemPrompt",
                                    &content,
                                    &self.logging_persistence.last_logged_system_prompt,
                                )
                                .await;
                            }
                        }

                        // Get model info for this model
                        let model_info = self.model_info.iter().find(|m| m.id == model).cloned();

                        // Determine capabilities from model info or heuristics
                        let model_supports_reasoning = model_info
                            .as_ref()
                            .map(|m| m.reasoning)
                            .unwrap_or_else(|| model.to_lowercase().contains("reasoning"));

                        let model_supports_tools =
                            model_info.as_ref().map(|m| m.tool_calling).unwrap_or(false);

                        let supports_reasoning_effort = model_info
                            .as_ref()
                            .map(|m| m.supports_reasoning_effort)
                            .unwrap_or(false);

                        let model_family = model_info
                            .as_ref()
                            .map(|m| m.family)
                            .unwrap_or(ModelFamily::Generic);

                        // Only use native tools if model supports them, tools were provided, and native tool calling is enabled.
                        let use_native_tools = model_supports_tools
                            && native_tool_calling_enabled
                            && native_tool_specs
                                .as_ref()
                                .map(|t| !t.is_empty())
                                .unwrap_or(false);

                        println!("[FoundryActor] Model: {} | family: {:?} | reasoning: {} | tools: {} | reasoning_effort: {}",
                            model, model_family, model_supports_reasoning, use_native_tools, supports_reasoning_effort);

                        if use_native_tools {
                            let tool_names: Vec<&str> = native_tool_specs
                                .as_ref()
                                .map(|t| t.iter().map(|tool| tool.function.name.as_str()).collect())
                                .unwrap_or_default();
                            println!(
                                "[FoundryActor] Including {} native tools: {:?}",
                                tool_names.len(),
                                tool_names
                            );

                            // Log tool specs with diff logic
                            if let Some(specs) = &native_tool_specs {
                                if let Ok(specs_json) = serde_json::to_string_pretty(specs) {
                                    self.log_with_diff(
                                        "FoundryActor:ToolsJSON",
                                        &specs_json,
                                        &self.logging_persistence.last_logged_tools_json,
                                    )
                                    .await;
                                }
                            }
                        } else if native_tool_specs
                            .as_ref()
                            .map(|t| !t.is_empty())
                            .unwrap_or(false)
                        {
                            println!("[FoundryActor] Model does NOT support native tool calling, falling back to text-based tools");
                        }

                        // Log the start of completion with first 128 chars of the last user message
                        if let Some(last_user_msg) =
                            messages.iter().rev().find(|m| m.role == "user")
                        {
                            let preview: String = last_user_msg.content.chars().take(128).collect();
                            println!(
                                "[FoundryActor] 🚀 Starting completion: \"{}{}\"",
                                preview,
                                if last_user_msg.content.len() > 128 {
                                    "..."
                                } else {
                                    ""
                                }
                            );
                        }
                        use std::io::Write;
                        let request_start = std::time::Instant::now();
                        println!(
                            "[FoundryActor] Sending streaming request to Foundry at {}",
                            url
                        );
                        let _ = std::io::stdout().flush();

                        // Emit status: sending request to model
                        let _ = self.app_handle.emit(
                            "chat-stream-status",
                            json!({
                                "phase": "sending",
                                "message": "Sending request to model..."
                            }),
                        );

                        let client_clone = self.http_client.clone();
                        let respond_to_clone = respond_to.clone();

                        // Retry logic with exponential backoff for 4XX errors
                        const MAX_RETRIES: u32 = 3;
                        let mut retry_delay = Duration::from_secs(2);
                        let mut last_error: Option<String>;

                        for attempt in 1..=MAX_RETRIES {
                            // Rebuild URL in case port changed after restart
                            let current_url = if let Some(p) = self.port {
                                if use_responses_api {
                                    format!("http://127.0.0.1:{}/v1/responses", p)
                                } else {
                                    format!("http://127.0.0.1:{}/v1/chat/completions", p)
                                }
                            } else {
                                url.clone()
                            };

                            // Rebuild body in case anything changed after restart
                            let body_build_start = std::time::Instant::now();
                            let current_body = build_chat_request_body(
                                &model,
                                model_family,
                                &messages,
                                &native_tool_specs,
                                use_native_tools,
                                model_supports_reasoning,
                                supports_reasoning_effort,
                                &reasoning_effort,
                                use_responses_api,
                            );
                            let body_build_elapsed = body_build_start.elapsed();

                            // Note: Request body logging moved to log_with_diff for system prompt and tools JSON
                            // to keep logs clean. Enable RUST_LOG=debug for full request body if needed.

                            let send_start = std::time::Instant::now();
                            match client_clone
                                .post(&current_url)
                                .json(&current_body)
                                .send()
                                .await
                            {
                                Ok(mut resp) => {
                                    let send_elapsed = send_start.elapsed();
                                    let total_elapsed = request_start.elapsed();
                                    println!(
                                        "[FoundryActor] ⏱️  Request timing: body_build={:?}, http_send={:?}, total={:?}",
                                        body_build_elapsed, send_elapsed, total_elapsed
                                    );
                                    let _ = std::io::stdout().flush();

                                    // Emit status: model is responding
                                    let _ = self.app_handle.emit(
                                        "chat-stream-status",
                                        json!({
                                            "phase": "streaming",
                                            "message": "Generating response...",
                                            "time_to_first_response_ms": total_elapsed.as_millis() as u64
                                        }),
                                    );
                                    let status = resp.status();

                                    // Handle 4XX client errors with retry and service restart
                                    if status.is_client_error() {
                                        let text = resp.text().await.unwrap_or_default();
                                        println!(
                                            "FoundryActor: 4XX error ({}) on attempt {}/{}: {}",
                                            status, attempt, MAX_RETRIES, text
                                        );
                                        last_error = Some(format!("HTTP {}: {}", status, text));

                                        if attempt < MAX_RETRIES {
                                            // Restart service and re-detect port/EPs
                                            println!("FoundryActor: Restarting service due to 4XX error...");
                                            if let Err(e) = self.restart_service().await {
                                                println!(
                                                    "FoundryActor: Service restart failed: {}",
                                                    e
                                                );
                                            }

                                            // Re-detect port and EPs after restart
                                            let status = self.detect_port_and_eps().await;
                                            self.port = status.port;
                                            self.registered_eps = status.registered_eps;
                                            self.valid_eps = status.valid_eps;

                                            println!(
                                                "FoundryActor: Waiting {:?} before retry...",
                                                retry_delay
                                            );
                                            sleep(retry_delay).await;
                                            retry_delay = Duration::from_millis(
                                                (retry_delay.as_millis() as f64 * 1.5) as u64,
                                            )
                                            .min(Duration::from_secs(10));
                                            continue;
                                        } else {
                                            // Retries exhausted for 4XX error - suggest fallback
                                            let error_msg = format!("HTTP {} after {} retries: {}", status, MAX_RETRIES, text);
                                            if !model.to_lowercase().contains(DEFAULT_FALLBACK_MODEL) {
                                                self.emit_model_fallback_required(&model, &error_msg);
                                            }
                                        }
                                    } else if !status.is_success() {
                                        // Other non-success errors (5XX, etc.)
                                        let text = resp.text().await.unwrap_or_default();
                                        println!("Foundry error ({}): {}", status, text);
                                        // Emit fallback suggestion for 5XX errors
                                        let error_msg = format!("HTTP {}: {}", status, text);
                                        if !model.to_lowercase().contains(DEFAULT_FALLBACK_MODEL) {
                                            self.emit_model_fallback_required(&model, &error_msg);
                                        }
                                        let _ = respond_to_clone.send(format!("Error: {}", text));
                                        break;
                                    } else {
                                        // Success - stream the response
                                        let mut buffer = String::new();
                                        let mut streaming_tool_calls =
                                            StreamingToolCalls::default();
                                        let stream_start = std::time::Instant::now();
                                        let mut token_count: usize = 0;
                                        let mut last_token_time = stream_start;
                                        let mut last_progress_log = stream_start;
                                        println!(
                                            "[FoundryActor] 📡 Stream started (time_to_first_response={:?})",
                                            request_start.elapsed()
                                        );
                                        let _ = std::io::stdout().flush();

                                        // Note: Tool calls can arrive in two formats:
                                        // 1. Text-based: in content field as <tool_call>JSON</tool_call>
                                        // 2. Native OpenAI: in delta.tool_calls array (accumulated here)

                                        let mut stream_cancelled = false;
                                        'stream_loop: loop {
                                            tokio::select! {
                                                biased;

                                                // Check for cancellation (higher priority)
                                                _ = stream_cancel_rx.changed() => {
                                                    if *stream_cancel_rx.borrow() {
                                                        let elapsed = stream_start.elapsed();
                                                        println!("[FoundryActor] 🛑 Stream CANCELLED by user after {} tokens in {:.2}s",
                                                            token_count, elapsed.as_secs_f64());
                                                        let _ = std::io::stdout().flush();
                                                        stream_cancelled = true;
                                                        break 'stream_loop;
                                                    }
                                                }

                                                // Read next chunk from HTTP stream
                                                chunk_result = resp.chunk() => {
                                                    match chunk_result {
                                                        Ok(Some(chunk)) => {
                                                            if let Ok(s) = String::from_utf8(chunk.to_vec()) {
                                                                buffer.push_str(&s);

                                                                // Process lines
                                                                while let Some(idx) = buffer.find('\n') {
                                                                    let line = buffer[..idx].to_string();
                                                                    buffer = buffer[idx + 1..].to_string();

                                                                    let trimmed = line.trim();
                                                                    if trimmed.starts_with("data: ") {
                                                                        let data = &trimmed["data: ".len()..];
                                                                        if data == "[DONE]" {
                                                                            let elapsed = stream_start.elapsed();
                                                                            println!("[FoundryActor] ✅ Stream DONE. {} tokens in {:.2}s ({:.1} tok/s)",
                                                                                token_count,
                                                                                elapsed.as_secs_f64(),
                                                                                token_count as f64 / elapsed.as_secs_f64());
                                                                            let _ = std::io::stdout().flush();
                                                                            break 'stream_loop;
                                                                        }
                                                                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                                                        if let Some(content) =
                                                                            extract_stream_text(&json, use_responses_api)
                                                                        {
                                                                            if !content.is_empty() {
                                                                                token_count += 1;
                                                                                last_token_time = std::time::Instant::now();

                                                                                let _ = respond_to_clone.send(content);

                                                                                // Log progress every 5 seconds (verbose only)
                                                                                if verbose_logging
                                                                                    && last_progress_log.elapsed()
                                                                                        >= Duration::from_secs(5)
                                                                                {
                                                                                    let elapsed = stream_start.elapsed();
                                                                                    println!("[FoundryActor] 📊 Streaming: {} tokens so far ({:.2}s elapsed, {:.1} tok/s)",
                                                                                        token_count,
                                                                                        elapsed.as_secs_f64(),
                                                                                        token_count as f64 / elapsed.as_secs_f64());
                                                                                    let _ = std::io::stdout().flush();
                                                                                    last_progress_log = std::time::Instant::now();
                                                                                }
                                                                            }
                                                                        }

                                                                            // Accumulate native OpenAI tool calls (delta.tool_calls)
                                                                            if let Some(tool_calls) = json["choices"][0]["delta"]["tool_calls"].as_array() {
                                                                                streaming_tool_calls.process_delta(tool_calls);
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        Ok(None) => {
                                                            // Stream ended naturally (connection closed)
                                                            println!("[FoundryActor] Stream ended (connection closed)");
                                                            let _ = std::io::stdout().flush();
                                                            break 'stream_loop;
                                                        }
                                        Err(e) => {
                                            println!("[FoundryActor] ❌ Stream error: {:?}", e);
                                            println!(
                                                "[FoundryActor] Stream error details - is_timeout: {}, is_connect: {}, is_decode: {}",
                                                e.is_timeout(), e.is_connect(), e.is_decode()
                                            );
                                            let _ = std::io::stdout().flush();
                                            break 'stream_loop;
                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // If cancelled, skip the post-stream processing
                                        if stream_cancelled {
                                            break; // Exit retry loop
                                        }

                                        // Log if stream ended without DONE marker
                                        let total_elapsed = stream_start.elapsed();
                                        let since_last_token = last_token_time.elapsed();
                                        if since_last_token > Duration::from_secs(1)
                                            && token_count > 0
                                        {
                                            println!("[FoundryActor] ⚠️ Stream ended. Last token was {:.2}s ago. Total: {} tokens in {:.2}s", 
                                                since_last_token.as_secs_f64(),
                                                token_count,
                                                total_elapsed.as_secs_f64());
                                            let _ = std::io::stdout().flush();
                                        }

                                        // After stream ends, emit any accumulated native tool calls as text
                                        // so the existing agentic loop parser can detect them
                                        if !streaming_tool_calls.is_empty() {
                                            let native_calls =
                                                streaming_tool_calls.into_parsed_calls();
                                            println!("[FoundryActor] Emitting {} native tool calls as text", native_calls.len());
                                            for call in &native_calls {
                                                // Emit in <tool_call> format for parser compatibility
                                                let tool_call_text = format!(
                                                    "\n<tool_call>{{\"name\": \"{}\", \"arguments\": {}}}</tool_call>\n",
                                                    call.tool,
                                                    serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
                                                );
                                                let _ = respond_to_clone.send(tool_call_text);
                                            }
                                        }

                                        println!("Foundry stream loop finished.");
                                        break; // Success, exit retry loop
                                    }
                                }
                                Err(e) => {
                                    println!(
                                        "FoundryActor: ❌ Failed to call Foundry (attempt {}/{}): {:?}",
                                        attempt, MAX_RETRIES, e
                                    );
                                    println!(
                                        "FoundryActor: Request error details - url: {}, is_timeout: {}, is_connect: {}, is_request: {}, is_body: {}",
                                        current_url, e.is_timeout(), e.is_connect(), e.is_request(), e.is_body()
                                    );
                                    if let Some(status) = e.status() {
                                        println!("FoundryActor: Error had HTTP status: {}", status);
                                    }
                                    last_error = Some(format!("Connection error: {:?}", e));

                                    if attempt < MAX_RETRIES {
                                        // Restart service on connection errors too
                                        println!("FoundryActor: Restarting service due to connection error...");
                                        if let Err(restart_err) = self.restart_service().await {
                                            println!(
                                                "FoundryActor: Service restart failed: {}",
                                                restart_err
                                            );
                                        }

                                        // Re-detect port and EPs
                                        let status = self.detect_port_and_eps().await;
                                        self.port = status.port;
                                        self.registered_eps = status.registered_eps;
                                        self.valid_eps = status.valid_eps;

                                        sleep(retry_delay).await;
                                        retry_delay = Duration::from_millis(
                                            (retry_delay.as_millis() as f64 * 1.5) as u64,
                                        )
                                        .min(Duration::from_secs(10));
                                        continue;
                                    }
                                }
                            }

                            // If we get here with an error after max retries, report it
                            if let Some(err) = &last_error {
                                if attempt == MAX_RETRIES {
                                    // Emit fallback suggestion for connection errors after retries exhausted
                                    if !model.to_lowercase().contains(DEFAULT_FALLBACK_MODEL) {
                                        self.emit_model_fallback_required(&model, err);
                                    }
                                    let _ = respond_to_clone.send(format!(
                                        "Error after {} retries: {}",
                                        MAX_RETRIES, err
                                    ));
                                }
                            }
                            break;
                        }
                    } else {
                        println!("Foundry endpoint not available (port not found).");
                        // Emit fallback when Foundry endpoint not available
                        if !requested_model.to_lowercase().contains(DEFAULT_FALLBACK_MODEL) {
                            self.emit_model_fallback_required(&requested_model, "Foundry endpoint not available");
                        }
                        let _ = respond_to.send("The local model service is not available. Please check if Foundry is installed and running.".to_string());
                    }
                    
                    // Clear GPU operation status (use cloned guard)
                    *gpu_guard.current_operation.write().await = None;
                    let _ = self.app_handle.emit("gpu-status", json!({
                        "operation": "chat",
                        "status": "completed"
                    }));
                }
                FoundryMsg::DownloadModel {
                    model_name,
                    respond_to,
                } => {
                    println!("FoundryActor: Downloading model: {}", model_name);
                    if let Some(port) = self.port {
                        let result = self.download_model_impl(&self.http_client, port, &model_name).await;
                        // Refresh the models list after successful download
                        if result.is_ok() {
                            let models = self.get_models_via_rest(port).await;
                            self.available_models = models.iter().map(|m| m.id.clone()).collect();
                            println!(
                                "FoundryActor: Refreshed models list after download, {} models available",
                                self.available_models.len()
                            );
                            // Notify frontend that available models have changed
                            self.emit_available_models_changed(&self.available_models);
                        }
                        let _ = respond_to.send(result);
                    } else {
                        let _ = respond_to.send(Err("Foundry service not available".to_string()));
                    }
                }
                FoundryMsg::LoadModel {
                    model_name,
                    respond_to,
                } => {
                    // Transition to LoadingModel state BEFORE acquiring lock
                    self.transition_model_state(ModelState::LoadingModel {
                        model_id: model_name.clone(),
                    });
                    
                    // Perform the actual model load with GPU lock
                    let load_result = {
                        // Acquire GPU mutex for model loading
                        let _gpu_lock = self.gpu_guard.mutex.lock().await;
                        *self.gpu_guard.current_operation.write().await = Some(format!("Loading model: {}", model_name));
                        
                        let _ = self.app_handle.emit("gpu-status", json!({
                            "operation": "load_model",
                            "model": &model_name,
                            "status": "started"
                        }));
                        
                        println!("FoundryActor: Loading model into VRAM: {}", model_name);
                        let result = if let Some(port) = self.port {
                            self.load_model_impl(&self.http_client, port, &model_name).await
                        } else {
                            Err("Foundry service not available".to_string())
                        };
                        
                        *self.gpu_guard.current_operation.write().await = None;
                        let _ = self.app_handle.emit("gpu-status", json!({
                            "operation": "load_model",
                            "model": &model_name,
                            "status": "completed"
                        }));
                        
                        result
                        // _gpu_lock released here
                    };
                    
                    // Now update state after lock is released
                    match &load_result {
                        Ok(()) => {
                            self.model_id = Some(model_name.clone());
                            self.transition_to_ready(model_name.clone());
                            self.emit_model_selected(&model_name);
                            println!("FoundryActor: Updated selected model to: {}", model_name);
                        }
                        Err(e) => {
                            if e.contains("service not available") {
                                self.transition_to_service_unavailable(e.clone());
                            } else {
                                self.transition_to_error(
                                    e.clone(),
                                    self.model_id.clone(),
                                );
                            }
                        }
                    }
                    let _ = respond_to.send(load_result);
                }
                FoundryMsg::GetLoadedModels { respond_to } => {
                    println!("FoundryActor: Getting loaded models");
                    if let Some(port) = self.port {
                        let models = self.get_loaded_models_impl(&self.http_client, port).await;
                        let _ = respond_to.send(models);
                    } else {
                        let _ = respond_to.send(Vec::new());
                    }
                }
                FoundryMsg::GetCurrentModel { respond_to } => {
                    // Return the ModelInfo for the currently selected model
                    let current = self
                        .model_id
                        .as_ref()
                        .and_then(|id| self.model_info.iter().find(|m| &m.id == id).cloned());
                    println!(
                        "FoundryActor: GetCurrentModel returning: {:?}",
                        current.as_ref().map(|m| &m.id)
                    );
                    let _ = respond_to.send(current);
                }
                FoundryMsg::GetModelState { respond_to } => {
                    // Return the current model state machine state
                    let _ = respond_to.send(self.model_state.clone());
                }
                FoundryMsg::Reload { respond_to } => {
                    println!("FoundryActor: Reloading foundry service...");
                    
                    // Transition to ServiceRestarting state
                    self.transition_to_service_restarting();

                    // Restart the service
                    let result = match self.restart_service().await {
                        Ok(()) => {
                            // Re-detect port, endpoints, and available models after restart
                            if self.update_connection_info().await {
                                println!("FoundryActor: Service reloaded successfully. Port: {:?}, Models: {}", 
                                    self.port, self.available_models.len());
                                
                                // Transition to Ready with current model if we have one
                                if let Some(ref model_id) = self.model_id {
                                    self.transition_to_ready(model_id.clone());
                                } else if !self.available_models.is_empty() {
                                    // Select first available model if none selected
                                    let first_model = self.available_models[0].clone();
                                    self.model_id = Some(first_model.clone());
                                    self.transition_to_ready(first_model);
                                } else {
                                    // Service is up but no models available
                                    self.transition_to_error(
                                        "No models available after restart".to_string(),
                                        None,
                                    );
                                }
                                Ok(())
                            } else {
                                self.transition_to_service_unavailable(
                                    "Could not reconnect after restart".to_string()
                                );
                                Err("Failed to reconnect after restart".to_string())
                            }
                        }
                        Err(e) => {
                            println!("FoundryActor: ❌ Failed to reload service: {:?}", e);
                            println!("FoundryActor: Reload service error details - kind: {:?}", e.kind());
                            self.transition_to_service_unavailable(format!("Restart failed: {}", e));
                            Err(format!("Failed to reload service: {}", e))
                        }
                    };

                    let _ = respond_to.send(result);
                }
                FoundryMsg::GetCatalogModels { respond_to } => {
                    println!("FoundryActor: Getting catalog models");
                    if let Some(port) = self.port {
                        let catalog = self.get_catalog_models_impl(&self.http_client, port).await;
                        let _ = respond_to.send(catalog);
                    } else {
                        let _ = respond_to.send(Vec::new());
                    }
                }
                FoundryMsg::UnloadModel {
                    model_name,
                    respond_to,
                } => {
                    // Acquire GPU mutex for model unloading
                    let _gpu_lock = self.gpu_guard.mutex.lock().await;
                    *self.gpu_guard.current_operation.write().await = Some(format!("Unloading model: {}", model_name));
                    
                    println!("FoundryActor: Unloading model: {}", model_name);
                    if let Some(port) = self.port {
                        let result = self.unload_model_impl(&self.http_client, port, &model_name).await;
                        let _ = respond_to.send(result);
                    } else {
                        let _ = respond_to.send(Err("Foundry service not available".to_string()));
                    }
                    
                    *self.gpu_guard.current_operation.write().await = None;
                }
                FoundryMsg::UnloadCurrentLlm { respond_to } => {
                    // Unload the currently selected LLM to free GPU memory for embedding operations
                    // This prevents Metal context contention between Foundry Local's LLM and
                    // fastembed's ONNX Runtime + CoreML for GPU embeddings.
                    if let Some(ref model_name) = self.model_id {
                        println!("╔══════════════════════════════════════════════════════════════╗");
                        println!("║  UNLOADING LLM FOR GPU EMBEDDING: {}  ", model_name);
                        println!("╚══════════════════════════════════════════════════════════════╝");
                        
                        *self.gpu_guard.current_operation.write().await = Some(format!("Unloading LLM for embedding: {}", model_name));
                        
                        let model_name_clone = model_name.clone();
                        if let Some(port) = self.port {
                            match self.unload_model_impl(&self.http_client, port, &model_name_clone).await {
                                Ok(()) => {
                                    println!("FoundryActor: LLM unloaded successfully, GPU memory freed for embedding");
                                    let _ = respond_to.send(Ok(Some(model_name_clone)));
                                }
                                Err(e) => {
                                    println!("FoundryActor: ⚠️ WARNING - Failed to unload LLM: {}", e);
                                    println!("FoundryActor: LLM unload failed, but continuing - embedding may still work, just slower");
                                    // Return Ok anyway - the embedding may still work, just slower
                                    let _ = respond_to.send(Ok(Some(model_name_clone)));
                                }
                            }
                        } else {
                            // No port means service isn't running, so nothing to unload
                            let _ = respond_to.send(Ok(Some(model_name_clone)));
                        }
                        
                        *self.gpu_guard.current_operation.write().await = None;
                    } else {
                        println!("FoundryActor: No LLM currently loaded, nothing to unload");
                        let _ = respond_to.send(Ok(None));
                    }
                }
                FoundryMsg::GetServiceStatus { respond_to } => {
                    println!("FoundryActor: Getting service status");
                    if let Some(port) = self.port {
                        let result = self.get_service_status_impl(&self.http_client, port).await;
                        let _ = respond_to.send(result);
                    } else {
                        let _ = respond_to.send(Err("Foundry service not available".to_string()));
                    }
                }
                FoundryMsg::RemoveCachedModel {
                    model_name,
                    respond_to,
                } => {
                    println!("FoundryActor: Removing cached model: {}", model_name);
                    let result = self.remove_cached_model_impl(&model_name).await;
                    // Refresh the models list after successful removal
                    if result.is_ok() {
                        if let Some(port) = self.port {
                            let models = self.get_models_via_rest(port).await;
                            self.available_models = models.iter().map(|m| m.id.clone()).collect();
                            println!(
                                "FoundryActor: Refreshed models list after removal, {} models available",
                                self.available_models.len()
                            );
                            // Notify frontend that available models have changed
                            self.emit_available_models_changed(&self.available_models);
                        }
                    }
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    async fn update_connection_info(&mut self) -> bool {
        // Get port and EPs from foundry service status
        let status = self.detect_port_and_eps().await;
        self.port = status.port;
        self.registered_eps = status.registered_eps;
        self.valid_eps = status.valid_eps;

        if let Some(p) = self.port {
            println!("Foundry service detected on port {}", p);
            println!("FoundryActor: Valid EPs: {:?}", self.valid_eps);

            if self.valid_eps.is_empty() {
                println!(
                    "FoundryActor: Warning - no valid EPs available, models may not be loadable"
                );
            }

            // Always check REST API for downloaded models (they can be listed even without EPs)
            let models = self.get_models_via_rest(p).await;

            if models.is_empty() {
                println!("FoundryActor: No models found via REST API");
                // Return true to indicate service is running but with no models
                // The frontend will handle showing help to the user
                self.available_models = Vec::new();
                self.model_info = Vec::new();
                return true; // Service is reachable, just no models cached
            }

            // Build available_models and model_info from REST API response
            self.available_models = models.iter().map(|m| m.id.clone()).collect();

            // Build model info with inferred capabilities from model names
            self.model_info = models.iter()
                .map(|model_obj| {
                    let id = model_obj.id.clone();
                    let id_lower = id.to_lowercase();

                    // Detect model family from ID
                    let family = ModelFamily::from_model_id(&id);

                    // Infer tool calling support from model name
                    // Most modern instruction-tuned models support tool calling
                    // Either natively or via text-based format (Hermes-style <tool_call>)
                    let mut tool_calling = id_lower.contains("instruct")
                        || id_lower.contains("coder")
                        || id_lower.contains("qwen")
                        || id_lower.contains("phi")
                        || id_lower.contains("granite")
                        || id_lower.contains("llama")
                        || id_lower.contains("mistral")
                        || id_lower.contains("gemma")
                        || id_lower.contains("chat");

                    // Check for supportsToolCalling tag from Foundry
                    let supports_tool_calling = model_obj.tags.iter().any(|t| t == "supportsToolCalling");
                    if supports_tool_calling {
                        println!("FoundryActor: Model {} explicitly supports tool calling via tag", id);
                        tool_calling = true;
                    }

                    // Determine tool format based on model family and capabilities
                    // Note: This should match the format expected by model_profiles.rs
                    let tool_format = if !tool_calling {
                        ToolFormat::TextBased
                    } else {
                        match family {
                            // Qwen, Mistral, LLaMA use Hermes-style <tool_call> format
                            ModelFamily::GptOss => ToolFormat::Hermes,
                            ModelFamily::Gemma => ToolFormat::Gemini,
                            ModelFamily::Phi => ToolFormat::Hermes,
                            ModelFamily::Granite => ToolFormat::Granite,
                            ModelFamily::Generic => ToolFormat::Hermes,
                        }
                    };

                    // Infer reasoning support from model name
                    let reasoning = id_lower.contains("reasoning");

                    // Determine reasoning format based on model family
                    let reasoning_format = if !reasoning {
                        ReasoningFormat::None
                    } else {
                        match family {
                            ModelFamily::GptOss => ReasoningFormat::ChannelBased,
                            ModelFamily::Phi => ReasoningFormat::ThinkTags,
                            ModelFamily::Granite => ReasoningFormat::ThinkingTags,
                            _ => ReasoningFormat::None,
                        }
                    };

                    // Use conservative defaults for token limits (actual values would come from API)
                    let max_input_tokens = 4096;
                    let max_output_tokens = 4096;

                    // Parameter support flags based on model family
                    let supports_temperature = true;
                    let supports_top_p = true;
                    let supports_reasoning_effort = reasoning && matches!(family, ModelFamily::Phi);

                    // Vision support inferred from model name
                    let vision = id_lower.contains("vision");

                    println!(
                        "  Model: {} | family: {:?} | toolCalling: {} ({:?}) | vision: {} | reasoning: {} ({:?})",
                        id, family, tool_calling, tool_format, vision, reasoning, reasoning_format
                    );

                    ModelInfo {
                        id,
                        family,
                        tool_calling,
                        tool_format,
                        vision,
                        reasoning,
                        reasoning_format,
                        max_input_tokens,
                        max_output_tokens,
                        supports_tool_calling,
                        supports_temperature,
                        supports_top_p,
                        supports_reasoning_effort,
                    }
                })
                .collect();

            // Select model at startup - SIMPLE LOGIC:
            // 1. Read selected_model from settings
            // 2. If not set or not available, use phi-4-mini-instruct
            // NO "first model" fallback - that causes problems!
            if self.model_id.is_none() {
                use std::io::Write;
                
                println!("\n[FoundryActor] ========== MODEL SELECTION AT STARTUP ==========");
                println!("[FoundryActor] Available models: {:?}", self.available_models);
                let _ = std::io::stdout().flush();
                
                // Step 1: Read persisted model from settings
                let persisted_model: Option<String> = if let Some(settings_state) = self.app_handle.try_state::<SettingsState>() {
                    let guard = futures::executor::block_on(settings_state.settings.read());
                    let persisted = guard.selected_model.clone();
                    println!("[FoundryActor] Settings selected_model: {:?}", persisted);
                    let _ = std::io::stdout().flush();
                    persisted
                } else {
                    println!("[FoundryActor] ❌ Could not access SettingsState!");
                    let _ = std::io::stdout().flush();
                    None
                };
                
                // Step 2: Check if persisted model exists in available models
                let persisted_available = persisted_model.as_ref().and_then(|pm| {
                    self.available_models.iter().find(|m| *m == pm).cloned()
                });
                
                // Step 3: Find phi-4-mini-instruct as fallback
                let fallback_model = self.available_models.iter().find(|m| {
                    m.to_lowercase().contains(DEFAULT_FALLBACK_MODEL)
                }).cloned();
                
                println!("[FoundryActor] Persisted model available: {:?}", persisted_available);
                println!("[FoundryActor] Fallback model (phi-4-mini): {:?}", fallback_model);
                let _ = std::io::stdout().flush();
                
                // Step 4: Choose model - settings OR phi-4-mini-instruct ONLY
                let selected = if let Some(model) = persisted_available {
                    println!("[FoundryActor] ✅ SELECTED: {} (from settings)", model);
                    let _ = std::io::stdout().flush();
                    Some(model)
                } else if let Some(model) = fallback_model {
                    println!("[FoundryActor] ✅ SELECTED: {} (fallback - settings model not available)", model);
                    let _ = std::io::stdout().flush();
                    Some(model)
                } else {
                    // No phi-4-mini available - this is a problem, but don't pick random model
                    println!("[FoundryActor] ❌ Neither settings model nor phi-4-mini-instruct available!");
                    println!("[FoundryActor] Available models: {:?}", self.available_models);
                    let _ = std::io::stdout().flush();
                    None
                };
                
                println!("[FoundryActor] =================================================\n");
                let _ = std::io::stdout().flush();
                
                if let Some(model) = selected {
                    self.model_id = Some(model.clone());
                    self.transition_to_ready(model.clone());
                    self.emit_model_selected(&model);
                } else {
                    // No model available - transition to error state
                    self.transition_to_error(
                        "No compatible model available. Please download phi-4-mini-instruct.".to_string(),
                        None,
                    );
                    return true; // Service is up, but no model selected
                }
            }

            println!(
                "FoundryActor: Found {} models via REST API",
                self.available_models.len()
            );
            return true;
        } else {
            println!("Warning: Could not detect Foundry service port.");
        }
        false
    }

    /// Get models via REST API: GET /openai/models
    /// Returns a list of model info objects
    async fn get_models_via_rest(&self, port: u16) -> Vec<FoundryModel> {
        let url = format!("http://127.0.0.1:{}/openai/models", port);
        println!("FoundryActor: Fetching models via REST API: {}", url);

        match self.http_client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let body_val: Value = match resp.json().await {
                        Ok(v) => v,
                        Err(e) => {
                            println!("FoundryActor: ❌ Failed to parse models response JSON: {:?}", e);
                            println!("FoundryActor: Parse error - is_decode: {}, is_body: {}", e.is_decode(), e.is_body());
                            return Vec::new();
                        }
                    };

                    // Try parsing as array of strings first (legacy format)
                    if let Some(arr) = body_val.as_array() {
                        if arr.get(0).and_then(|v| v.as_str()).is_some() {
                            return arr.iter()
                                .filter_map(|v| v.as_str())
                                .map(|id| FoundryModel { id: id.to_string(), tags: Vec::new() })
                                .collect();
                        }
                        
                        // Try parsing as array of objects (foundry specific)
                        return arr.iter()
                            .filter_map(|v| serde_json::from_value::<FoundryModel>(v.clone()).ok())
                            .collect();
                    }

                    // Try parsing as OpenAI-compatible list: { "data": [...] }
                    if let Ok(resp_obj) = serde_json::from_value::<FoundryModelsResponse>(body_val) {
                        return resp_obj.data;
                    }

                    println!("FoundryActor: Unexpected models response format");
                    Vec::new()
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    println!("FoundryActor: ❌ REST API error fetching models - HTTP {}: {}", status, body);
                    println!("FoundryActor: Request URL was: {}", url);
                    Vec::new()
                }
            }
            Err(e) => {
                println!("FoundryActor: ❌ Failed to call REST API for models: {:?}", e);
                println!(
                    "FoundryActor: Request error details - url: {}, is_timeout: {}, is_connect: {}",
                    url, e.is_timeout(), e.is_connect()
                );
                Vec::new()
            }
        }
    }

    /// Try to update connection info with exponential backoff
    /// Returns true if successfully connected and found models
    async fn update_connection_info_with_retry(
        &mut self,
        max_retries: u32,
        initial_delay: Duration,
    ) -> bool {
        let mut delay = initial_delay;

        for attempt in 1..=max_retries {
            println!(
                "FoundryActor: Connection attempt {}/{}",
                attempt, max_retries
            );

            if self.update_connection_info().await {
                println!(
                    "FoundryActor: Successfully connected to Foundry on attempt {}",
                    attempt
                );
                return true;
            }

            if attempt < max_retries {
                println!(
                    "FoundryActor: Attempt {} failed, retrying in {:?}...",
                    attempt, delay
                );
                sleep(delay).await;
                delay = Duration::from_millis((delay.as_millis() as f64 * 1.5) as u64)
                    .min(Duration::from_secs(10));
            }
        }

        println!(
            "FoundryActor: Failed to connect after {} attempts",
            max_retries
        );
        false
    }

    fn emit_model_selected(&self, model: &str) {
        let _ = self.app_handle.emit("model-selected", model.to_string());
    }

    /// Emit an event to request fallback to the default model due to errors.
    /// Also DIRECTLY updates and saves settings to ensure the fallback persists across restarts.
    fn emit_model_fallback_required(&self, current_model: &str, error: &str) {
        use std::io::Write;
        
        println!(
            "\n[FoundryActor] ⚠️  MODEL FALLBACK TRIGGERED ⚠️"
        );
        println!(
            "[FoundryActor] Current model: {}", current_model
        );
        println!(
            "[FoundryActor] Fallback model: {}", DEFAULT_FALLBACK_MODEL
        );
        println!(
            "[FoundryActor] Error: {}", error
        );
        let _ = std::io::stdout().flush();
        
        // CRITICAL: Directly update settings to prevent restart loop
        // Don't rely on frontend event handler - do it here in the backend
        let app_handle = self.app_handle.clone();
        let fallback_model = DEFAULT_FALLBACK_MODEL.to_string();
        let current = current_model.to_string();
        
        // Spawn a task to update settings (we can't block here in the actor)
        tokio::spawn(async move {
            println!("[FoundryActor] Attempting to persist fallback model to settings...");
            let _ = std::io::stdout().flush();
            
            // Access SettingsState through the app handle
            if let Some(settings_state) = app_handle.try_state::<SettingsState>() {
                let mut guard = settings_state.settings.write().await;
                let old_model = guard.selected_model.clone();
                guard.selected_model = Some(fallback_model.clone());
                
                println!(
                    "[FoundryActor] 📝 Updating settings: selected_model '{}' -> '{}'",
                    old_model.as_deref().unwrap_or("<none>"),
                    fallback_model
                );
                let _ = std::io::stdout().flush();
                
                // Save immediately
                match settings::save_settings(&guard).await {
                    Ok(()) => {
                        println!(
                            "[FoundryActor] ✅ SUCCESS: Fallback model '{}' persisted to settings file!",
                            fallback_model
                        );
                        println!(
                            "[FoundryActor] On next restart, app will use '{}' instead of '{}'",
                            fallback_model, current
                        );
                        let _ = std::io::stdout().flush();
                    }
                    Err(e) => {
                        println!(
                            "[FoundryActor] ❌ FAILED to save settings: {}",
                            e
                        );
                        let _ = std::io::stdout().flush();
                    }
                }
            } else {
                println!(
                    "[FoundryActor] ❌ Could not access SettingsState - fallback not persisted!"
                );
                let _ = std::io::stdout().flush();
            }
        });
        
        // Also emit event for frontend (backup mechanism)
        let _ = self.app_handle.emit(
            "model-fallback-required",
            json!({
                "current_model": current_model,
                "fallback_model": DEFAULT_FALLBACK_MODEL,
                "error": error
            }),
        );
    }

    /// Emit an event when the available models list changes (after download, removal, or reload).
    /// The frontend listens to update the model dropdown in real-time.
    fn emit_available_models_changed(&self, models: &[String]) {
        println!(
            "[FoundryActor] Emitting available-models-changed: {} models",
            models.len()
        );
        let _ = self.app_handle.emit("available-models-changed", models.to_vec());
    }

    /// Logs the content with a diff if it has changed since the last log.
    /// If it's the first log, it logs the full content.
    async fn log_with_diff(
        &self,
        label: &str,
        current_content: &str,
        storage: &Arc<RwLock<Option<String>>>,
    ) {
        let mut last_content_guard = storage.write().await;
        
        match last_content_guard.as_ref() {
            None => {
                // First time logging this content in this execution
                println!(
                    "[{}] --- FIRST LOG BEGIN ---\n{}\n[{}] --- FIRST LOG END ---",
                    label, current_content, label
                );
                *last_content_guard = Some(current_content.to_string());
            }
            Some(last_content) if last_content != current_content => {
                // Content changed, log the diff
                println!("[{}] Content changed! Printing line-based diff:", label);
                
                let diff = TextDiff::from_lines(last_content.as_str(), current_content);
                for change in diff.iter_all_changes() {
                    let sign = match change.tag() {
                        ChangeTag::Delete => "-",
                        ChangeTag::Insert => "+",
                        ChangeTag::Equal => " ",
                    };
                    print!("{}{}", sign, change);
                }
                println!("[{}] --- DIFF END ---", label);
                
                *last_content_guard = Some(current_content.to_string());
            }
            _ => {
                // Content unchanged, skip logging unless verbose
                if crate::is_verbose_logging_enabled() {
                    println!("[{}] Content unchanged ({} chars)", label, current_content.len());
                }
            }
        }
    }

    async fn ensure_service_running(&self) -> std::io::Result<()> {
        println!("FoundryActor: Checking/Starting Foundry service...");
        // Try to start service via CLI: `foundry service start`
        // We use a timeout to prevent hanging indefinitely
        let foundry_bin = find_foundry_binary();
        let child = Command::new(&foundry_bin).args(&["service", "start"]).output();

        let output = match timeout(Duration::from_secs(10), child).await {
            Ok(res) => res?,
            Err(_) => {
                println!("FoundryActor: 'foundry service start' timed out.");
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "foundry service start timed out",
                ));
            }
        };

        if output.status.success() {
            println!("Foundry service start command issued successfully.");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("Foundry service start command failed: {}", stderr);
        }
        Ok(())
    }

    async fn run_foundry_command_with_timeout(
        &self,
        args: &[&str],
        timeout_secs: u64,
        op_desc: &str,
    ) -> std::io::Result<std::process::Output> {
        let foundry_bin = find_foundry_binary();
        println!("FoundryActor: Running `{} {}` ...", foundry_bin, args.join(" "));
        let child = Command::new(&foundry_bin).args(args).output();
        match timeout(Duration::from_secs(timeout_secs), child).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => {
                println!(
                    "FoundryActor: Failed to run `foundry {}` during {}: {}",
                    args.join(" "),
                    op_desc,
                    e
                );
                Err(e)
            }
            Err(_) => {
                println!(
                    "FoundryActor: `foundry {}` timed out after {}s during {}",
                    args.join(" "),
                    timeout_secs,
                    op_desc
                );
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "foundry {} timed out after {} seconds",
                        args.join(" "),
                        timeout_secs
                    ),
                ))
            }
        }
    }

    async fn stop_service_immediately(&self, timeout_secs: u64) -> std::io::Result<()> {
        // Single stop command (CLI does not accept --no-wait/--force)
        let output = self
            .run_foundry_command_with_timeout(&["service", "stop"], timeout_secs, "service stop")
            .await?;

        if output.status.success() {
            println!("Foundry service stop command succeeded.");
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("Foundry service stop command failed: {}", stderr);
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("service stop failed: {}", stderr),
        ))
    }

    async fn start_service_after_stop(&self, timeout_secs: u64) -> std::io::Result<()> {
        match self
            .run_foundry_command_with_timeout(&["service", "start"], timeout_secs, "service start")
            .await
        {
            Ok(output) => {
                if output.status.success() {
                    println!("Foundry service start command issued successfully.");
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("Foundry service start command failed: {}", stderr);
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("service start failed: {}", stderr),
                    ))
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn restart_service(&mut self) -> std::io::Result<()> {
        println!("Restarting Foundry service (stop then start)...");

        let stop_timeout_secs: u64 = 20;
        let start_timeout_secs: u64 = 75;

        // Emit event: stop started
        let _ = self.app_handle.emit(
            "service-restart-started",
            json!({
                "message": "Stopping Foundry service..."
            }),
        );
        let _ = self.app_handle.emit(
            "service-stop-started",
            json!({
                "message": "Stopping Foundry service..."
            }),
        );

        if let Err(e) = self.stop_service_immediately(stop_timeout_secs).await {
            let _ = self.app_handle.emit(
                "service-stop-complete",
                json!({
                    "success": false,
                    "error": format!("Failed to stop service: {}", e)
                }),
            );
            let _ = self.app_handle.emit(
                "service-restart-complete",
                json!({
                    "success": false,
                    "error": format!("Failed to stop service: {}", e)
                }),
            );
            return Err(e);
        }

        let _ = self.app_handle.emit(
            "service-stop-complete",
            json!({
                "success": true,
                "message": "Service stopped"
            }),
        );

        // Emit event: start started
        let _ = self.app_handle.emit(
            "service-start-started",
            json!({
                "message": "Starting Foundry service..."
            }),
        );

        if let Err(e) = self.start_service_after_stop(start_timeout_secs).await {
            let _ = self.app_handle.emit(
                "service-start-complete",
                json!({
                    "success": false,
                    "error": format!("Failed to start service: {}", e)
                }),
            );
            let _ = self.app_handle.emit(
                "service-restart-complete",
                json!({
                    "success": false,
                    "error": format!("Failed to start service: {}", e)
                }),
            );
            return Err(e);
        }

        // Wait for the service to come up and verify connectivity
        println!("Waiting for service to be ready...");
        if !self
            .update_connection_info_with_retry(5, Duration::from_secs(2))
            .await
        {
            let err = "Service start reported success, but endpoint not reachable";
            println!("FoundryActor: {}", err);
            let _ = self.app_handle.emit(
                "service-start-complete",
                json!({
                    "success": false,
                    "error": err
                }),
            );
            let _ = self.app_handle.emit(
                "service-restart-complete",
                json!({
                    "success": false,
                    "error": err
                }),
            );
            return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
        }

        let _ = self.app_handle.emit(
            "service-start-complete",
            json!({
                "success": true,
                "message": "Service started"
            }),
        );

        // Emit overall restart complete
        let _ = self.app_handle.emit(
            "service-restart-complete",
            json!({
                "success": true,
                "message": "Service restarted successfully"
            }),
        );

        Ok(())
    }

    /// Detect port and Execution Providers via `foundry service status`
    ///
    /// Parses output like:
    /// ```text
    /// 🟢 Model management service is running on http://127.0.0.1:54657/openai/status
    /// EP autoregistration status: Successfully downloaded and registered the following EPs: NvTensorRTRTXExecutionProvider, OpenVINOExecutionProvider.
    /// Valid EPs: CPUExecutionProvider, WebGpuExecutionProvider, NvTensorRTRTXExecutionProvider, OpenVINOExecutionProvider, CUDAExecutionProvider
    /// ```
    async fn detect_port_and_eps(&self) -> ServiceStatus {
        let foundry_bin = find_foundry_binary();
        println!("FoundryActor: Detecting port and EPs via '{} service status'...", foundry_bin);

        let child = Command::new(&foundry_bin)
            .args(&["service", "status"])
            .output();

        match timeout(Duration::from_secs(5), child).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let status = self.parse_service_status(&stdout);
                status
            }
            Ok(Err(e)) => {
                println!("Failed to run foundry status: {}", e);
                ServiceStatus {
                    port: None,
                    registered_eps: Vec::new(),
                    valid_eps: Vec::new(),
                }
            }
            Err(_) => {
                println!("FoundryActor: 'foundry service status' timed out.");
                ServiceStatus {
                    port: None,
                    registered_eps: Vec::new(),
                    valid_eps: Vec::new(),
                }
            }
        }
    }

    /// Parse the output of `foundry service status`
    fn parse_service_status(&self, output: &str) -> ServiceStatus {
        let mut port = None;
        let mut registered_eps = Vec::new();
        let mut valid_eps = Vec::new();

        for line in output.lines() {
            // Parse port from URL: "http://127.0.0.1:54657" or "https://127.0.0.1:54657"
            if let Some(start_idx) = line.find("http://127.0.0.1:") {
                let rest = &line[start_idx + "http://127.0.0.1:".len()..];
                let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = port_str.parse::<u16>() {
                    port = Some(p);
                    println!("FoundryActor: Detected port {}", p);
                }
            } else if let Some(start_idx) = line.find("https://127.0.0.1:") {
                let rest = &line[start_idx + "https://127.0.0.1:".len()..];
                let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = port_str.parse::<u16>() {
                    port = Some(p);
                    println!("FoundryActor: Detected port {} (https)", p);
                }
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

    /// Download a model from the Foundry catalog
    /// POST /openai/download with streaming progress
    async fn download_model_impl(
        &self,
        client: &reqwest::Client,
        port: u16,
        model_name: &str,
    ) -> Result<(), String> {
        let url = format!("http://127.0.0.1:{}/openai/download", port);
        println!(
            "FoundryActor: Downloading model {} from {}",
            model_name, url
        );

        // Get model info from the catalog first to build the proper download request
        let catalog_url = format!("http://127.0.0.1:{}/foundry/list", port);
        let catalog_response = client
            .get(&catalog_url)
            .send()
            .await
            .map_err(|e| {
                println!(
                    "FoundryActor: ❌ Failed to fetch catalog for download: {:?}",
                    e
                );
                println!(
                    "FoundryActor: Catalog fetch error details - url: {}, is_timeout: {}, is_connect: {}",
                    catalog_url, e.is_timeout(), e.is_connect()
                );
                format!("Failed to fetch catalog: {:?}", e)
            })?;

        if !catalog_response.status().is_success() {
            let status = catalog_response.status();
            let body = catalog_response.text().await.unwrap_or_default();
            println!(
                "FoundryActor: ❌ Catalog fetch failed - HTTP {}: {}",
                status, body
            );
            println!("FoundryActor: Catalog URL was: {}", catalog_url);
            return Err(format!(
                "Failed to fetch catalog: HTTP {} - {}",
                status, body
            ));
        }

        let catalog: serde_json::Value = catalog_response
            .json()
            .await
            .map_err(|e| {
                println!("FoundryActor: ❌ Failed to parse catalog JSON: {:?}", e);
                format!("Failed to parse catalog: {:?}", e)
            })?;

        println!(
            "FoundryActor: Catalog response type: {}",
            if catalog.is_array() {
                "array"
            } else if catalog.is_object() {
                "object"
            } else {
                "other"
            }
        );

        // Handle both formats:
        // 1. { "models": [...] } - documented format
        // 2. [...] - direct array
        let models: Vec<&serde_json::Value> =
            if let Some(models_array) = catalog.get("models").and_then(|m| m.as_array()) {
                models_array.iter().collect()
            } else if let Some(direct_array) = catalog.as_array() {
                direct_array.iter().collect()
            } else {
                println!("FoundryActor: Catalog structure: {:?}", catalog);
                return Err(
                    "Invalid catalog format: expected 'models' array or direct array".to_string(),
                );
            };

        println!("FoundryActor: Found {} models in catalog", models.len());

        let model_info = models
            .iter()
            .find(|m| {
                let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let alias = m.get("alias").and_then(|a| a.as_str()).unwrap_or("");
                let display_name = m.get("displayName").and_then(|n| n.as_str()).unwrap_or("");
                name.to_lowercase().contains(&model_name.to_lowercase())
                    || alias.to_lowercase().contains(&model_name.to_lowercase())
                    || display_name
                        .to_lowercase()
                        .contains(&model_name.to_lowercase())
            })
            .ok_or_else(|| {
                format!(
                    "Model '{}' not found in catalog ({} models available)",
                    model_name,
                    models.len()
                )
            })?;

        // Build download request body
        let uri = model_info.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let name = model_info
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or(model_name);

        // Get version - could be string or integer in JSON
        let version: String = model_info
            .get("version")
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else if let Some(n) = v.as_u64() {
                    n.to_string()
                } else {
                    "1".to_string()
                }
            })
            .unwrap_or_else(|| "1".to_string());

        // Build model name with version, avoiding double version suffix
        // Some catalog entries may have the version already in the name (e.g., "Model:5")
        let model_name_with_version = if name.contains(':') {
            // Name already has version, use as-is
            name.to_string()
        } else {
            // Append version
            format!("{}:{}", name, version)
        };

        // Get prompt template if available
        let prompt_template = model_info
            .get("promptTemplate")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        let request_body = serde_json::json!({
            "model": {
                "Uri": uri,
                "ProviderType": "AzureFoundryLocal",
                "Name": model_name_with_version,
                "Publisher": "",
                "PromptTemplate": prompt_template
            },
            "ignorePipeReport": true
        });

        println!("FoundryActor: Download request body: {:?}", request_body);
        use std::io::Write;
        let _ = std::io::stdout().flush();

        // Send download request with streaming response
        println!("FoundryActor: Sending download request to {}...", url);
        let _ = std::io::stdout().flush();

        let response = client
            .post(&url)
            .json(&request_body)
            .timeout(Duration::from_secs(3600)) // 1 hour timeout for large models
            .send()
            .await
            .map_err(|e| {
                println!(
                    "FoundryActor: ❌ Download request failed for {}: {:?}",
                    model_name, e
                );
                println!(
                    "FoundryActor: Download error details - url: {}, is_timeout: {}, is_connect: {}",
                    url, e.is_timeout(), e.is_connect()
                );
                format!("Download request failed: {:?}", e)
            })?;

        println!(
            "FoundryActor: Download response status: {}",
            response.status()
        );
        let _ = std::io::stdout().flush();

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Download failed: HTTP {} - {}", status, text));
        }

        // Read streaming progress - the download happens during this phase
        println!("FoundryActor: Reading download stream...");
        let _ = std::io::stdout().flush();

        let mut buffer = String::new();
        let mut chunk_count = 0u32;
        let mut last_progress_log = std::time::Instant::now();

        // Use bytes_stream for better streaming handling
        let mut stream = response;
        while let Ok(Some(chunk)) = stream.chunk().await {
            chunk_count += 1;
            if let Ok(s) = String::from_utf8(chunk.to_vec()) {
                buffer.push_str(&s);

                // Log periodically to show we're receiving data
                if last_progress_log.elapsed() > Duration::from_secs(5) {
                    println!(
                        "FoundryActor: Received {} chunks, buffer len: {}",
                        chunk_count,
                        buffer.len()
                    );
                    let _ = std::io::stdout().flush();
                    last_progress_log = std::time::Instant::now();
                }

                // Parse progress updates: ("file name", percentage)
                // Look for complete lines
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    println!("FoundryActor: Download stream line: {}", line);
                    let _ = std::io::stdout().flush();

                    // Flexible parsing: look for any number followed by % anywhere in the line
                    // This handles formats like:
                    //   "Total 0.043% Downloading v4/model.onnx"
                    //   "0.5% complete"
                    //   "Progress: 50%"
                    //   ("filename", 0.5)  <- legacy format where 0.5 = 50%
                    if let Some(percent_pos) = line.find('%') {
                        // Walk backwards from % to find the number
                        let before_percent = &line[..percent_pos];
                        let number_start = before_percent
                            .rfind(|c: char| !c.is_ascii_digit() && c != '.')
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        let number_str = before_percent[number_start..].trim();

                        if let Ok(progress) = number_str.parse::<f32>() {
                            // Extract filename: anything after % that looks like a path
                            let after_percent = &line[percent_pos + 1..];
                            let filename = after_percent
                                .split_whitespace()
                                .find(|s| s.contains('/') || s.contains('.'))
                                .unwrap_or("")
                                .to_string();

                            println!(
                                "FoundryActor: Download progress: {} - {:.1}%",
                                if filename.is_empty() { "downloading" } else { &filename },
                                progress
                            );
                            let _ = std::io::stdout().flush();

                            // Emit progress event
                            let _ = self.app_handle.emit(
                                "model-download-progress",
                                serde_json::json!({
                                    "file": filename,
                                    "progress": progress
                                }),
                            );
                        }
                    }
                    // Handle legacy tuple format: ("filename", 0.5) where 0.5 means 50%
                    else if line.starts_with('(') && line.ends_with(')') {
                        let inner = &line[1..line.len() - 1];
                        let parts: Vec<&str> = inner.rsplitn(2, ',').collect();
                        if parts.len() == 2 {
                            let progress_str = parts[0].trim();
                            let file_part = parts[1].trim();
                            let filename = file_part.trim_matches('"').to_string();

                            if let Ok(progress) = progress_str.parse::<f32>() {
                                // Legacy format uses 0-1 scale, convert to percentage
                                let progress_percent = progress * 100.0;
                                println!(
                                    "FoundryActor: Download progress: {} - {:.1}%",
                                    filename, progress_percent
                                );
                                let _ = std::io::stdout().flush();

                                let _ = self.app_handle.emit(
                                    "model-download-progress",
                                    serde_json::json!({
                                        "file": filename,
                                        "progress": progress_percent
                                    }),
                                );
                            }
                        }
                    }
                    // Try to parse as JSON response (final status)
                    else if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                        println!("FoundryActor: Download JSON response: {:?}", json);
                        let _ = std::io::stdout().flush();
                    }
                }
            }
        }

        println!(
            "FoundryActor: Download stream ended. Total chunks: {}, remaining buffer: {}",
            chunk_count,
            buffer.len()
        );
        let _ = std::io::stdout().flush();

        // Check for any remaining content in buffer (final response)
        if !buffer.trim().is_empty() {
            println!("FoundryActor: Final buffer content: {}", buffer.trim());
            let _ = std::io::stdout().flush();

            // Try to parse as JSON
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&buffer) {
                if let Some(success) = json.get("Success").and_then(|v| v.as_bool()) {
                    if !success {
                        let error_msg = json
                            .get("ErrorMessage")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown error");
                        return Err(format!("Download failed: {}", error_msg));
                    }
                }
            }
        }

        // Note: Frontend should call fetchModels/fetchCachedModels to refresh after download
        println!("FoundryActor: Model download complete: {}", model_name);
        let _ = std::io::stdout().flush();
        Ok(())
    }

    /// Load a model into VRAM
    /// GET /openai/load/{name}
    async fn load_model_impl(
        &self,
        client: &reqwest::Client,
        port: u16,
        model_name: &str,
    ) -> Result<(), String> {
        // URL encode the model name in case it has special characters
        let encoded_name = urlencoding::encode(model_name);
        let url = format!(
            "http://127.0.0.1:{}/openai/load/{}?ttl=0",
            port, encoded_name
        );
        println!("FoundryActor: Loading model {} from {}", model_name, url);

        let response = client
            .get(&url)
            .timeout(Duration::from_secs(300)) // 5 minute timeout for loading
            .send()
            .await
            .map_err(|e| {
                println!(
                    "FoundryActor: ❌ Load model request failed for {}: {:?}",
                    model_name, e
                );
                println!(
                    "FoundryActor: Load error details - url: {}, is_timeout: {}, is_connect: {}",
                    url, e.is_timeout(), e.is_connect()
                );
                format!("Load request failed: {:?}", e)
            })?;

        if response.status().is_success() {
            println!("FoundryActor: Model loaded successfully: {}", model_name);

            // Emit success event
            let _ = self.app_handle.emit(
                "model-load-complete",
                serde_json::json!({
                    "model": model_name,
                    "success": true
                }),
            );

            // Update current model
            self.emit_model_selected(model_name);

            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let error_msg = format!("HTTP {} - {}", status, text);

            println!(
                "FoundryActor: Failed to load model {}: {}",
                model_name, error_msg
            );

            // Emit failure event
            let _ = self.app_handle.emit(
                "model-load-complete",
                serde_json::json!({
                    "model": model_name,
                    "success": false,
                    "error": error_msg
                }),
            );

            // On 5XX server errors, emit fallback event so the frontend can auto-switch
            // to a known-good model. This prevents users from getting stuck with a broken
            // model that always returns 500 errors.
            if status.is_server_error() && !model_name.to_lowercase().contains(DEFAULT_FALLBACK_MODEL) {
                self.emit_model_fallback_required(model_name, &error_msg);
            }

            Err(error_msg)
        }
    }

    /// Get currently loaded models
    /// GET /openai/loadedmodels
    async fn get_loaded_models_impl(&self, client: &reqwest::Client, port: u16) -> Vec<String> {
        let url = format!("http://127.0.0.1:{}/openai/loadedmodels", port);
        println!("FoundryActor: Getting loaded models from {}", url);

        match client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<Vec<String>>().await {
                        Ok(models) => {
                            println!("FoundryActor: Loaded models: {:?}", models);
                            models
                        }
                        Err(e) => {
                            println!("FoundryActor: ❌ Failed to parse loaded models JSON: {:?}", e);
                            Vec::new()
                        }
                    }
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    println!(
                        "FoundryActor: ❌ Get loaded models failed - HTTP {}: {}",
                        status, body
                    );
                    println!("FoundryActor: Request URL was: {}", url);
                    Vec::new()
                }
            }
            Err(e) => {
                println!("FoundryActor: ❌ Get loaded models request failed: {:?}", e);
                println!(
                    "FoundryActor: Request error details - url: {}, is_timeout: {}, is_connect: {}",
                    url, e.is_timeout(), e.is_connect()
                );
                Vec::new()
            }
        }
    }

    /// Get all models from the Foundry catalog
    /// GET /foundry/list
    async fn get_catalog_models_impl(
        &self,
        client: &reqwest::Client,
        port: u16,
    ) -> Vec<CatalogModel> {
        let url = format!("http://127.0.0.1:{}/foundry/list", port);
        println!("FoundryActor: Getting catalog models from {}", url);

        match client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<Vec<CatalogModel>>().await {
                        Ok(models) => {
                            println!("FoundryActor: Catalog contains {} models", models.len());
                            models
                        }
                        Err(e) => {
                            println!("FoundryActor: ❌ Failed to parse catalog models JSON: {:?}", e);
                            Vec::new()
                        }
                    }
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    println!(
                        "FoundryActor: ❌ Get catalog models failed - HTTP {}: {}",
                        status, body
                    );
                    println!("FoundryActor: Request URL was: {}", url);
                    Vec::new()
                }
            }
            Err(e) => {
                println!("FoundryActor: ❌ Get catalog models request failed: {:?}", e);
                println!(
                    "FoundryActor: Request error details - url: {}, is_timeout: {}, is_connect: {}",
                    url, e.is_timeout(), e.is_connect()
                );
                Vec::new()
            }
        }
    }

    /// Unload a model from memory
    /// GET /openai/unload/{name}
    async fn unload_model_impl(
        &self,
        client: &reqwest::Client,
        port: u16,
        model_name: &str,
    ) -> Result<(), String> {
        let encoded_name = urlencoding::encode(model_name);
        let url = format!(
            "http://127.0.0.1:{}/openai/unload/{}?force=true",
            port, encoded_name
        );
        println!("FoundryActor: Unloading model {} from {}", model_name, url);

        let response = client
            .get(&url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| {
                println!(
                    "FoundryActor: ❌ Unload model request failed for {}: {:?}",
                    model_name, e
                );
                println!(
                    "FoundryActor: Unload error details - url: {}, is_timeout: {}, is_connect: {}",
                    url, e.is_timeout(), e.is_connect()
                );
                format!("Unload request failed: {:?}", e)
            })?;

        if response.status().is_success() {
            println!("FoundryActor: Model unloaded successfully: {}", model_name);
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            println!(
                "FoundryActor: ❌ Failed to unload model {} - HTTP {}: {}",
                model_name, status, text
            );
            println!("FoundryActor: Unload request URL was: {}", url);
            Err(format!("HTTP {} - {}", status, text))
        }
    }

    /// Get Foundry service status
    /// GET /openai/status
    async fn get_service_status_impl(
        &self,
        client: &reqwest::Client,
        port: u16,
    ) -> Result<FoundryServiceStatus, String> {
        let url = format!("http://127.0.0.1:{}/openai/status", port);
        println!("FoundryActor: Getting service status from {}", url);

        let response = client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                println!(
                    "FoundryActor: ❌ Service status request failed: {:?} (url: {}, is_timeout: {}, is_connect: {})",
                    e, url, e.is_timeout(), e.is_connect()
                );
                format!("Status request failed: {:?}", e)
            })?;

        if response.status().is_success() {
            response
                .json::<FoundryServiceStatus>()
                .await
                .map_err(|e| {
                    println!("FoundryActor: ❌ Failed to parse service status JSON: {:?}", e);
                    format!("Failed to parse status: {:?}", e)
                })
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            println!(
                "FoundryActor: ❌ Service status request failed - HTTP {}: {}",
                status, body
            );
            println!("FoundryActor: Status request URL was: {}", url);
            Err(format!("HTTP {} - {}", status, body))
        }
    }

    /// Remove a model from the disk cache using CLI
    /// foundry cache remove --yes <model>
    async fn remove_cached_model_impl(&self, model_name: &str) -> Result<(), String> {
        let foundry_bin = find_foundry_binary();
        println!(
            "FoundryActor: Removing model from cache via CLI: {} (using {})",
            model_name, foundry_bin
        );

        let output = Command::new(&foundry_bin)
            .args(["cache", "remove", "--yes", model_name])
            .output()
            .await
            .map_err(|e| format!("Failed to run {} cache remove: {}", foundry_bin, e))?;

        if output.status.success() {
            println!("FoundryActor: Model removed from cache: {}", model_name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(format!(
                "Failed to remove model: {} {}",
                stdout.trim(),
                stderr.trim()
            ))
        }
    }
}
