use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use crate::process_utils::HideConsoleWindow;
use crate::protocol::McpHostMsg;
use crate::settings::{McpServerConfig, Transport};

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
    #[serde(rename = "inputSchema", alias = "input_schema", default)]
    pub input_schema: Option<Value>,
    /// Optional examples to help small models use the tool correctly
    #[serde(default, rename = "inputExamples", alias = "input_examples")]
    pub input_examples: Option<Vec<Value>>,
    /// Allowed callers for programmatic tool use (e.g., ["python_execution_20251206"])
    #[serde(default, rename = "allowedCallers", alias = "allowed_callers")]
    pub allowed_callers: Option<Vec<String>>,
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
        self.stdin
            .write_all(format!("{}\n", request_str).as_bytes())
            .await
            .map_err(|e| format!("Failed to write request: {}", e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush request: {}", e))?;

        // Read response with timeout, ensuring we match on the request id
        let read_result =
            tokio::time::timeout(Duration::from_secs(30), self.read_response(id)).await;

        match read_result {
            Ok(Ok(response)) => {
                if let Some(error) = response.error {
                    Err(format!("MCP error {}: {}", error.code, error.message))
                } else {
                    response
                        .result
                        .ok_or_else(|| "No result in response".to_string())
                }
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("Request timed out waiting for id {}", id)),
        }
    }

    /// Read a JSON-RPC response from stdout
    async fn read_response(&mut self, expected_id: u64) -> Result<JsonRpcResponse, String> {
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
                        Ok(response) => {
                            match response.id {
                                Some(id) if id == expected_id => return Ok(response),
                                Some(other_id) => {
                                    println!(
                                        "McpHostActor: Skipping response with mismatched id {} (expected {})",
                                        other_id, expected_id
                                    );
                                    continue;
                                }
                                None => {
                                    println!(
                                        "McpHostActor: Skipping response with null id (expected {})",
                                        expected_id
                                    );
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            // Might be a notification or other message, skip
                            println!(
                                "McpHostActor: Skipping non-response line: {} ({})",
                                trimmed, e
                            );
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
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), String> {
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

        self.stdin
            .write_all(format!("{}\n", notif_str).as_bytes())
            .await
            .map_err(|e| format!("Failed to write notification: {}", e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush notification: {}", e))?;

        Ok(())
    }
}

/// MCP Host Actor - manages MCP server connections
pub struct McpToolRouterActor {
    mcp_tool_msg_rx: mpsc::Receiver<McpHostMsg>,
    connections: Arc<RwLock<HashMap<String, McpServerConnection>>>,
}

impl McpToolRouterActor {
    pub fn new(mcp_tool_msg_rx: mpsc::Receiver<McpHostMsg>) -> Self {
        Self {
            mcp_tool_msg_rx,
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run(mut self) {

        while let Some(msg) = self.mcp_tool_msg_rx.recv().await {
            match msg {
                McpHostMsg::ConnectServer { config, respond_to } => {
                    let result = self.connect_server(config).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::DisconnectServer {
                    server_id,
                    respond_to,
                } => {
                    let result = self.disconnect_server(&server_id).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::ListTools {
                    server_id,
                    respond_to,
                } => {
                    let result = self.list_tools(&server_id).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::ExecuteTool {
                    server_id,
                    tool_name,
                    arguments,
                    respond_to,
                } => {
                    let result = self.execute_tool(&server_id, &tool_name, arguments).await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::GetAllToolDescriptions { respond_to } => {
                    let result = self.get_all_tool_descriptions().await;
                    let _ = respond_to.send(result);
                }
                McpHostMsg::GetServerStatus {
                    server_id,
                    respond_to,
                } => {
                    let status = self.get_server_status(&server_id).await;
                    let _ = respond_to.send(status);
                }
                McpHostMsg::SyncEnabledServers {
                    configs,
                    respond_to,
                } => {
                    let results = self.sync_enabled_servers(configs).await;
                    let _ = respond_to.send(results);
                }
                McpHostMsg::TestServerConfig { config, respond_to } => {
                    let result = self.test_server_config(config).await;
                    let _ = respond_to.send(result);
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
        println!(
            "McpHostActor: Connecting to server: {} ({})",
            config.name, config.id
        );

        // Check if already connected
        {
            let connections = self.connections.read().await;
            if connections.contains_key(&config.id) {
                return Err(format!("Server {} is already connected", config.id));
            }
        }

        match &config.transport {
            Transport::Stdio => self.connect_stdio_server(config).await,
            Transport::Sse { url } => {
                // SSE transport - to be implemented later
                Err(format!(
                    "SSE transport not yet implemented for URL: {}",
                    url
                ))
            }
        }
    }

    async fn connect_stdio_server(&self, config: McpServerConfig) -> Result<(), String> {
        let command = config
            .command
            .clone()
            .ok_or_else(|| "No command specified for stdio transport".to_string())?;

        println!(
            "McpHostActor: Spawning process: {} {:?}",
            command, config.args
        );

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

        // Hide console window on Windows to avoid distracting popups
        cmd.hide_console_window();

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server process '{}': {}", command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open stdout".to_string())?;
        let stderr = child.stderr.take();

        // Capture stderr in a shared buffer for error reporting AND track readiness
        let stderr_buffer = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let stderr_buffer_clone = stderr_buffer.clone();
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

                    // Store in buffer for error reporting (keep last 50 lines)
                    {
                        let mut buffer = stderr_buffer_clone.lock().await;
                        if buffer.len() >= 50 {
                            buffer.remove(0);
                        }
                        buffer.push(line.clone());
                    }

                    // For cargo run, detect when the actual server starts
                    if !signaled_ready
                        && command_clone == "cargo"
                        && (line.contains("MCP Test Server starting") || line.contains("Running `"))
                    {
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

        // Helper to format error with captured output
        let format_error_with_output = |base_error: String, stderr_buf: &[String]| -> String {
            let mut error = base_error;
            if !stderr_buf.is_empty() {
                error.push_str("\n\n--- stderr output ---\n");
                error.push_str(&stderr_buf.join("\n"));
            }
            error
        };

        // Check if process is still running
        match connection.process.try_wait() {
            Ok(Some(status)) => {
                // Wait a moment for stderr to be collected
                tokio::time::sleep(Duration::from_millis(200)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!(
                        "Server process exited before initialization with status: {}",
                        status
                    ),
                    &stderr_output,
                ));
            }
            Ok(None) => {
                println!(
                    "McpHostActor: Server process is still running, proceeding with initialization"
                );
            }
            Err(e) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!("Could not check process status: {}", e),
                    &stderr_output,
                ));
            }
        }

        // Send initialize request
        let init_result = connection
            .send_request(
                "initialize",
                Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "clientInfo": {
                        "name": "plugable-chat",
                        "version": "0.1.0"
                    }
                })),
            )
            .await;

        match init_result {
            Ok(response) => {
                println!(
                    "McpHostActor: Server {} initialized: {:?}",
                    server_id, response
                );

                // Send initialized notification
                if let Err(e) = connection
                    .send_notification("notifications/initialized", None)
                    .await
                {
                    println!(
                        "McpHostActor: Warning: Failed to send initialized notification: {}",
                        e
                    );
                }
            }
            Err(e) => {
                println!(
                    "McpHostActor: Failed to initialize server {}: {}",
                    server_id, e
                );
                let _ = connection.process.kill().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!("Failed to initialize MCP server: {}", e),
                    &stderr_output,
                ));
            }
        }

        // Fetch available tools
        match connection.send_request("tools/list", None).await {
            Ok(tools_response) => {
                if let Some(tools_array) = tools_response.get("tools").and_then(|t| t.as_array()) {
                    connection.tools = tools_array
                        .iter()
                        .filter_map(|t| serde_json::from_value(t.clone()).ok())
                        .collect();
                    let mode = if connection.config.defer_tools {
                        "DEFERRED"
                    } else {
                        "ACTIVE"
                    };
                    println!(
                        "McpHostActor: Server {} has {} tools [{}]",
                        server_id,
                        connection.tools.len(),
                        mode
                    );
                    for tool in &connection.tools {
                        println!(
                            "McpHostActor:   - {} [{}]: {}",
                            tool.name,
                            mode,
                            tool.description.as_deref().unwrap_or("(no description)")
                        );
                    }
                }
            }
            Err(e) => {
                println!(
                    "McpHostActor: Warning: Failed to fetch tools for {}: {}",
                    server_id, e
                );
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
            conn.process
                .kill()
                .await
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

    async fn execute_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<McpToolResult, String> {
        // Log the input
        println!("\n╔══════════════════════════════════════════════════════════════");
        println!("║ MCP TOOL CALL INPUT");
        println!("╠══════════════════════════════════════════════════════════════");
        println!("║ Server:    {}", server_id);
        println!("║ Tool:      {}", tool_name);
        println!(
            "║ Arguments: {}",
            serde_json::to_string_pretty(&arguments).unwrap_or_else(|_| arguments.to_string())
        );
        println!("╚══════════════════════════════════════════════════════════════\n");

        let mut connections = self.connections.write().await;
        let connection = connections.get_mut(server_id).ok_or_else(|| {
            println!("║ ERROR: Server {} not connected", server_id);
            format!("Server {} not connected", server_id)
        })?;

        let result = connection
            .send_request(
                "tools/call",
                Some(json!({
                    "name": tool_name,
                    "arguments": arguments
                })),
            )
            .await;

        match result {
            Ok(raw_result) => {
                // Parse the result
                let tool_result: McpToolResult = serde_json::from_value(raw_result.clone())
                    .map_err(|e| format!("Failed to parse tool result: {}", e))?;

                // Log the output
                println!("\n╔══════════════════════════════════════════════════════════════");
                println!("║ MCP TOOL CALL OUTPUT");
                println!("╠══════════════════════════════════════════════════════════════");
                println!("║ Server:   {}", server_id);
                println!("║ Tool:     {}", tool_name);
                println!("║ Is Error: {}", tool_result.is_error);
                println!("║ Content:");
                for content in &tool_result.content {
                    if let Some(text) = &content.text {
                        // Indent multi-line output
                        for line in text.lines() {
                            println!("║   {}", line);
                        }
                    }
                    if let Some(data) = &content.data {
                        let preview: String = data.chars().take(200).collect();
                        println!(
                            "║   [Binary data: {} bytes, preview: {}...]",
                            data.len(),
                            preview
                        );
                    }
                }
                println!("╚══════════════════════════════════════════════════════════════\n");

                Ok(tool_result)
            }
            Err(e) => {
                // Log the error
                println!("\n╔══════════════════════════════════════════════════════════════");
                println!("║ MCP TOOL CALL ERROR");
                println!("╠══════════════════════════════════════════════════════════════");
                println!("║ Server: {}", server_id);
                println!("║ Tool:   {}", tool_name);
                println!("║ Error:  {}", e);
                println!("╚══════════════════════════════════════════════════════════════\n");

                Err(e)
            }
        }
    }

    async fn get_all_tool_descriptions(&self) -> Vec<(String, Vec<McpTool>)> {
        let connections = self.connections.read().await;

        let result: Vec<_> = connections
            .iter()
            .filter(|(_, conn)| conn.config.enabled)
            .map(|(id, conn)| (id.clone(), conn.tools.clone()))
            .collect();

        println!(
            "McpHostActor: get_all_tool_descriptions returning {} servers",
            result.len()
        );
        for (id, tools) in &result {
            if let Some(conn) = connections.get(id) {
                let mode = if conn.config.defer_tools {
                    "DEFERRED"
                } else {
                    "ACTIVE"
                };
                println!(
                    "McpHostActor:   {} has {} tools [{}]",
                    id,
                    tools.len(),
                    mode
                );
            } else {
                println!("McpHostActor:   {} has {} tools", id, tools.len());
            }
        }

        result
    }

    async fn get_server_status(&self, server_id: &str) -> bool {
        let connections = self.connections.read().await;
        connections.contains_key(server_id)
    }

    /// Sync enabled servers - connect enabled ones that aren't connected, disconnect disabled ones
    async fn sync_enabled_servers(
        &self,
        configs: Vec<McpServerConfig>,
    ) -> Vec<(String, Result<(), String>)> {
        let mut results = Vec::new();

        println!(
            "McpHostActor: SyncEnabledServers called with {} configs",
            configs.len()
        );
        for cfg in &configs {
            println!(
                "McpHostActor:   Config: '{}' (id={}, enabled={}, transport={:?}, command={:?})",
                cfg.name,
                cfg.id,
                cfg.enabled,
                cfg.transport,
                cfg.command
            );
        }

        // Get currently connected server IDs
        let connected_ids: Vec<String> = {
            let connections = self.connections.read().await;
            connections.keys().cloned().collect()
        };

        println!(
            "McpHostActor: Currently connected servers: {:?}",
            connected_ids
        );

        // Connect enabled servers that aren't connected
        for config in &configs {
            if config.enabled && !connected_ids.contains(&config.id) {
                println!(
                    "McpHostActor: ➡️ Connecting enabled server: '{}' (id={}, command={:?}, args={:?})",
                    config.name, config.id, config.command, config.args
                );
                let result = self.connect_server(config.clone()).await;
                match &result {
                    Ok(()) => println!(
                        "McpHostActor: ✓ Successfully connected: '{}' ({})",
                        config.name, config.id
                    ),
                    Err(e) => println!(
                        "McpHostActor: ❌ Failed to connect '{}' ({}): {}",
                        config.name, config.id, e
                    ),
                }
                results.push((config.id.clone(), result));
            } else if config.enabled {
                println!(
                    "McpHostActor: ⏭️ Skipping already connected server: '{}' ({})",
                    config.name, config.id
                );
            }
        }

        // Disconnect servers that are explicitly provided in the list but disabled
        let disabled_ids: Vec<&str> = configs
            .iter()
            .filter(|c| !c.enabled)
            .map(|c| c.id.as_str())
            .collect();

        for connected_id in &connected_ids {
            if disabled_ids.contains(&connected_id.as_str()) {
                println!(
                    "McpHostActor: ⏹️ Disconnecting disabled server: {}",
                    connected_id
                );
                let result = self.disconnect_server(connected_id).await;
                results.push((connected_id.clone(), result));
            }
        }

        // Log summary
        let connected_count = {
            let connections = self.connections.read().await;
            connections.len()
        };
        println!(
            "McpHostActor: Sync complete - {} operations performed, {} servers now connected",
            results.len(),
            connected_count
        );
        for (id, res) in &results {
            match res {
                Ok(()) => println!("McpHostActor:   ✓ {} - OK", id),
                Err(e) => println!("McpHostActor:   ❌ {} - {}", id, e),
            }
        }

        results
    }

    /// Test a server config by connecting, getting tools, then cleaning up
    /// This does NOT store the connection - it's purely for testing
    async fn test_server_config(&self, config: McpServerConfig) -> Result<Vec<McpTool>, String> {
        println!(
            "McpHostActor: Testing server config: {} ({})",
            config.name, config.id
        );

        match &config.transport {
            Transport::Stdio => self.test_stdio_server_config(config).await,
            Transport::Sse { url } => Err(format!(
                "SSE transport not yet implemented for URL: {}",
                url
            )),
        }
    }

    /// Test a stdio server config - spawns process, initializes, gets tools, then cleans up
    /// Captures stdout/stderr and includes them in error messages for debugging
    async fn test_stdio_server_config(
        &self,
        config: McpServerConfig,
    ) -> Result<Vec<McpTool>, String> {
        let command = config
            .command
            .clone()
            .ok_or_else(|| "No command specified for stdio transport".to_string())?;

        println!(
            "McpHostActor: Test - Spawning process: {} {:?}",
            command, config.args
        );

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

        // Kill process on drop
        cmd.kill_on_drop(true);

        // Hide console window on Windows to avoid distracting popups
        cmd.hide_console_window();

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server process '{}': {}", command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open stdout".to_string())?;
        let stderr = child.stderr.take();

        // Capture stderr in a shared buffer for error reporting
        let stderr_buffer = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let stderr_buffer_clone = stderr_buffer.clone();
        let server_id_clone = config.id.clone();

        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("McpHostActor [TEST {}] stderr: {}", server_id_clone, line);
                    let mut buffer = stderr_buffer_clone.lock().await;
                    // Keep last 50 lines to avoid memory issues
                    if buffer.len() >= 50 {
                        buffer.remove(0);
                    }
                    buffer.push(line);
                }
            });
        }

        let stdout_lines = BufReader::new(stdout).lines();

        // Create temporary connection
        let mut connection = McpServerConnection {
            config: config.clone(),
            process: child,
            stdin,
            stdout_lines,
            tools: Vec::new(),
            request_id: 0,
        };

        // Wait for server to start
        let startup_delay = if command == "cargo" {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(500)
        };
        tokio::time::sleep(startup_delay).await;

        // Helper to format error with captured output
        let format_error_with_output = |base_error: String, stderr_buf: &[String]| -> String {
            let mut error = base_error;
            if !stderr_buf.is_empty() {
                error.push_str("\n\n--- stderr output ---\n");
                error.push_str(&stderr_buf.join("\n"));
            }
            error
        };

        // Check if process is still running
        match connection.process.try_wait() {
            Ok(Some(status)) => {
                // Wait a moment for stderr to be collected
                tokio::time::sleep(Duration::from_millis(200)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!(
                        "Server process exited before initialization with status: {}",
                        status
                    ),
                    &stderr_output,
                ));
            }
            Ok(None) => {
                // Process still running, good
            }
            Err(e) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!("Could not check process status: {}", e),
                    &stderr_output,
                ));
            }
        }

        // Send initialize request
        let init_result = connection
            .send_request(
                "initialize",
                Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "clientInfo": {
                        "name": "plugable-chat-test",
                        "version": "0.1.0"
                    }
                })),
            )
            .await;

        match init_result {
            Ok(_response) => {
                // Send initialized notification
                let _ = connection
                    .send_notification("notifications/initialized", None)
                    .await;
            }
            Err(e) => {
                let _ = connection.process.kill().await;
                // Wait a moment to collect any final stderr output
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!("Failed to initialize MCP server: {}", e),
                    &stderr_output,
                ));
            }
        }

        // Fetch available tools
        let tools: Vec<McpTool> = match connection.send_request("tools/list", None).await {
            Ok(tools_response) => {
                if let Some(tools_array) = tools_response.get("tools").and_then(|t| t.as_array()) {
                    tools_array
                        .iter()
                        .filter_map(|t| serde_json::from_value::<McpTool>(t.clone()).ok())
                        .collect()
                } else {
                    Vec::new()
                }
            }
            Err(e) => {
                let _ = connection.process.kill().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!("Failed to fetch tools: {}", e),
                    &stderr_output,
                ));
            }
        };

        // Clean up - kill the process
        let _ = connection.process.kill().await;

        let mode = if config.defer_tools {
            "DEFERRED"
        } else {
            "ACTIVE"
        };
        println!(
            "McpHostActor: Test complete - found {} tools [{}]",
            tools.len(),
            mode
        );
        for tool in &tools {
            println!(
                "McpHostActor: Test -   {} [{}]: {}",
                tool.name,
                mode,
                tool.description.as_deref().unwrap_or("(no description)")
            );
        }

        Ok(tools)
    }
}
