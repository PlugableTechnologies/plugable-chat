use tokio::sync::mpsc;
use crate::protocol::FoundryMsg;
use serde_json::json;
use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;

pub struct FoundryActor {
    rx: mpsc::Receiver<FoundryMsg>,
    port: Option<u16>,
    model_id: Option<String>,
    available_models: Vec<String>,
}

impl FoundryActor {
    pub fn new(rx: mpsc::Receiver<FoundryMsg>) -> Self {
        Self { rx, port: None, model_id: None, available_models: Vec::new() }
    }

    pub async fn run(mut self) {
        println!("Initializing Foundry Local Manager via CLI...");
        
        // Try to start the service or ensure it's running
        if let Err(e) = self.ensure_service_running() {
            println!("Warning: Failed to ensure Foundry service is running: {}", e);
        }

        // Try to get the port and model
        self.update_connection_info().await;

        let client = reqwest::Client::new();

        while let Some(msg) = self.rx.recv().await {
            match msg {
                FoundryMsg::GetEmbedding { text: _, respond_to } => {
                    // Mock embedding generation for now
                    let mock_embedding = vec![0.1; 384];
                    println!("FoundryActor: Mocking embedding generation for query.");
                    let _ = respond_to.send(mock_embedding);
                }
                FoundryMsg::GetModels { respond_to } => {
                    if self.port.is_none() || self.available_models.is_empty() {
                        self.update_connection_info().await;
                    }
                    let _ = respond_to.send(self.available_models.clone());
                }
                FoundryMsg::SetModel { model_id, respond_to } => {
                    self.model_id = Some(model_id);
                    let _ = respond_to.send(true);
                }
                FoundryMsg::Chat { history, respond_to } => {
                     // Check if we need to restart/reconnect
                     if self.port.is_none() || self.available_models.is_empty() {
                         println!("FoundryActor: No models found or port missing. Attempting to restart service...");
                         let _ = respond_to.send("Restarting local model service...".to_string()).await;
                         
                         // Restart service
                         if let Err(e) = self.restart_service().await {
                             println!("FoundryActor: Failed to restart service: {}", e);
                             let _ = respond_to.send(format!("Error: Failed to restart service: {}", e)).await;
                             continue;
                         }
                         
                         // Update info
                         self.update_connection_info().await;
                     }

                     if let Some(port) = self.port {
                         // Use detected model or default to "Phi-4-generic-gpu:1" if detection failed but port is open
                         let model = self.model_id.clone().unwrap_or_else(|| "Phi-4-generic-gpu:1".to_string());
                         
                         let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
                         
                         // Convert history to messages
                         let body = json!({
                             "model": model, 
                             "messages": history,
                             "stream": true
                         });
                         
                         println!("Sending streaming request to Foundry at {}", url);
                         
                         // We need to clone client/url/body or move them. 
                         // Since we are in a loop, we clone the client (cheap).
                         let client_clone = client.clone();
                         let respond_to_clone = respond_to.clone(); // Mpsc sender is clonable
                         
                         // Spawn a task to handle the streaming response so we don't block the actor loop?
                         // Actually, blocking the actor loop per user request is fine for a single-user desktop app 
                         // to ensure sequential processing, but streaming might take time.
                         // Let's do it inline for now to simplify.
                         
                         match client_clone.post(&url).json(&body).send().await {
                            Ok(mut resp) => {
                                let status = resp.status();
                                if !status.is_success() {
                                     let text = resp.text().await.unwrap_or_default();
                                     println!("Foundry error ({}): {}", status, text);
                                     let _ = respond_to_clone.send(format!("Error: {}", text)).await;
                                } else {
                                    let mut buffer = String::new();
                                    println!("Foundry stream started.");
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
                                                        // Check for content delta
                                                        if let Some(content) = json["choices"][0]["delta"]["content"].as_str() {
                                                            if !content.is_empty() {
                                                                // println!("Token: {:?}", content); // Uncomment for verbose logging
                                                                let _ = respond_to_clone.send(content.to_string()).await;
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
                                let _ = respond_to_clone.send(format!("Error connecting to local model: {}", e)).await;
                            }
                         }
                     } else {
                         println!("Foundry endpoint not available (port not found).");
                         let _ = respond_to.send("The local model service is not available. Please check if Foundry is installed and running.".to_string()).await;
                     }
                }
            }
        }
    }

    async fn update_connection_info(&mut self) {
        self.port = self.detect_port();
        if let Some(p) = self.port {
            println!("Foundry service detected on port {}", p);
            
            // Fetch available models
            let client = reqwest::Client::new();
            let models_url = format!("http://127.0.0.1:{}/v1/models", p);
            match client.get(&models_url).send().await {
                Ok(resp) => {
                     match resp.json::<serde_json::Value>().await {
                         Ok(json) => {
                             println!("Available models: {}", json);
                            if let Some(data) = json["data"].as_array() {
                                self.available_models = data.iter()
                                    .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                                    .collect();
                                
                                if self.model_id.is_none() {
                                    if let Some(first) = self.available_models.first() {
                                        println!("Selected default model: {}", first);
                                        self.model_id = Some(first.clone());
                                    }
                                }
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
    }

    fn ensure_service_running(&self) -> std::io::Result<()> {
        // Try to start service via CLI: `foundry service start`
        // We won't wait for it to fully initialize here, just trigger it
        let output = Command::new("foundry")
            .args(&["service", "start"])
            .output()?;
            
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
            .output()?;
            
        if output.status.success() {
             println!("Foundry service restart command issued successfully.");
        } else {
             let stderr = String::from_utf8_lossy(&output.stderr);
             println!("Foundry service restart command failed: {}", stderr);
             // If restart fails (e.g. not running), try start
             self.ensure_service_running()?;
        }
        
        // Wait a few seconds for it to spin up
        println!("Waiting for service to be ready...");
        sleep(Duration::from_secs(5)).await;
        
        Ok(())
    }

    fn detect_port(&self) -> Option<u16> {
        // Try `foundry service status` to get endpoint
        // Expected output often contains "http://127.0.0.1:PORT"
        match Command::new("foundry")
            .args(&["service", "status"])
            .output() 
        {
            Ok(output) => {
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
            Err(e) => {
                println!("Failed to run foundry status: {}", e);
                None
            }
        }
    }
}
