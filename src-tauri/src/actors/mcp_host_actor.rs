use tokio::sync::mpsc;
use tokio::process::{Command, Child, ChildStdin};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

use crate::settings::{McpServerConfig, Transport};
use crate::protocol::McpHostMsg;

/// MCP JSON-RPC request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// MCP JSON-RPC response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<Value>,
}

/// MCP Tool definition from tools/list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<Value>,
}

/// Result from tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    #[serde(default)]
    pub content: Vec<McpContent>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

/// Connected MCP server state
struct McpServerConnection {
    config: McpServerConfig,
    process: Child,
    stdin: ChildStdin,
    stdout_lines: Lines<BufReader<tokio::process::ChildStdout>>,
    tools: Vec<McpTool>,
    request_id: u64,
}

impl McpServerConnection {
    fn next_id(&mut self) -> u64 {
        self.request_id += 1;
        self.request_id
    }
    
    /// Send a request and wait for response
    async fn send_request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id();
        
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let request_str = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        println!("McpHostActor: Sending: {}", request_str);

        // Write request
        self.stdin.write_all(format!("{}\n", request_str).as_bytes()).await
            .map_err(|e| format!("Failed to write request: {}", e))?;
        self.stdin.flush().await
            .map_err(|e| format!("Failed to flush request: {}", e))?;

        // Read response with timeout
        let read_result = tokio::time::timeout(
            Duration::from_secs(30),
            self.read_response()
        ).await;
        
        match read_result {
            Ok(Ok(response)) => {
                if let Some(error) = response.error {
                    Err(format!("MCP error {}: {}", error.code, error.message))
                } else {
                    response.result.ok_or_else(|| "No result in response".to_string())
                }
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Request timed out".to_string()),
        }
    }
    
    /// Read a JSON-RPC response from stdout
    async fn read_response(&mut self) -> Result<JsonRpcResponse, String> {
        loop {
            match self.stdout_lines.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    
                    println!("McpHostActor: Received: {}", trimmed);
                    
                    // Try to parse as JSON-RPC response
                    match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                        Ok(response) => return Ok(response),
                        Err(e) => {
                            // Might be a notification or other message, skip
                            println!("McpHostActor: Skipping non-response line: {} ({})", trimmed, e);
                            continue;
                        }
                    }
                }
                Ok(None) => {
                    return Err("Server closed connection (EOF)".to_string());
                }
                Err(e) => {
                    return Err(format!("Failed to read from server: {}", e));
                }
            }
        }
    }
    
    /// Send a notification (no response expected)
    async fn send_notification(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let notification = if let Some(p) = params {
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": p
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "method": method
            })
        };
        
        let notif_str = serde_json::to_string(&notification)
            .map_err(|e| format!("Failed to serialize notification: {}", e))?;
        
        println!("McpHostActor: Sending notification: {}", notif_str);
        
        self.stdin.write_all(format!("{}\n", notif_str).as_bytes()).await
            .map_err(|e| format!("Failed to write notification: {}", e))?;
        self.stdin.flush().await
            .map_err(|e| format!("Failed to flush notification: {}", e))?;
        
        Ok(())
    }
}

/// MCP Host Actor - manages MCP server connections
pub struct McpHostActor {
    rx: mpsc::Receiver<McpHostMsg>,
    connections: Arc<RwLock<HashMap<String, McpServerConnection>>>,
}

