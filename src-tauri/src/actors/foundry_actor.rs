use tokio::sync::mpsc;
use crate::protocol::{FoundryMsg, CachedModel, ModelInfo, ModelFamily, ToolFormat, ReasoningFormat, ChatMessage, OpenAITool};
use serde_json::{json, Value};
use tokio::process::Command;
use std::time::Duration;
use std::sync::Arc;
use tokio::time::{sleep, timeout};
use tauri::{AppHandle, Emitter};
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};

/// Target embedding dimension (must match LanceDB schema)
const EMBEDDING_DIM: usize = 384;

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

pub struct FoundryActor {
    rx: mpsc::Receiver<FoundryMsg>,
    port: Option<u16>,
    model_id: Option<String>,
    available_models: Vec<String>,
    model_info: Vec<ModelInfo>,
    app_handle: AppHandle,
    embedding_model: Option<Arc<TextEmbedding>>,
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
                    if self.port.is_none() || self.available_models.is_empty() {
                        // Retry with exponential backoff if still not connected
                        self.update_connection_info_with_retry(3, Duration::from_secs(1)).await;
                    }
                    let _ = respond_to.send(self.available_models.clone());
                }
                FoundryMsg::GetModelInfo { respond_to } => {
                    if self.port.is_none() || self.model_info.is_empty() {
                        // Retry with exponential backoff if still not connected
                        self.update_connection_info_with_retry(3, Duration::from_secs(1)).await;
                    }
                    let _ = respond_to.send(self.model_info.clone());
                }
                FoundryMsg::GetCachedModels { respond_to } => {
                    let cached = self.get_cached_models().await;
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
                        } else if tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
                            println!("[FoundryActor] Model does NOT support native tool calling, falling back to text-based tools");
                        }
                        
                        // Build request body with model-family-specific parameters
                        let body = build_chat_request_body(
                            &model,
                            model_family,
                            &messages,
                            &tools,
                            use_native_tools,
                            model_supports_reasoning,
                            supports_reasoning_effort,
                            &reasoning_effort,
                        );
                         
                         println!("Sending streaming request to Foundry at {}", url);
                         
                         let client_clone = client.clone();
                         let respond_to_clone = respond_to.clone();
                         
                         match client_clone.post(&url).json(&body).send().await {
                            Ok(mut resp) => {
                                let status = resp.status();
                                if !status.is_success() {
                                     let text = resp.text().await.unwrap_or_default();
                                     println!("Foundry error ({}): {}", status, text);
                                     let _ = respond_to_clone.send(format!("Error: {}", text));
                                } else {
                                    let mut buffer = String::new();
                                    println!("Foundry stream started.");
                                    
                                    // Note: In streaming mode, Foundry Local provides tool calls in the 
                                    // content field using Hermes-style <tool_call> XML format, not in the
                                    // structured tool_calls array. Our text-based parser handles this.
                                    
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
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    
                                    println!("Foundry stream loop finished.");
                                }
                            },
                            Err(e) => {
                                println!("Failed to call Foundry: {}", e);
                                let _ = respond_to_clone.send(format!("Error connecting to local model: {}", e));
                            }
                         }
                     } else {
                         println!("Foundry endpoint not available (port not found).");
                         let _ = respond_to.send("The local model service is not available. Please check if Foundry is installed and running.".to_string());
                     }
                }
            }
        }
    }

    async fn update_connection_info(&mut self) -> bool {
        self.port = self.detect_port().await;
        if let Some(p) = self.port {
            println!("Foundry service detected on port {}", p);
            
            // Get CLI-reported capabilities (more reliable than API for some models)
            let cli_capabilities = self.get_model_capabilities_from_cli().await;
            
            // Fetch available models from API
            let client = reqwest::Client::new();
            let models_url = format!("http://127.0.0.1:{}/v1/models", p);
            match client.get(&models_url).send().await {
                Ok(resp) => {
                     match resp.json::<serde_json::Value>().await {
                         Ok(json) => {
                             println!("Available models from API: {}", json);
                            if let Some(data) = json["data"].as_array() {
                                // Extract just model IDs for backwards compatibility
                                self.available_models = data.iter()
                                    .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                                    .collect();
                                
                                // Parse full model info including capabilities
                                self.model_info = data.iter()
                                    .filter_map(|m| {
                                        let id = m["id"].as_str()?.to_string();
                                        
                                        // Detect model family from ID
                                        let family = ModelFamily::from_model_id(&id);
                                        
                                        // Get tool_calling from API
                                        let api_tool_calling = m["toolCalling"].as_bool().unwrap_or(false);
                                        
                                        // Check CLI capabilities - this is more reliable
                                        // The API sometimes incorrectly reports toolCalling: false
                                        let cli_tool_calling = cli_capabilities.get(&id).copied().unwrap_or(false);
                                        
                                        // Use CLI value if it says tools are supported (API might be wrong)
                                        let tool_calling = api_tool_calling || cli_tool_calling;
                                        
                                        if cli_tool_calling && !api_tool_calling {
                                            println!("  Model: {} | API says toolCalling: false, but CLI says tools supported - using CLI", id);
                                        }
                                        
                                        // Determine tool format based on model family and capabilities
                                        let tool_format = if !tool_calling {
                                            ToolFormat::TextBased
                                        } else {
                                            match family {
                                                ModelFamily::GptOss => ToolFormat::OpenAI,
                                                ModelFamily::Gemma => ToolFormat::Gemini,
                                                ModelFamily::Phi => ToolFormat::Hermes,
                                                ModelFamily::Granite => ToolFormat::Granite,
                                                ModelFamily::Generic => ToolFormat::OpenAI,
                                            }
                                        };
                                        
                                        let vision = m["vision"].as_bool().unwrap_or(false);
                                        // Check API field first, fallback to heuristic (model name contains "reasoning")
                                        let reasoning = m["reasoning"].as_bool().unwrap_or_else(|| {
                                            id.to_lowercase().contains("reasoning")
                                        });
                                        
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
                                        
                                        let max_input_tokens = m["maxInputTokens"].as_u64().unwrap_or(4096) as u32;
                                        let max_output_tokens = m["maxOutputTokens"].as_u64().unwrap_or(4096) as u32;
                                        
                                        // Parameter support flags based on model family
                                        let supports_temperature = true; // Most models support this
                                        let supports_top_p = true; // Most models support this
                                        let supports_reasoning_effort = reasoning && matches!(family, ModelFamily::Phi);
                                        
                                        println!("  Model: {} | family: {:?} | toolCalling: {} ({:?}) | vision: {} | reasoning: {} ({:?}) | maxIn: {} | maxOut: {}", 
                                            id, family, tool_calling, tool_format, vision, reasoning, reasoning_format, max_input_tokens, max_output_tokens);
                                        
                                        Some(ModelInfo {
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
                                        })
                                    })
                                    .collect();
                                
                                if self.model_id.is_none() {
                                if let Some(first) = self.available_models.first() {
                                    println!("Selected default model: {}", first);
                                    self.model_id = Some(first.clone());
                                    self.emit_model_selected(first);
                                    }
                                }
                                return !self.available_models.is_empty();
                            }
                         },
                         Err(e) => println!("Failed to parse models response: {}", e),
                     }
                },
                Err(e) => println!("Failed to query models: {}", e),
            }

        } else {
             println!("Warning: Could not detect Foundry service port.");
        }
        false
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
        
        // Run `foundry service restart`
        let output = Command::new("foundry")
            .args(&["service", "restart"])
            .output()
            .await?;
            
        if output.status.success() {
             println!("Foundry service restart command issued successfully.");
        } else {
             let stderr = String::from_utf8_lossy(&output.stderr);
             println!("Foundry service restart command failed: {}", stderr);
             // If restart fails (e.g. not running), try start
             self.ensure_service_running().await?;
        }
        
        // Wait a few seconds for it to spin up
        println!("Waiting for service to be ready...");
        sleep(Duration::from_secs(5)).await;
        
        Ok(())
    }

    async fn detect_port(&self) -> Option<u16> {
        println!("FoundryActor: Detecting port via 'foundry service status'...");
        // Try `foundry service status` to get endpoint
        // Expected output often contains "http://127.0.0.1:PORT"
        let child = Command::new("foundry")
            .args(&["service", "status"])
            .output();
            
        match timeout(Duration::from_secs(5), child).await 
        {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Look for pattern http://127.0.0.1:(\d+)
                // Simple parsing:
                if let Some(start_idx) = stdout.find("http://127.0.0.1:") {
                    let rest = &stdout[start_idx + "http://127.0.0.1:".len()..];
                    let port_str: String = rest.chars().take_while(|c| c.is_digit(10)).collect();
                    if let Ok(port) = port_str.parse::<u16>() {
                        return Some(port);
                    }
                }
                // Fallback: check logs if command doesn't output it directly but we saw it in logs earlier
                None
            }
            Ok(Err(e)) => {
                println!("Failed to run foundry status: {}", e);
                None
            }
            Err(_) => {
                println!("FoundryActor: 'foundry service status' timed out.");
                None
            }
        }
    }

    /// Get model capabilities from `foundry model list`
    /// This is more reliable than the /v1/models API which sometimes incorrectly reports capabilities
    /// Returns a map of model_id -> supports_tools
    async fn get_model_capabilities_from_cli(&self) -> std::collections::HashMap<String, bool> {
        println!("FoundryActor: Getting model capabilities via 'foundry model list'...");
        
        let child = Command::new("foundry")
            .args(&["model", "list"])
            .output();
            
        match timeout(Duration::from_secs(10), child).await {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    println!("FoundryActor: 'foundry model list' failed");
                    return std::collections::HashMap::new();
                }
                
                let stdout = String::from_utf8_lossy(&output.stdout);
                self.parse_model_list_capabilities(&stdout)
            }
            Ok(Err(e)) => {
                println!("FoundryActor: Failed to run 'foundry model list': {}", e);
                std::collections::HashMap::new()
            }
            Err(_) => {
                println!("FoundryActor: 'foundry model list' timed out.");
                std::collections::HashMap::new()
            }
        }
    }

    /// Parse the output of `foundry model list` to extract tool capability
    /// Format:
    /// ```
    /// Alias                          Device     Task           File Size    License      Model ID            
    /// qwen2.5-0.5b                   GPU        chat, tools    0.68 GB      apache-2.0   qwen2.5-0.5b-instruct-generic-gpu:4
    /// phi-4                          GPU        chat           8.37 GB      MIT          Phi-4-generic-gpu:1 
    /// ```
    fn parse_model_list_capabilities(&self, output: &str) -> std::collections::HashMap<String, bool> {
        let mut capabilities = std::collections::HashMap::new();
        
        for line in output.lines() {
            let trimmed = line.trim();
            
            // Skip empty lines, headers, and separator lines
            if trimmed.is_empty() 
                || trimmed.starts_with("Alias")
                || trimmed.starts_with("---")
                || trimmed.starts_with("-")
            {
                continue;
            }
            
            // Check if line contains "tools" in the Task column
            // The Task column comes after Device (GPU/CPU) and before File Size
            let has_tools = trimmed.contains("tools");
            
            // Extract the Model ID (last column, contains a colon like "model-name:version")
            // Split by multiple spaces and find the last token that looks like a model ID
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(model_id) = parts.iter().rev().find(|p| p.contains(':') && !p.contains("://")) {
                let model_id_str = model_id.to_string();
                // Only insert if not already present (first GPU entry takes precedence)
                if !capabilities.contains_key(&model_id_str) {
                    capabilities.insert(model_id_str.clone(), has_tools);
                    if has_tools {
                        println!("FoundryActor: Model {} supports tools (from CLI)", model_id_str);
                    }
                }
            }
        }
        
        println!("FoundryActor: Found {} models with capabilities from CLI", capabilities.len());
        capabilities
    }

    /// Get cached models from `foundry cache ls`
    /// Parses output like:
    /// ```
    /// Models cached on device:
    ///
    ///    Alias                                             Model ID
    ///
    /// 💾 qwen2.5-coder-0.5b                                qwen2.5-coder-0.5b-instruct-generic-gpu:4
    /// 💾 phi-4-mini-reasoning                              Phi-4-mini-reasoning-generic-gpu:3
    /// ```
    async fn get_cached_models(&self) -> Vec<CachedModel> {
        println!("FoundryActor: Getting cached models via 'foundry cache ls'...");
        
        let child = Command::new("foundry")
            .args(&["cache", "ls"])
            .output();
            
        match timeout(Duration::from_secs(10), child).await {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    println!("FoundryActor: 'foundry cache ls' failed");
                    return Vec::new();
                }
                
                let stdout = String::from_utf8_lossy(&output.stdout);
                self.parse_cache_ls_output(&stdout)
            }
            Ok(Err(e)) => {
                println!("FoundryActor: Failed to run 'foundry cache ls': {}", e);
                Vec::new()
            }
            Err(_) => {
                println!("FoundryActor: 'foundry cache ls' timed out.");
                Vec::new()
            }
        }
    }

    /// Parse the output of `foundry cache ls`
    fn parse_cache_ls_output(&self, output: &str) -> Vec<CachedModel> {
        let mut models = Vec::new();
        
        for line in output.lines() {
            let trimmed = line.trim();
            
            // Skip empty lines and header lines
            if trimmed.is_empty() 
                || trimmed.starts_with("Models cached")
                || trimmed.starts_with("Alias")
                || trimmed.starts_with("---") 
            {
                continue;
            }
            
            // Lines with models start with 💾 emoji
            // Format: "💾 alias                                             model_id"
            if trimmed.starts_with("💾") || trimmed.starts_with("💾") {
                // Remove the emoji and parse
                let rest = trimmed.trim_start_matches("💾").trim_start_matches("💾").trim();
                
                // Split on multiple spaces (the columns are separated by many spaces)
                // Find where alias ends and model_id begins by looking for multiple spaces
                if let Some(split_pos) = rest.find("  ") {
                    let alias = rest[..split_pos].trim().to_string();
                    let model_id = rest[split_pos..].trim().to_string();
                    
                    if !alias.is_empty() && !model_id.is_empty() {
                        println!("FoundryActor: Found cached model: {} -> {}", alias, model_id);
                        models.push(CachedModel { alias, model_id });
                    }
                }
            }
        }
        
        println!("FoundryActor: Found {} cached models", models.len());
        models
    }
}
