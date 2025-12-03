use tokio::sync::mpsc;
use crate::protocol::{FoundryMsg, CachedModel, ModelInfo, ModelFamily, ToolFormat, ReasoningFormat, ChatMessage, OpenAITool, ParsedToolCall};
use serde_json::{json, Value};
use tokio::process::Command;
use std::time::Duration;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::time::{sleep, timeout};
use tauri::{AppHandle, Emitter};
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use crate::tool_adapters::parse_combined_tool_name;

/// Target embedding dimension (must match LanceDB schema)
const EMBEDDING_DIM: usize = 384;

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
            let entry = self.calls.entry(index).or_insert_with(|| {
                (String::new(), String::new(), String::new())
            });
            
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
        
        for (_index, (_id, name, arguments_str)) in indexed {
            // Skip entries without a name (incomplete)
            if name.is_empty() {
                continue;
            }
            
            // Parse the accumulated arguments JSON
            let arguments = if arguments_str.is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&arguments_str).unwrap_or_else(|e| {
                    println!("[StreamingToolCalls] Failed to parse arguments for {}: {}", name, e);
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
            
            result.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw,
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
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });
    
    // Note: EP (execution provider) parameter is not passed to completions
    // as it didn't work reliably. Foundry will auto-select the best EP.
    
    // Add model-family-specific parameters
    match family {
        ModelFamily::GptOss => {
            // GPT-OSS models: standard OpenAI-compatible parameters
            body["max_tokens"] = json!(16384);
            body["temperature"] = json!(0.7);
            
            if use_native_tools {
                body["tools"] = json!(tools);
            }
        }
        ModelFamily::Phi => {
            // Phi models: may support reasoning_effort
            if supports_reasoning && supports_reasoning_effort {
                println!("[FoundryActor] Phi model with reasoning, using effort: {}", reasoning_effort);
                body["max_tokens"] = json!(8192);
                body["reasoning_effort"] = json!(reasoning_effort);
                // Note: Reasoning models typically don't use tools in the same request
            } else if use_native_tools {
                body["max_tokens"] = json!(16384);
                body["tools"] = json!(tools);
            } else {
                body["max_tokens"] = json!(16384);
            }
        }
        ModelFamily::Gemma => {
            // Gemma models: support temperature and top_k
            body["max_tokens"] = json!(8192);
            body["temperature"] = json!(0.7);
            // Gemma supports top_k which is useful for controlling randomness
            body["top_k"] = json!(40);
            
            if use_native_tools {
                // Gemma may use a different tool format, but Foundry handles this
                body["tools"] = json!(tools);
            }
        }
        ModelFamily::Granite => {
            // IBM Granite models: support repetition_penalty
            body["max_tokens"] = json!(8192);
            body["temperature"] = json!(0.7);
            // Granite models benefit from repetition penalty
            body["repetition_penalty"] = json!(1.05);
            
            if supports_reasoning {
                // Granite reasoning models use <|thinking|> tags internally
                println!("[FoundryActor] Granite model with reasoning support");
            }
            
            if use_native_tools {
                body["tools"] = json!(tools);
            }
        }
        ModelFamily::Generic => {
            // Generic/unknown models: use safe defaults
            if supports_reasoning && supports_reasoning_effort {
                body["max_tokens"] = json!(8192);
                body["reasoning_effort"] = json!(reasoning_effort);
            } else if use_native_tools {
                body["max_tokens"] = json!(16384);
                body["tools"] = json!(tools);
            } else {
                body["max_tokens"] = json!(16384);
            }
        }
    }
    
    body
}

/// Result of parsing `foundry service status` output
struct ServiceStatus {
    port: Option<u16>,
    registered_eps: Vec<String>,
    valid_eps: Vec<String>,
}

pub struct FoundryActor {
    rx: mpsc::Receiver<FoundryMsg>,
    port: Option<u16>,
    model_id: Option<String>,
    available_models: Vec<String>,
    model_info: Vec<ModelInfo>,
    app_handle: AppHandle,
    embedding_model: Option<Arc<TextEmbedding>>,
    /// Execution Providers successfully registered by Foundry
    registered_eps: Vec<String>,
    /// All valid Execution Providers available on this system
    valid_eps: Vec<String>,
}