impl McpHostActor {
    pub fn new(rx: mpsc::Receiver<McpHostMsg>) -> Self {
        Self {
            rx,
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run(mut self) {
        println!("McpHostActor: Starting...");

        while let Some(msg) = self.rx.recv().await {
            match msg {
                McpHostMsg::ConnectServer { config, respond_to } => {
                    let result = self.connect_server(config).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::DisconnectServer { server_id, respond_to } => {
                    let result = self.disconnect_server(&server_id).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::ListTools { server_id, respond_to } => {
                    let result = self.list_tools(&server_id).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::ExecuteTool { server_id, tool_name, arguments, respond_to } => {
                    let result = self.execute_tool(&server_id, &tool_name, arguments).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::GetAllToolDescriptions { respond_to } => {
                    let result = self.get_all_tool_descriptions().await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::GetServerStatus { server_id, respond_to } => {
                    let status = self.get_server_status(&server_id).await;
                    let _ = respond_to.send(status);
                }
                McpHostMsg::SyncEnabledServers { configs, respond_to } => {
                    let results = self.sync_enabled_servers(configs).await;
                    let _ = respond_to.send(results);
                }
            }
        }

        println!("McpHostActor: Shutting down...");
        // Clean up all connections on shutdown
        let mut connections = self.connections.write().await;
        for (id, mut conn) in connections.drain() {
            println!("McpHostActor: Killing server process: {}", id);
            let _ = conn.process.kill().await;
        }
    }

    async fn connect_server(&self, config: McpServerConfig) -> Result<(), String> {
        println!("McpHostActor: Connecting to server: {} ({})", config.name, config.id);

        // Check if already connected
        {
            let connections = self.connections.read().await;
            if connections.contains_key(&config.id) {
                return Err(format!("Server {} is already connected", config.id));
            }
        }

        match &config.transport {
            Transport::Stdio => {
                self.connect_stdio_server(config).await
            }
            Transport::Sse { url } => {
                // SSE transport - to be implemented later
                Err(format!("SSE transport not yet implemented for URL: {}", url))
            }
        }
    }

    async fn connect_stdio_server(&self, config: McpServerConfig) -> Result<(), String> {
        let command = config.command.clone()
            .ok_or_else(|| "No command specified for stdio transport".to_string())?;

        println!("McpHostActor: Spawning process: {} {:?}", command, config.args);

        let mut cmd = Command::new(&command);
        cmd.args(&config.args);
        
        // Set environment variables
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        
        // Set up stdio pipes
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        
        // Kill process on drop to avoid zombies
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn()
            .map_err(|e| format!("Failed to spawn MCP server process: {}", e))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| "Failed to open stdin".to_string())?;
        let stdout = child.stdout.take()
            .ok_or_else(|| "Failed to open stdout".to_string())?;
        let stderr = child.stderr.take();

        // Spawn a task to log stderr and track when server is ready
        let (stderr_ready_tx, mut stderr_ready_rx) = tokio::sync::mpsc::channel::<()>(1);
        if let Some(stderr) = stderr {
            let server_id_clone = config.id.clone();
            let command_clone = command.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                let mut signaled_ready = false;
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("McpHostActor [{}] stderr: {}", server_id_clone, line);
                    // For cargo run, detect when the actual server starts
                    if !signaled_ready && command_clone == "cargo" && 
                       (line.contains("MCP Test Server starting") || line.contains("Running `")) {
                        let _ = stderr_ready_tx.send(()).await;
                        signaled_ready = true;
                    }
                }
            });
        }

        let server_id = config.id.clone();
        let stdout_lines = BufReader::new(stdout).lines();
        
        // Create connection
        let mut connection = McpServerConnection {
            config,
            process: child,
            stdin,
            stdout_lines,
            tools: Vec::new(),
            request_id: 0,
        };

        // Wait for server to be ready
        // For cargo run, wait for the build to finish; otherwise use a short delay
        let startup_delay = if command == "cargo" {
            println!("McpHostActor: Waiting for cargo to build and start...");
            // Wait for stderr signal or timeout after 60 seconds
            match tokio::time::timeout(Duration::from_secs(60), stderr_ready_rx.recv()).await {
                Ok(Some(())) => {
                    println!("McpHostActor: Server signaled ready via stderr");
                    Duration::from_millis(200)
                }
                _ => {
                    println!("McpHostActor: No ready signal, waiting fixed time");
                    Duration::from_secs(5)
                }
            }
        } else {
            Duration::from_millis(500)
        };
        tokio::time::sleep(startup_delay).await;
        
        // Check if process is still running
        match connection.process.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("Server process exited before initialization with status: {}", status));
            }
            Ok(None) => {
                println!("McpHostActor: Server process is still running, proceeding with initialization");
            }
            Err(e) => {
                println!("McpHostActor: Warning: Could not check process status: {}", e);
            }
        }

        // Send initialize request
        let init_result = connection.send_request("initialize", Some(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "clientInfo": {
                "name": "plugable-chat",
                "version": "0.1.0"
            }
        }))).await;

        match init_result {
            Ok(response) => {
                println!("McpHostActor: Server {} initialized: {:?}", server_id, response);
                
                // Send initialized notification
                if let Err(e) = connection.send_notification("notifications/initialized", None).await {
                    println!("McpHostActor: Warning: Failed to send initialized notification: {}", e);
                }
            }
            Err(e) => {
                println!("McpHostActor: Failed to initialize server {}: {}", server_id, e);
                let _ = connection.process.kill().await;
                return Err(format!("Failed to initialize MCP server: {}", e));
            }
        }

        // Fetch available tools
        match connection.send_request("tools/list", None).await {
            Ok(tools_response) => {
                if let Some(tools_array) = tools_response.get("tools").and_then(|t| t.as_array()) {
                    connection.tools = tools_array.iter()
                        .filter_map(|t| serde_json::from_value(t.clone()).ok())
                        .collect();
                    println!("McpHostActor: Server {} has {} tools", server_id, connection.tools.len());
                    for tool in &connection.tools {
                        println!("McpHostActor:   - {}: {}", tool.name, tool.description.as_deref().unwrap_or("(no description)"));
                    }
                }
            }
            Err(e) => {
                println!("McpHostActor: Warning: Failed to fetch tools for {}: {}", server_id, e);
            }
        }

        // Store connection
        {
            let mut connections = self.connections.write().await;
            connections.insert(server_id.clone(), connection);
        }

        println!("McpHostActor: Server {} connected successfully", server_id);
        Ok(())
    }

    async fn disconnect_server(&self, server_id: &str) -> Result<(), String> {
        let mut connections = self.connections.write().await;
        
        if let Some(mut conn) = connections.remove(server_id) {
            println!("McpHostActor: Disconnecting server: {}", server_id);
            conn.process.kill().await
                .map_err(|e| format!("Failed to kill process: {}", e))?;
            Ok(())
        } else {
            Err(format!("Server {} not connected", server_id))
        }
    }

    async fn list_tools(&self, server_id: &str) -> Result<Vec<McpTool>, String> {
        let connections = self.connections.read().await;
        
        if let Some(conn) = connections.get(server_id) {
            Ok(conn.tools.clone())
        } else {
            Err(format!("Server {} not connected", server_id))
        }
    }

    async fn execute_tool(&self, server_id: &str, tool_name: &str, arguments: Value) -> Result<McpToolResult, String> {
        println!("McpHostActor: Executing tool {} on server {}", tool_name, server_id);
        
        let mut connections = self.connections.write().await;
        let connection = connections.get_mut(server_id)
            .ok_or_else(|| format!("Server {} not connected", server_id))?;
        
        let result = connection.send_request("tools/call", Some(json!({
            "name": tool_name,
            "arguments": arguments
        }))).await?;

        // Parse the result
        let tool_result: McpToolResult = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse tool result: {}", e))?;
        
        Ok(tool_result)
    }

    async fn get_all_tool_descriptions(&self) -> Vec<(String, Vec<McpTool>)> {
        let connections = self.connections.read().await;
        
        let result: Vec<_> = connections.iter()
            .filter(|(_, conn)| conn.config.enabled)
            .map(|(id, conn)| (id.clone(), conn.tools.clone()))
            .collect();
        
        println!("McpHostActor: get_all_tool_descriptions returning {} servers", result.len());
        for (id, tools) in &result {
            println!("McpHostActor:   {} has {} tools", id, tools.len());
        }
        
        result
    }

    async fn get_server_status(&self, server_id: &str) -> bool {
        let connections = self.connections.read().await;
        connections.contains_key(server_id)
    }
    
    /// Sync enabled servers - connect enabled ones that aren't connected, disconnect disabled ones
    async fn sync_enabled_servers(&self, configs: Vec<McpServerConfig>) -> Vec<(String, Result<(), String>)> {
        let mut results = Vec::new();
        
        // Get currently connected server IDs
        let connected_ids: Vec<String> = {
            let connections = self.connections.read().await;
            connections.keys().cloned().collect()
        };
        
        println!("McpHostActor: Syncing {} configs, {} currently connected", configs.len(), connected_ids.len());
        
        // Connect enabled servers that aren't connected
        for config in &configs {
            if config.enabled && !connected_ids.contains(&config.id) {
                println!("McpHostActor: Auto-connecting enabled server: {} ({})", config.name, config.id);
                let result = self.connect_server(config.clone()).await;
                results.push((config.id.clone(), result));
            }
        }
        
        // Disconnect servers that are no longer enabled
        let enabled_ids: Vec<&str> = configs.iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.as_str())
            .collect();
        
        for connected_id in &connected_ids {
            if !enabled_ids.contains(&connected_id.as_str()) {
                println!("McpHostActor: Disconnecting disabled server: {}", connected_id);
                let result = self.disconnect_server(connected_id).await;
                results.push((connected_id.clone(), result));
            }
        }
        
        // Log summary
        let connected_count = {
            let connections = self.connections.read().await;
            connections.len()
        };
        println!("McpHostActor: Sync complete - {} servers now connected", connected_count);
        
        results
    }
}
