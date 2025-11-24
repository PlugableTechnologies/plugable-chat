use tokio::sync::mpsc;
use crate::protocol::FoundryMsg;
use serde_json::json;
use std::process::Command;

pub struct FoundryActor {
    rx: mpsc::Receiver<FoundryMsg>,
    port: Option<u16>,
    model_id: Option<String>,
}

impl FoundryActor {
    pub fn new(rx: mpsc::Receiver<FoundryMsg>) -> Self {
        Self { rx, port: None, model_id: None }
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
                    let _ = respond_to.send(mock_embedding);
                }
                FoundryMsg::Chat { history, respond_to } => {
                     // Re-check port if not set (service might have started late)
                     if self.port.is_none() || self.model_id.is_none() {
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
                             "stream": false
                         });
                         
                         println!("Sending request to Foundry at {}", url);
                         println!("Request body: {}", body);
                         
                         match client.post(&url).json(&body).send().await {
                            Ok(resp) => {
                                let status = resp.status();
                                match resp.text().await {
                                    Ok(text) => {
                                        println!("Foundry response ({}): {}", status, text);
                                        match serde_json::from_str::<serde_json::Value>(&text) {
                                            Ok(json) => {
                                                if let Some(content) = json["choices"][0]["message"]["content"].as_str() {
                                                    let _ = respond_to.send(content.to_string());
                                                } else {
                                                    println!("Unexpected JSON response from Foundry: {}", json);
                                                    if let Some(err_msg) = json["error"]["message"].as_str() {
                                                         let _ = respond_to.send(format!("Error: {}", err_msg));
                                                    } else {
                                                         let _ = respond_to.send("Error: Unexpected response format from local model.".to_string());
                                                    }
                                                }
                                            },
                                            Err(e) => {
                                                println!("Failed to parse JSON: {}. Raw text: {}", e, text);
                                                let _ = respond_to.send("Error: Failed to parse JSON response from local model.".to_string());
                                            }
                                        }
                                    },
                                    Err(e) => {
                                        println!("Failed to read response text: {}", e);
                                        let _ = respond_to.send("Error: Failed to read response from local model.".to_string());
                                    }
                                }
                            },
                            Err(e) => {
                                println!("Failed to call Foundry: {}", e);
                                let _ = respond_to.send(format!("Error connecting to local model: {}", e));
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
                                 if let Some(first_model) = data.first() {
                                     if let Some(id) = first_model["id"].as_str() {
                                         println!("Selected model: {}", id);
                                         self.model_id = Some(id.to_string());
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