impl FoundryActor {
    pub fn new(rx: mpsc::Receiver<FoundryMsg>, app_handle: AppHandle) -> Self {
        Self { 
            rx, 
            port: None, 
            model_id: None, 
            available_models: Vec::new(), 
            model_info: Vec::new(),
            app_handle,
            embedding_model: None,
            registered_eps: Vec::new(),
            valid_eps: Vec::new(),
        }
    }

    pub async fn run(mut self) {
        println!("Initializing Foundry Local Manager via CLI...");
        
        // Initialize local embedding model (all-MiniLM-L6-v2, 384 dimensions)
        println!("FoundryActor: Initializing local embedding model (all-MiniLM-L6-v2)...");
        match tokio::task::spawn_blocking(|| {
            let mut options = InitOptions::default();
            options.model_name = EmbeddingModel::AllMiniLML6V2;
            options.show_download_progress = true;
            TextEmbedding::try_new(options)
        }).await {
            Ok(Ok(model)) => {
                println!("FoundryActor: Embedding model loaded successfully");
                self.embedding_model = Some(Arc::new(model));
            }
            Ok(Err(e)) => {
                println!("FoundryActor ERROR: Failed to load embedding model: {}", e);
            }
            Err(e) => {
                println!("FoundryActor ERROR: Embedding model initialization task panicked: {}", e);
            }
        }
        
        // Try to start the service or ensure it's running
        if let Err(e) = self.ensure_service_running().await {
            println!("Warning: Failed to ensure Foundry service is running: {}", e);
        }

        // Try to get the port and model with retries
        // Foundry may take time to start up, so we retry with exponential backoff
        self.update_connection_info_with_retry(5, Duration::from_secs(2)).await;

        let client = reqwest::Client::new();

        while let Some(msg) = self.rx.recv().await {
            match msg {
                FoundryMsg::GetEmbedding { text, respond_to } => {
                    // Generate embeddings using local fastembed model
                    if let Some(model) = &self.embedding_model {
                        let model_clone = Arc::clone(model);
                        let text_clone = text.clone();
                        
                        println!("FoundryActor: Generating embedding locally (text len: {})", text.len());
                        
                        match tokio::task::spawn_blocking(move || {
                            model_clone.embed(vec![text_clone], None)
                        }).await {
                            Ok(Ok(embeddings)) => {
                                if let Some(embedding) = embeddings.into_iter().next() {
                                    println!("FoundryActor: Generated embedding (dim: {})", embedding.len());
                                    let _ = respond_to.send(embedding);
                                } else {
                                    println!("FoundryActor ERROR: Empty embedding result, using fallback");
                                    let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                                }
                            }
                            Ok(Err(e)) => {
                                println!("FoundryActor ERROR: Embedding generation failed: {}", e);
                                let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                            }
                            Err(e) => {
                                println!("FoundryActor ERROR: Embedding task panicked: {}", e);
                                let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                            }
                        }
                    } else {
                        println!("FoundryActor WARNING: Embedding model not loaded, using fallback");
                        let _ = respond_to.send(vec![0.0; EMBEDDING_DIM]);
                    }
                }
                FoundryMsg::GetModels { respond_to } => {
                    if self.port.is_none() {
                        // Only retry if service is unreachable (not if models list is empty)
                        self.update_connection_info_with_retry(3, Duration::from_secs(1)).await;
                    }
                    let _ = respond_to.send(self.available_models.clone());
                }
                FoundryMsg::GetModelInfo { respond_to } => {
                    if self.port.is_none() {
                        // Only retry if service is unreachable (not if model info is empty)
                        self.update_connection_info_with_retry(3, Duration::from_secs(1)).await;
                    }
                    let _ = respond_to.send(self.model_info.clone());
                }
                FoundryMsg::GetCachedModels { respond_to } => {
                    // Use REST API to get models (same as available_models)
                    // Convert to CachedModel format for compatibility
                    let cached: Vec<CachedModel> = self.available_models.iter()
                        .map(|model_id| CachedModel {
                            alias: model_id.clone(), // Use model_id as alias since REST API doesn't provide aliases
                            model_id: model_id.clone(),
                        })
                        .collect();
                    let _ = respond_to.send(cached);
                }
                FoundryMsg::SetModel { model_id, respond_to } => {
                    self.model_id = Some(model_id.clone());
                    self.emit_model_selected(&model_id);
                    let _ = respond_to.send(true);
                }
                FoundryMsg::Chat { history, reasoning_effort, tools, respond_to } => {
                     // Check if we need to restart/reconnect
                     if self.port.is_none() || self.available_models.is_empty() {
                         println!("FoundryActor: No models found or port missing. Attempting to restart service...");
                         
                         // First try to just reconnect (maybe service started in meantime)
                         if !self.update_connection_info_with_retry(2, Duration::from_secs(1)).await {
                             // Still not working, restart the service
                             println!("FoundryActor: Quick reconnect failed, restarting service...");
                             
                             // Restart service
                             if let Err(e) = self.restart_service().await {
                                 println!("FoundryActor: Failed to restart service: {}", e);
                                 let _ = respond_to.send(format!("Error: Failed to restart local model service. Please ensure Foundry is installed: {}", e));
                                 continue;
                             }
                             
                             // Update info with longer retry
                             if !self.update_connection_info_with_retry(5, Duration::from_secs(2)).await {
                                 let _ = respond_to.send("Error: Could not connect to Foundry service after restart. Please check if Foundry is running.".to_string());
                                 continue;
                             }
                         }
                     }

                     if let Some(port) = self.port {
                         // Use detected model or default to "Phi-4-generic-gpu:1" if detection failed but port is open
                         let model = self.model_id.clone().unwrap_or_else(|| "Phi-4-generic-gpu:1".to_string());
                         
                         let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
                         
                         // Log incoming messages for debugging
                         println!("\n[FoundryActor] Received {} messages:", history.len());
                         for (i, msg) in history.iter().enumerate() {
                             let preview: String = msg.content.chars().take(100).collect();
                             println!("  [{}] role={}, len={}, preview: {}...", 
                                 i, msg.role, msg.content.len(), preview);
                         }
                         
                         // For reasoning models, ensure we have a system message that instructs
                         // the model to provide a final answer after thinking
                         let mut messages = history.clone();
                         let has_system_msg = messages.iter().any(|m| m.role == "system");
                         
                         println!("[FoundryActor] has_system_msg={}", has_system_msg);
                         
                        if !has_system_msg {
                            // Prepend system message
                            println!("[FoundryActor] WARNING: No system message found, adding default!");
                            messages.insert(0, crate::protocol::ChatMessage {
                                role: "system".to_string(),
                                content: "You are a helpful AI assistant.".to_string(),
                            });
                        } else {
                             // Log the actual system message being used
                             if let Some(sys_msg) = messages.iter().find(|m| m.role == "system") {
                                 println!("[FoundryActor] Using system message ({} chars)", sys_msg.content.len());
                                 // Log first 500 chars of system prompt
                                 let sys_preview: String = sys_msg.content.chars().take(500).collect();
                                 println!("[FoundryActor] System prompt preview:\n{}", sys_preview);
                             }
                         }
                         
                        // Get model info for this model
                        let model_info = self.model_info.iter()
                            .find(|m| m.id == model)
                            .cloned();
                        
                        // Determine capabilities from model info or heuristics
                        let model_supports_reasoning = model_info.as_ref()
                            .map(|m| m.reasoning)
                            .unwrap_or_else(|| model.to_lowercase().contains("reasoning"));
                        
                        let model_supports_tools = model_info.as_ref()
                            .map(|m| m.tool_calling)
                            .unwrap_or(false);
                        
                        let supports_reasoning_effort = model_info.as_ref()
                            .map(|m| m.supports_reasoning_effort)
                            .unwrap_or(false);
                        
                        let model_family = model_info.as_ref()
                            .map(|m| m.family)
                            .unwrap_or(ModelFamily::Generic);
                        
                        // Only use native tools if model supports them AND tools were provided
                        let use_native_tools = model_supports_tools && tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
                        
                        println!("[FoundryActor] Model: {} | family: {:?} | reasoning: {} | tools: {} | reasoning_effort: {}",
                            model, model_family, model_supports_reasoning, use_native_tools, supports_reasoning_effort);
                        
                            if use_native_tools {
                                println!("[FoundryActor] Including {} native tools in request", 
                                    tools.as_ref().map(|t| t.len()).unwrap_or(0));
                                // Log the tools being sent for debugging
                                if let Some(ref tool_list) = tools {
                                    for tool in tool_list {
                                        println!("[FoundryActor] Tool: {} - {:?}", 
                                            tool.function.name, 
                                            tool.function.description.as_deref().unwrap_or("(no description)"));
                                    }
                                }
                            } else if tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
                                println!("[FoundryActor] Model does NOT support native tool calling, falling back to text-based tools");
                            }
                        
                        println!("Sending streaming request to Foundry at {}", url);
                         
                         let client_clone = client.clone();
                         let respond_to_clone = respond_to.clone();
                         
                         // Retry logic with exponential backoff for 4XX errors
                         const MAX_RETRIES: u32 = 3;
                         let mut retry_delay = Duration::from_secs(2);
                         let mut last_error: Option<String>;
                         
                         for attempt in 1..=MAX_RETRIES {
                             // Rebuild URL in case port changed after restart
                             let current_url = if let Some(p) = self.port {
                                 format!("http://127.0.0.1:{}/v1/chat/completions", p)
                             } else {
                                 url.clone()
                             };
                             
                             // Rebuild body in case anything changed after restart
                             let current_body = build_chat_request_body(
                                 &model,
                                 model_family,
                                 &messages,
                                 &tools,
                                 use_native_tools,
                                 model_supports_reasoning,
                                 supports_reasoning_effort,
                                 &reasoning_effort,
                             );
                             
                             // Log the request body for debugging (truncate large content)
                             if use_native_tools {
                                 if let Some(tools_json) = current_body.get("tools") {
                                     println!("[FoundryActor] Request body 'tools' field:\n{}", 
                                         serde_json::to_string_pretty(tools_json).unwrap_or_default());
                                 }
                             }
                             
                             match client_clone.post(&current_url).json(&current_body).send().await {
                                Ok(mut resp) => {
                                    let status = resp.status();
                                    
                                    // Handle 4XX client errors with retry and service restart
                                    if status.is_client_error() {
                                        let text = resp.text().await.unwrap_or_default();
                                        println!("FoundryActor: 4XX error ({}) on attempt {}/{}: {}", 
                                            status, attempt, MAX_RETRIES, text);
                                        last_error = Some(format!("HTTP {}: {}", status, text));
                                        
                                        if attempt < MAX_RETRIES {
                                            // Restart service and re-detect port/EPs
                                            println!("FoundryActor: Restarting service due to 4XX error...");
                                            if let Err(e) = self.restart_service().await {
                                                println!("FoundryActor: Service restart failed: {}", e);
                                            }
                                            
                                            // Re-detect port and EPs after restart
                                            let status = self.detect_port_and_eps().await;
                                            self.port = status.port;
                                            self.registered_eps = status.registered_eps;
                                            self.valid_eps = status.valid_eps;
                                            
                                            println!("FoundryActor: Waiting {:?} before retry...", retry_delay);
                                            sleep(retry_delay).await;
                                            retry_delay = Duration::from_millis(
                                                (retry_delay.as_millis() as f64 * 1.5) as u64
                                            ).min(Duration::from_secs(10));
                                            continue;
                                        }
                                    } else if !status.is_success() {
                                        // Other non-success errors (5XX, etc.)
                                        let text = resp.text().await.unwrap_or_default();
                                        println!("Foundry error ({}): {}", status, text);
                                        let _ = respond_to_clone.send(format!("Error: {}", text));
                                        break;
                                    } else {
                                        // Success - stream the response
                                        let mut buffer = String::new();
                                        let mut streaming_tool_calls = StreamingToolCalls::default();
                                        println!("Foundry stream started.");
                                        
                                        // Note: Tool calls can arrive in two formats:
                                        // 1. Text-based: in content field as <tool_call>JSON</tool_call>
                                        // 2. Native OpenAI: in delta.tool_calls array (accumulated here)
                                        
                                        while let Ok(Some(chunk)) = resp.chunk().await {
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
                                                            println!("Foundry stream DONE.");
                                                            break;
                                                        }
                                                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                                            // Stream content tokens (includes tool calls in <tool_call> format)
                                                            if let Some(content) = json["choices"][0]["delta"]["content"].as_str() {
                                                                if !content.is_empty() {
                                                                    let _ = respond_to_clone.send(content.to_string());
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
                                        
                                        // After stream ends, emit any accumulated native tool calls as text
                                        // so the existing agentic loop parser can detect them
                                        if !streaming_tool_calls.is_empty() {
                                            let native_calls = streaming_tool_calls.into_parsed_calls();
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
                                },
                                Err(e) => {
                                    println!("Failed to call Foundry (attempt {}/{}): {}", attempt, MAX_RETRIES, e);
                                    last_error = Some(format!("Connection error: {}", e));
                                    
                                    if attempt < MAX_RETRIES {
                                        // Restart service on connection errors too
                                        println!("FoundryActor: Restarting service due to connection error...");
                                        if let Err(restart_err) = self.restart_service().await {
                                            println!("FoundryActor: Service restart failed: {}", restart_err);
                                        }
                                        
                                        // Re-detect port and EPs
                                        let status = self.detect_port_and_eps().await;
                                        self.port = status.port;
                                        self.registered_eps = status.registered_eps;
                                        self.valid_eps = status.valid_eps;
                                        
                                        sleep(retry_delay).await;
                                        retry_delay = Duration::from_millis(
                                            (retry_delay.as_millis() as f64 * 1.5) as u64
                                        ).min(Duration::from_secs(10));
                                        continue;
                                    }
                                }
                             }
                             
                             // If we get here with an error after max retries, report it
                             if let Some(err) = &last_error {
                                 if attempt == MAX_RETRIES {
                                     let _ = respond_to_clone.send(format!("Error after {} retries: {}", MAX_RETRIES, err));
                                 }
                             }
                             break;
                         }
                     } else {
                         println!("Foundry endpoint not available (port not found).");
                         let _ = respond_to.send("The local model service is not available. Please check if Foundry is installed and running.".to_string());
                     }
                }
                FoundryMsg::DownloadModel { model_name, respond_to } => {
                    println!("FoundryActor: Downloading model: {}", model_name);
                    if let Some(port) = self.port {
                        let result = self.download_model_impl(&client, port, &model_name).await;
                        let _ = respond_to.send(result);
                    } else {
                        let _ = respond_to.send(Err("Foundry service not available".to_string()));
                    }
                }
                FoundryMsg::LoadModel { model_name, respond_to } => {
                    println!("FoundryActor: Loading model into VRAM: {}", model_name);
                    if let Some(port) = self.port {
                        let result = self.load_model_impl(&client, port, &model_name).await;
                        let _ = respond_to.send(result);
                    } else {
                        let _ = respond_to.send(Err("Foundry service not available".to_string()));
                    }
                }
                FoundryMsg::GetLoadedModels { respond_to } => {
                    println!("FoundryActor: Getting loaded models");
                    if let Some(port) = self.port {
                        let models = self.get_loaded_models_impl(&client, port).await;
                        let _ = respond_to.send(models);
                    } else {
                        let _ = respond_to.send(Vec::new());
                    }
                }
                FoundryMsg::Reload { respond_to } => {
                    println!("FoundryActor: Reloading foundry service...");
                    
                    // Restart the service
                    let result = match self.restart_service().await {
                        Ok(()) => {
                            // Re-detect port, endpoints, and available models after restart
                            self.update_connection_info().await;
                            
                            println!("FoundryActor: Service reloaded successfully. Port: {:?}, Models: {}", 
                                self.port, self.available_models.len());
                            Ok(())
                        }
                        Err(e) => {
                            println!("FoundryActor: Failed to reload service: {}", e);
                            Err(format!("Failed to reload service: {}", e))
                        }
                    };
                    
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
                println!("FoundryActor: Warning - no valid EPs available, models may not be loadable");
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
            self.available_models = models.clone();
            
            // Build model info with inferred capabilities from model names
            self.model_info = models.iter()
                .map(|model_id| {
                    let id = model_id.clone();
                    let id_lower = id.to_lowercase();
                    
                    // Detect model family from ID
                    let family = ModelFamily::from_model_id(&id);
                    
                    // Infer tool calling support from model name
                    // Most modern instruction-tuned models support tool calling
                    // Either natively or via text-based format (Hermes-style <tool_call>)
                    let tool_calling = id_lower.contains("instruct")
                        || id_lower.contains("coder")
                        || id_lower.contains("qwen")
                        || id_lower.contains("phi")
                        || id_lower.contains("granite")
                        || id_lower.contains("llama")
                        || id_lower.contains("mistral")
                        || id_lower.contains("gemma")
                        || id_lower.contains("chat");
                    
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
                    
                    println!("  Model: {} | family: {:?} | toolCalling: {} ({:?}) | vision: {} | reasoning: {} ({:?})", 
                        id, family, tool_calling, tool_format, vision, reasoning, reasoning_format);
                    
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
                        supports_temperature,
                        supports_top_p,
                        supports_reasoning_effort,
                    }
                })
                .collect();
            
            // Select first model as default if none selected
            if self.model_id.is_none() {
                if let Some(first) = self.available_models.first() {
                    println!("Selected default model: {}", first);
                    self.model_id = Some(first.clone());
                    self.emit_model_selected(first);
                }
            }
            
            println!("FoundryActor: Found {} models via REST API", self.available_models.len());
            return true;

        } else {
             println!("Warning: Could not detect Foundry service port.");
        }
        false
    }
    
    /// Get models via REST API: GET /openai/models
    /// Returns a list of model names as strings
    async fn get_models_via_rest(&self, port: u16) -> Vec<String> {
        let url = format!("http://127.0.0.1:{}/openai/models", port);
        println!("FoundryActor: Fetching models via REST API: {}", url);
        
        let client = reqwest::Client::new();
        match client.get(&url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    // Response is an array of model names: ["model1", "model2"]
                    match resp.json::<Vec<String>>().await {
                        Ok(models) => {
                            println!("FoundryActor: REST API returned {} models", models.len());
                            models
                        }
                        Err(e) => {
                            println!("FoundryActor: Failed to parse models response: {}", e);
                            Vec::new()
                        }
                    }
                } else {
                    println!("FoundryActor: REST API error: {}", resp.status());
                    Vec::new()
                }
            }
            Err(e) => {
                println!("FoundryActor: Failed to call REST API: {}", e);
                Vec::new()
            }
        }
    }
    
    /// Try to update connection info with exponential backoff
    /// Returns true if successfully connected and found models
    async fn update_connection_info_with_retry(&mut self, max_retries: u32, initial_delay: Duration) -> bool {
        let mut delay = initial_delay;
        
        for attempt in 1..=max_retries {
            println!("FoundryActor: Connection attempt {}/{}", attempt, max_retries);
            
            if self.update_connection_info().await {
                println!("FoundryActor: Successfully connected to Foundry on attempt {}", attempt);
                return true;
            }
            
            if attempt < max_retries {
                println!("FoundryActor: Attempt {} failed, retrying in {:?}...", attempt, delay);
                sleep(delay).await;
                delay = Duration::from_millis((delay.as_millis() as f64 * 1.5) as u64).min(Duration::from_secs(10));
            }
        }
        
        println!("FoundryActor: Failed to connect after {} attempts", max_retries);
        false
    }

    fn emit_model_selected(&self, model: &str) {
        let _ = self.app_handle.emit("model-selected", model.to_string());
    }

    async fn ensure_service_running(&self) -> std::io::Result<()> {
        println!("FoundryActor: Checking/Starting Foundry service...");
        // Try to start service via CLI: `foundry service start`
        // We use a timeout to prevent hanging indefinitely
        let child = Command::new("foundry")
            .args(&["service", "start"])
            .output();
            
        let output = match timeout(Duration::from_secs(10), child).await {
            Ok(res) => res?,
            Err(_) => {
                println!("FoundryActor: 'foundry service start' timed out.");
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "foundry service start timed out"));
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

    async fn restart_service(&self) -> std::io::Result<()> {
        println!("Restarting Foundry service...");
        
        // Emit event: service restart started
        let _ = self.app_handle.emit("service-restart-started", json!({
            "message": "Restarting Foundry service..."
        }));
        
        // Run `foundry service restart` with timeout to prevent hanging
        let child = Command::new("foundry")
            .args(&["service", "restart"])
            .output();
            
        let output = match timeout(Duration::from_secs(15), child).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                println!("FoundryActor: Failed to run 'foundry service restart': {}", e);
                let _ = self.app_handle.emit("service-restart-complete", json!({
                    "success": false,
                    "error": format!("Failed to run foundry command: {}", e)
                }));
                return Err(e);
            }
            Err(_) => {
                println!("FoundryActor: 'foundry service restart' timed out after 15s");
                let _ = self.app_handle.emit("service-restart-complete", json!({
                    "success": false,
                    "error": "Service restart timed out after 15 seconds"
                }));
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "foundry service restart timed out"));
            }
        };
            
        if output.status.success() {
             println!("Foundry service restart command issued successfully.");
        } else {
             let stderr = String::from_utf8_lossy(&output.stderr);
             println!("Foundry service restart command failed: {}", stderr);
             // If restart fails (e.g. not running), try start
             if let Err(e) = self.ensure_service_running().await {
                 let _ = self.app_handle.emit("service-restart-complete", json!({
                     "success": false,
                     "error": format!("Failed to start service: {}", e)
                 }));
                 return Err(e);
             }
        }
        
        // Wait for service to be ready
        println!("Waiting for service to be ready...");
        sleep(Duration::from_secs(3)).await;
        
        // Emit event: service restart completed successfully
        let _ = self.app_handle.emit("service-restart-complete", json!({
            "success": true,
            "message": "Service restarted successfully"
        }));
        
        Ok(())
    }

    /// Detect port and Execution Providers via `foundry service status`
    /// 
    /// Parses output like:
    /// ```
    /// 🟢 Model management service is running on http://127.0.0.1:54657/openai/status
    /// EP autoregistration status: Successfully downloaded and registered the following EPs: NvTensorRTRTXExecutionProvider, OpenVINOExecutionProvider.
    /// Valid EPs: CPUExecutionProvider, WebGpuExecutionProvider, NvTensorRTRTXExecutionProvider, OpenVINOExecutionProvider, CUDAExecutionProvider
    /// ```
    async fn detect_port_and_eps(&self) -> ServiceStatus {
        println!("FoundryActor: Detecting port and EPs via 'foundry service status'...");
        
        let child = Command::new("foundry")
            .args(&["service", "status"])
            .output();
            
        match timeout(Duration::from_secs(5), child).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                self.parse_service_status(&stdout)
            }
            Ok(Err(e)) => {
                println!("Failed to run foundry status: {}", e);
                ServiceStatus { port: None, registered_eps: Vec::new(), valid_eps: Vec::new() }
            }
            Err(_) => {
                println!("FoundryActor: 'foundry service status' timed out.");
                ServiceStatus { port: None, registered_eps: Vec::new(), valid_eps: Vec::new() }
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
        
        ServiceStatus { port, registered_eps, valid_eps }
    }
    
    /// Download a model from the Foundry catalog
    /// POST /openai/download with streaming progress
    async fn download_model_impl(&self, client: &reqwest::Client, port: u16, model_name: &str) -> Result<(), String> {
        let url = format!("http://127.0.0.1:{}/openai/download", port);
        println!("FoundryActor: Downloading model {} from {}", model_name, url);
        
        // Get model info from the catalog first to build the proper download request
        let catalog_url = format!("http://127.0.0.1:{}/foundry/list", port);
        let catalog_response = client.get(&catalog_url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch catalog: {}", e))?;
        
        if !catalog_response.status().is_success() {
            return Err(format!("Failed to fetch catalog: HTTP {}", catalog_response.status()));
        }
        
        let catalog: serde_json::Value = catalog_response.json()
            .await
            .map_err(|e| format!("Failed to parse catalog: {}", e))?;
        
        println!("FoundryActor: Catalog response type: {}", 
            if catalog.is_array() { "array" } 
            else if catalog.is_object() { "object" } 
            else { "other" });
        
        // Handle both formats:
        // 1. { "models": [...] } - documented format
        // 2. [...] - direct array
        let models: Vec<&serde_json::Value> = if let Some(models_array) = catalog.get("models").and_then(|m| m.as_array()) {
            models_array.iter().collect()
        } else if let Some(direct_array) = catalog.as_array() {
            direct_array.iter().collect()
        } else {
            println!("FoundryActor: Catalog structure: {:?}", catalog);
            return Err("Invalid catalog format: expected 'models' array or direct array".to_string());
        };
        
        println!("FoundryActor: Found {} models in catalog", models.len());
        
        let model_info = models.iter()
            .find(|m| {
                let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let alias = m.get("alias").and_then(|a| a.as_str()).unwrap_or("");
                let display_name = m.get("displayName").and_then(|n| n.as_str()).unwrap_or("");
                name.to_lowercase().contains(&model_name.to_lowercase()) ||
                alias.to_lowercase().contains(&model_name.to_lowercase()) ||
                display_name.to_lowercase().contains(&model_name.to_lowercase())
            })
            .ok_or_else(|| format!("Model '{}' not found in catalog ({} models available)", model_name, models.len()))?;
        
        // Build download request body
        let uri = model_info.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let name = model_info.get("name").and_then(|n| n.as_str()).unwrap_or(model_name);
        
        // Get version - could be string or integer in JSON
        let version: String = model_info.get("version")
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
        let prompt_template = model_info.get("promptTemplate").cloned()
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
        
        // Send download request with streaming response
        let mut response = client.post(&url)
            .json(&request_body)
            .timeout(Duration::from_secs(3600)) // 1 hour timeout for large models
            .send()
            .await
            .map_err(|e| format!("Download request failed: {}", e))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Download failed: HTTP {} - {}", status, text));
        }
        
        // Read streaming progress
        let mut buffer = String::new();
        while let Ok(Some(chunk)) = response.chunk().await {
            if let Ok(s) = String::from_utf8(chunk.to_vec()) {
                buffer.push_str(&s);
                
                // Parse progress updates: ("file name", percentage)
                // Look for complete lines
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    
                    if line.is_empty() {
                        continue;
                    }
                    
                    // Try to parse progress: ("filename", 0.5) format
                    if line.starts_with('(') && line.ends_with(')') {
                        let inner = &line[1..line.len()-1];
                        let parts: Vec<&str> = inner.rsplitn(2, ',').collect();
                        if parts.len() == 2 {
                            let progress_str = parts[0].trim();
                            let file_part = parts[1].trim();
                            
                            // Extract filename (remove quotes)
                            let filename = file_part.trim_matches('"').to_string();
                            
                            // Parse progress
                            if let Ok(progress) = progress_str.parse::<f32>() {
                                let progress_percent = progress * 100.0;
                                println!("FoundryActor: Download progress: {} - {:.1}%", filename, progress_percent);
                                
                                // Emit progress event
                                let _ = self.app_handle.emit("model-download-progress", serde_json::json!({
                                    "file": filename,
                                    "progress": progress_percent
                                }));
                            }
                        }
                    }
                }
            }
        }
        
        // Note: Frontend should call fetchModels/fetchCachedModels to refresh after download
        println!("FoundryActor: Model download complete: {}", model_name);
        Ok(())
    }
    
    /// Load a model into VRAM
    /// GET /openai/load/{name}
    async fn load_model_impl(&self, client: &reqwest::Client, port: u16, model_name: &str) -> Result<(), String> {
        // URL encode the model name in case it has special characters
        let encoded_name = urlencoding::encode(model_name);
        let url = format!("http://127.0.0.1:{}/openai/load/{}?ttl=0", port, encoded_name);
        println!("FoundryActor: Loading model {} from {}", model_name, url);
        
        let response = client.get(&url)
            .timeout(Duration::from_secs(300)) // 5 minute timeout for loading
            .send()
            .await
            .map_err(|e| format!("Load request failed: {}", e))?;
        
        if response.status().is_success() {
            println!("FoundryActor: Model loaded successfully: {}", model_name);
            
            // Emit success event
            let _ = self.app_handle.emit("model-load-complete", serde_json::json!({
                "model": model_name,
                "success": true
            }));
            
            // Update current model
            self.emit_model_selected(model_name);
            
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let error_msg = format!("HTTP {} - {}", status, text);
            
            println!("FoundryActor: Failed to load model {}: {}", model_name, error_msg);
            
            // Emit failure event
            let _ = self.app_handle.emit("model-load-complete", serde_json::json!({
                "model": model_name,
                "success": false,
                "error": error_msg
            }));
            
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
                            println!("FoundryActor: Failed to parse loaded models: {}", e);
                            Vec::new()
                        }
                    }
                } else {
                    println!("FoundryActor: Get loaded models failed: HTTP {}", resp.status());
                    Vec::new()
                }
            }
            Err(e) => {
                println!("FoundryActor: Get loaded models request failed: {}", e);
                Vec::new()
            }
        }
    }

}
