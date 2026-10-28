use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
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
    /// Machine-readable result some servers send next to (or instead of) the text content.
    #[serde(rename = "structuredContent", default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
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

/// Seconds to wait for one request when a server does not say otherwise.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// The per-request timeout for a server: its own `request_timeout_secs`, else the default.
pub fn request_timeout_for(config: &McpServerConfig) -> Duration {
    Duration::from_secs(config.request_timeout_secs.filter(|s| *s > 0).unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS))
}

/// A line from the server that is a request or notification (it has a `method`), not a response.
#[derive(Debug, PartialEq)]
pub struct ServerOriginatedMessage {
    pub method: String,
    /// What to send back, if the message needs an answer we can give (a `ping`).
    pub reply: Option<String>,
}

pub fn server_originated_request(line: &str) -> Option<ServerOriginatedMessage> {
    let value: Value = serde_json::from_str(line).ok()?;
    let method = value.get("method")?.as_str()?.to_string();
    let reply = match (method.as_str(), value.get("id")) {
        ("ping", Some(id)) => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string()),
        _ => None,
    };
    Some(ServerOriginatedMessage { method, reply })
}

/// Why a stdio server could not start. A missing program names what usually provides it, because
/// "program not found" alone does not tell a tester what to install (rc18: `cargo`).
pub fn describe_spawn_error(command: &str, error: &std::io::Error) -> String {
    if error.kind() != std::io::ErrorKind::NotFound {
        return format!("Failed to spawn MCP server process '{command}': {error}");
    }
    let program = std::path::Path::new(command)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    let install = match program.as_str() {
        "cargo" | "rustc" => "Install the Rust toolchain (https://rustup.rs)",
        "npx" | "npm" | "node" => "Install Node.js (https://nodejs.org)",
        "uvx" | "uv" => "Install uv (https://docs.astral.sh/uv)",
        "python" | "python3" | "pip" | "pipx" => "Install Python (https://python.org)",
        _ => "Install it and make sure it is on your PATH",
    };
    format!("Could not start '{command}': program not found. {install}, or change the command in Settings > Tools.")
}

/// A spawned stdio server and its pipes.
struct StdioLink {
    process: Child,
    stdin: ChildStdin,
    stdout_lines: Lines<BufReader<ChildStdout>>,
}

impl StdioLink {
    async fn send_line(&mut self, line: &str, what: &str) -> Result<(), String> {
        self.stdin
            .write_all(format!("{}\n", line).as_bytes())
            .await
            .map_err(|e| format!("Failed to write {}: {}", what, e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush {}: {}", what, e))
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

                    // A message with a method is the server talking to us (a notification, or a
                    // request such as ping), never the response we are waiting for. Answer a ping
                    // so a server that checks on us does not give up; ignore the rest.
                    if let Some(request) = server_originated_request(trimmed) {
                        if let Some(reply) = request.reply {
                            let _ = self.stdin.write_all(format!("{}\n", reply).as_bytes()).await;
                            let _ = self.stdin.flush().await;
                        }
                        println!("McpHostActor: Skipping server message '{}'", request.method);
                        continue;
                    }

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
}

/// An MCP server reached over Streamable HTTP: every message is a POST to one URL. There is no
/// connection to keep open; the optional session id and protocol version the server hands out on
/// `initialize` are sent back on later requests.
struct HttpLink {
    client: reqwest::Client,
    url: String,
    session_id: Option<String>,
    protocol_version: Option<String>,
}

/// The protocol version a Streamable HTTP client asks for. The stdio path keeps the older one it was built against.
const HTTP_PROTOCOL_VERSION: &str = "2025-03-26";

impl HttpLink {
    fn new(url: &str) -> Result<Self, String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("No URL set for this HTTP server".to_string());
        }
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("'{}' is not a valid URL: {}", url, e))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(format!("The URL must start with http:// or https://, got '{}'", url));
        }
        // No Origin header is ever sent (a desktop app is not a web page), and there is no overall
        // reqwest timeout: the per-server request timeout wraps each call, because a tool may run for minutes.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent("plugable-chat")
            .build()
            .map_err(|e| format!("Could not set up the HTTP client: {}", e))?;
        Ok(Self { client, url: url.to_string(), session_id: None, protocol_version: None })
    }

    fn request_builder(&self, body: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(session) = &self.session_id {
            req = req.header("Mcp-Session-Id", session);
        }
        if let Some(version) = &self.protocol_version {
            req = req.header("MCP-Protocol-Version", version);
        }
        req
    }

    /// POST one request and return the response with the matching id.
    async fn post_request(&mut self, body: &str, expected_id: u64) -> Result<JsonRpcResponse, String> {
        let response = self
            .request_builder(body)
            .send()
            .await
            .map_err(|e| format!("Could not reach {}: {}", self.url, e))?;
        if let Some(session) = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session_id = Some(session.to_string());
        }
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let text = response
            .text()
            .await
            .map_err(|e| format!("Failed to read the reply from {}: {}", self.url, e))?;
        println!("McpHostActor: Received (HTTP {}): {}", status.as_u16(), text.chars().take(2000).collect::<String>());
        if !status.is_success() {
            let detail: String = text.trim().chars().take(300).collect();
            return Err(format!("HTTP {} from {}{}", status.as_u16(), self.url, if detail.is_empty() { String::new() } else { format!(": {}", detail) }));
        }
        parse_http_reply(&content_type, &text, expected_id)
    }

    /// POST a notification; the server answers 202 (or any 2xx) with no body to use.
    async fn post_notification(&mut self, body: &str) -> Result<(), String> {
        let response = self
            .request_builder(body)
            .send()
            .await
            .map_err(|e| format!("Could not reach {}: {}", self.url, e))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {} from {}", response.status().as_u16(), self.url))
        }
    }

    /// End the session if the server gave us one. Best effort: the server may not support it.
    async fn close(&mut self) {
        if let Some(session) = self.session_id.take() {
            let delete = self.client.delete(&self.url).header("Mcp-Session-Id", session).send();
            let _ = tokio::time::timeout(Duration::from_secs(3), delete).await;
        }
    }
}

/// Find the response to `expected_id` in an HTTP reply. A reply is either one JSON-RPC message
/// (`application/json`) or a server-sent-event stream that carries it (`text/event-stream`); messages
/// that are requests or notifications from the server are skipped, as on stdio.
pub fn parse_http_reply(content_type: &str, body: &str, expected_id: u64) -> Result<JsonRpcResponse, String> {
    let mut candidates: Vec<String> = Vec::new();
    if content_type.contains("text/event-stream") {
        let mut data = String::new();
        for line in body.lines().chain(std::iter::once("")) {
            if line.is_empty() {
                if !data.is_empty() {
                    candidates.push(std::mem::take(&mut data));
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
    } else {
        candidates.push(body.to_string());
    }
    for candidate in candidates {
        let value: Value = match serde_json::from_str(candidate.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // A JSON array is a batch of messages.
        let messages = match value {
            Value::Array(items) => items,
            other => vec![other],
        };
        for message in messages {
            if message.get("method").is_some() {
                continue; // a request or notification from the server, not our answer
            }
            if let Ok(response) = serde_json::from_value::<JsonRpcResponse>(message) {
                if response.id == Some(expected_id) {
                    return Ok(response);
                }
            }
        }
    }
    Err(format!("The server's reply held no response to request {}", expected_id))
}

enum Link {
    Stdio(StdioLink),
    Http(HttpLink),
}

/// Connected MCP server state
struct McpServerConnection {
    config: McpServerConfig,
    link: Link,
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

        // The whole exchange (write, then read the answer with the matching id) shares one limit.
        let request_timeout = request_timeout_for(&self.config);
        let read_result = match &mut self.link {
            Link::Stdio(link) => {
                tokio::time::timeout(request_timeout, async {
                    link.send_line(&request_str, "request").await?;
                    link.read_response(id).await
                })
                .await
            }
            Link::Http(link) => tokio::time::timeout(request_timeout, link.post_request(&request_str, id)).await,
        };

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
            Err(_) => Err(format!(
                "Request timed out after {}s waiting for id {} (raise request_timeout_secs for this server)",
                request_timeout.as_secs(),
                id
            )),
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

        match &mut self.link {
            Link::Stdio(link) => link.send_line(&notif_str, "notification").await,
            Link::Http(link) => {
                match tokio::time::timeout(request_timeout_for(&self.config), link.post_notification(&notif_str)).await {
                    Ok(result) => result,
                    Err(_) => Err(format!("Notification '{}' timed out", method)),
                }
            }
        }
    }

    /// The exit status if a spawned process has already ended; None while it runs or when there is no process.
    fn exited(&mut self) -> Result<Option<std::process::ExitStatus>, std::io::Error> {
        match &mut self.link {
            Link::Stdio(link) => link.process.try_wait(),
            Link::Http(_) => Ok(None),
        }
    }

    /// Stop the server: kill the process, or end the HTTP session.
    async fn kill(&mut self) -> Result<(), String> {
        match &mut self.link {
            Link::Stdio(link) => link
                .process
                .kill()
                .await
                .map_err(|e| format!("Failed to kill process: {}", e)),
            Link::Http(link) => {
                link.close().await;
                Ok(())
            }
        }
    }

    /// initialize, notifications/initialized, tools/list. Stdio keeps its own inline version of this
    /// (it adds process-start checks and stderr capture around it); the HTTP paths share this one.
    async fn handshake(&mut self, client_name: &str) -> Result<Vec<McpTool>, String> {
        let init = self
            .send_request(
                "initialize",
                Some(json!({
                    "protocolVersion": HTTP_PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "clientInfo": { "name": client_name, "version": "0.1.0" }
                })),
            )
            .await
            .map_err(|e| format!("Failed to initialize MCP server: {}", e))?;
        if let (Link::Http(link), Some(version)) = (&mut self.link, init.get("protocolVersion").and_then(|v| v.as_str())) {
            link.protocol_version = Some(version.to_string());
        }
        if let Err(e) = self.send_notification("notifications/initialized", None).await {
            println!("McpHostActor: Warning: Failed to send initialized notification: {}", e);
        }
        let listed = self
            .send_request("tools/list", None)
            .await
            .map_err(|e| format!("Failed to fetch tools: {}", e))?;
        Ok(listed
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|tools| tools.iter().filter_map(|t| serde_json::from_value::<McpTool>(t.clone()).ok()).collect())
            .unwrap_or_default())
    }
}

/// A connected server as the router keeps it. The connection sits behind its own lock so a tool call
/// that takes minutes holds only that server's lock, never the table: other servers, the tool list and the
/// settings screen keep working while it runs. `config` and `tools` are fixed at connect time.
struct McpServerEntry {
    config: McpServerConfig,
    tools: Vec<McpTool>,
    connection: Arc<tokio::sync::Mutex<McpServerConnection>>,
}

impl McpServerEntry {
    fn new(connection: McpServerConnection) -> Self {
        Self {
            config: connection.config.clone(),
            tools: connection.tools.clone(),
            connection: Arc::new(tokio::sync::Mutex::new(connection)),
        }
    }
}

/// MCP Host Actor - manages MCP server connections
pub struct McpToolRouterActor {
    mcp_tool_msg_rx: mpsc::Receiver<McpHostMsg>,
    connections: Arc<RwLock<HashMap<String, McpServerEntry>>>,
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
                    // Run the call in its own task. A tool can take minutes, and this loop must keep answering
                    // everything else (tool lists, connects, status) in the meantime.
                    let connections = self.connections.clone();
                    tokio::spawn(async move {
                        let result = execute_tool_on(&connections, &server_id, &tool_name, arguments).await;
                        let _ = respond_to.send(result);
                    });
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
        for (id, entry) in connections.drain() {
            println!("McpHostActor: Stopping server: {}", id);
            // A call still running holds the lock; the process (kill_on_drop) dies when that call lets go.
            if let Ok(mut conn) = entry.connection.try_lock() {
                let _ = conn.kill().await;
            }
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
            Transport::Http { .. } => self.connect_http_server(config).await,
        }
    }

    /// Connect to a server at a URL (MCP Streamable HTTP).
    async fn connect_http_server(&self, config: McpServerConfig) -> Result<(), String> {
        let url = match &config.transport {
            Transport::Http { url } => url.clone(),
            Transport::Stdio => return Err("Not an HTTP server".to_string()),
        };
        println!("McpHostActor: Connecting over HTTP to {}", url);
        let mut connection = McpServerConnection {
            config: config.clone(),
            link: Link::Http(HttpLink::new(&url)?),
            tools: Vec::new(),
            request_id: 0,
        };
        let tools = connection.handshake("plugable-chat").await?;
        let mode = if config.defer_tools { "DEFERRED" } else { "ACTIVE" };
        println!("McpHostActor: Server {} has {} tools [{}]", config.id, tools.len(), mode);
        connection.tools = tools;
        let server_id = config.id.clone();
        self.connections
            .write()
            .await
            .insert(server_id.clone(), McpServerEntry::new(connection));
        println!("McpHostActor: Server {} connected successfully", server_id);
        Ok(())
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
            .map_err(|e| describe_spawn_error(&command, &e))?;

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
            link: Link::Stdio(StdioLink { process: child, stdin, stdout_lines }),
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
        match connection.exited() {
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
                let _ = connection.kill().await;
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
            connections.insert(server_id.clone(), McpServerEntry::new(connection));
        }

        println!("McpHostActor: Server {} connected successfully", server_id);
        Ok(())
    }

    async fn disconnect_server(&self, server_id: &str) -> Result<(), String> {
        // Take it out of the table first so no new call can start. A call already running holds the
        // connection lock; it finishes (or times out) and the process then dies with the last reference
        // (kill_on_drop), so a disconnect never waits minutes for a slow tool.
        let removed = self.connections.write().await.remove(server_id);

        if let Some(entry) = removed {
            println!("McpHostActor: Disconnecting server: {}", server_id);
            if let Ok(mut conn) = entry.connection.try_lock() {
                conn.kill().await?;
            }
            Ok(())
        } else {
            Err(format!("Server {} not connected", server_id))
        }
    }

    async fn list_tools(&self, server_id: &str) -> Result<Vec<McpTool>, String> {
        let connections = self.connections.read().await;

        if let Some(entry) = connections.get(server_id) {
            Ok(entry.tools.clone())
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
        execute_tool_on(&self.connections, server_id, tool_name, arguments).await
    }

    async fn get_all_tool_descriptions(&self) -> Vec<(String, Vec<McpTool>)> {
        let connections = self.connections.read().await;

        let result: Vec<_> = connections
            .iter()
            .filter(|(_, entry)| entry.config.enabled)
            .map(|(id, entry)| (id.clone(), entry.tools.clone()))
            .collect();

        println!(
            "McpHostActor: get_all_tool_descriptions returning {} servers",
            result.len()
        );
        for (id, tools) in &result {
            if let Some(entry) = connections.get(id) {
                let mode = if entry.config.defer_tools {
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
            Transport::Http { url } => {
                let url = url.clone();
                self.test_http_server_config(config, &url).await
            }
        }
    }

    /// Test a server at a URL: initialize, list its tools, then end the session. Stores nothing.
    async fn test_http_server_config(&self, config: McpServerConfig, url: &str) -> Result<Vec<McpTool>, String> {
        let mut connection = McpServerConnection {
            config,
            link: Link::Http(HttpLink::new(url)?),
            tools: Vec::new(),
            request_id: 0,
        };
        let result = connection.handshake("plugable-chat-test").await;
        let _ = connection.kill().await;
        result
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
            .map_err(|e| describe_spawn_error(&command, &e))?;

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
            link: Link::Stdio(StdioLink { process: child, stdin, stdout_lines }),
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
        match connection.exited() {
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
                let _ = connection.kill().await;
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
                let _ = connection.kill().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stderr_output = stderr_buffer.lock().await;
                return Err(format_error_with_output(
                    format!("Failed to fetch tools: {}", e),
                    &stderr_output,
                ));
            }
        };

        // Clean up - kill the process
        let _ = connection.kill().await;

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

/// Execute a tool on a connected server. Free of the actor so the run loop can spawn it: only the one
/// server's connection lock is held while the call runs, never the table.
async fn execute_tool_on(
    connections: &RwLock<HashMap<String, McpServerEntry>>,
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

    // Hold the table lock only long enough to find the server; the call itself can take minutes.
    let connection = {
        let connections = connections.read().await;
        connections.get(server_id).map(|entry| entry.connection.clone())
    }
    .ok_or_else(|| {
        println!("║ ERROR: Server {} not connected", server_id);
        format!("Server {} not connected", server_id)
    })?;
    let mut connection = connection.lock().await;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_timeout(secs: Option<u64>) -> McpServerConfig {
        let mut config = McpServerConfig::new("s".to_string(), "s".to_string());
        config.request_timeout_secs = secs;
        config
    }

    #[test]
    fn a_server_waits_thirty_seconds_unless_it_sets_its_own_timeout() {
        assert_eq!(request_timeout_for(&config_with_timeout(None)).as_secs(), DEFAULT_REQUEST_TIMEOUT_SECS);
        assert_eq!(request_timeout_for(&config_with_timeout(Some(900))).as_secs(), 900);
        assert_eq!(request_timeout_for(&config_with_timeout(Some(0))).as_secs(), DEFAULT_REQUEST_TIMEOUT_SECS, "zero means unset");
    }

    #[test]
    fn a_saved_config_without_the_timeout_still_loads_and_does_not_write_one_back() {
        let old: McpServerConfig = serde_json::from_str(r#"{"id":"a","name":"a","enabled":true,"command":"x"}"#).unwrap();
        assert_eq!(old.request_timeout_secs, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("request_timeout_secs"));
        let set: McpServerConfig = serde_json::from_str(r#"{"id":"a","name":"a","enabled":true,"request_timeout_secs":900}"#).unwrap();
        assert_eq!(set.request_timeout_secs, Some(900));
    }

    #[test]
    fn a_ping_from_the_server_is_answered_and_other_server_messages_are_just_skipped() {
        let ping = server_originated_request(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#).unwrap();
        let reply: Value = serde_json::from_str(&ping.reply.unwrap()).unwrap();
        assert_eq!((reply["id"].as_u64(), &reply["result"]), (Some(7), &json!({})));
        let note = server_originated_request(r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{}}"#).unwrap();
        assert_eq!((note.method.as_str(), note.reply), ("notifications/progress", None));
        let roots = server_originated_request(r#"{"jsonrpc":"2.0","id":3,"method":"roots/list"}"#).unwrap();
        assert_eq!(roots.reply, None, "a request we cannot serve is skipped, not mistaken for a response");
        assert_eq!(server_originated_request(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#), None);
        assert_eq!(server_originated_request("not json at all"), None);
    }

    #[test]
    fn structured_content_is_read_and_stands_in_for_text_when_there_is_none() {
        let both: McpToolResult = serde_json::from_value(json!({
            "content": [{"type": "text", "text": "6,992 orders"}],
            "structuredContent": {"orders": 6992}
        })).unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&both), "6,992 orders", "text wins when there is text");
        let only: McpToolResult = serde_json::from_value(json!({"structuredContent": {"orders": 6992}})).unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&only), r#"{"orders":6992}"#);
        let neither: McpToolResult = serde_json::from_value(json!({"content": []})).unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&neither), "");
    }

    /// A scripted stdio MCP server in Python. Tools: `slow` (sleeps `seconds`), `structured`
    /// (structuredContent only), `noisy` (launcher noise, a ping and a notification before the answer).
    #[cfg(unix)]
    const SCRIPTED_SERVER: &str = r#"
import json, sys, time
print("launcher banner that is not JSON", flush=True)
for line in sys.stdin:
    msg = json.loads(line)
    method, mid = msg.get("method"), msg.get("id")
    if mid is None:
        continue
    if method == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "t", "version": "0"}}
    elif method == "tools/list":
        result = {"tools": [{"name": n, "description": n, "inputSchema": {"type": "object"}} for n in ("slow", "structured", "noisy")]}
    elif method == "tools/call":
        name, args = msg["params"]["name"], msg["params"].get("arguments", {})
        if name == "slow":
            time.sleep(args.get("seconds", 0))
            result = {"content": [{"type": "text", "text": "done"}]}
        elif name == "structured":
            result = {"content": [], "structuredContent": {"orders": 6992}}
        else:
            print("more launcher noise", flush=True)
            print(json.dumps({"jsonrpc": "2.0", "method": "notifications/message", "params": {}}), flush=True)
            print(json.dumps({"jsonrpc": "2.0", "id": 900, "method": "ping"}), flush=True)
            result = {"content": [{"type": "text", "text": "answered"}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": mid, "result": result}), flush=True)
"#;

    #[cfg(unix)]
    async fn connect_scripted_server(timeout_secs: Option<u64>) -> Option<McpToolRouterActor> {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            eprintln!("python3 is not available; skipping the scripted-server test");
            return None;
        }
        let (_tx, rx) = mpsc::channel(1);
        let actor = McpToolRouterActor::new(rx);
        let mut config = McpServerConfig::new("scripted".to_string(), "scripted".to_string());
        config.enabled = true;
        config.command = Some("python3".to_string());
        config.args = vec!["-u".to_string(), "-c".to_string(), SCRIPTED_SERVER.to_string()];
        config.request_timeout_secs = timeout_secs;
        actor.connect_stdio_server(config).await.expect("the scripted server connects");
        Some(actor)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_slow_tool_call_times_out_at_the_servers_own_limit_and_succeeds_under_a_longer_one() {
        let Some(short) = connect_scripted_server(Some(1)).await else { return };
        let err = short.execute_tool("scripted", "slow", json!({"seconds": 3})).await.unwrap_err();
        assert!(err.contains("timed out after 1s"), "{err}");
        let Some(long) = connect_scripted_server(Some(10)).await else { return };
        let ok = long.execute_tool("scripted", "slow", json!({"seconds": 2})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&ok), "done");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn structured_only_results_and_noisy_servers_work_end_to_end() {
        let Some(actor) = connect_scripted_server(Some(10)).await else { return };
        let structured = actor.execute_tool("scripted", "structured", json!({})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&structured), r#"{"orders":6992}"#);
        let noisy = actor.execute_tool("scripted", "noisy", json!({})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&noisy), "answered");
    }

    // ---- HTTP transport ---------------------------------------------------------------------------

    #[test]
    fn a_json_reply_and_an_event_stream_reply_both_yield_the_response_to_our_request() {
        let json_body = r#"{"jsonrpc":"2.0","id":4,"result":{"ok":true}}"#;
        let r = parse_http_reply("application/json", json_body, 4).unwrap();
        assert_eq!((r.id, r.result), (Some(4), Some(json!({"ok": true}))));

        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":4,\"result\":{\"ok\":true}}\n\n";
        let r = parse_http_reply("text/event-stream; charset=utf-8", sse, 4).unwrap();
        assert_eq!(r.result, Some(json!({"ok": true})));
    }

    #[test]
    fn server_messages_and_other_ids_in_a_reply_are_skipped_and_a_batch_is_searched() {
        let sse = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{\"stale\":true}}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{\"mine\":true}}\n\n",
        );
        assert_eq!(parse_http_reply("text/event-stream", sse, 5).unwrap().result, Some(json!({"mine": true})));
        let batch = r#"[{"jsonrpc":"2.0","id":1,"result":{}},{"jsonrpc":"2.0","id":2,"result":{"two":2}}]"#;
        assert_eq!(parse_http_reply("application/json", batch, 2).unwrap().result, Some(json!({"two": 2})));
        // multi-line data fields are joined with a newline before parsing
        let split = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":6,\"result\":{}}\n\n";
        assert!(parse_http_reply("text/event-stream", split, 6).is_ok());
    }

    #[test]
    fn a_reply_with_no_answer_to_our_request_is_an_error_that_says_so() {
        let err = parse_http_reply("application/json", r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, 2).unwrap_err();
        assert!(err.contains("no response to request 2"), "{err}");
        assert!(parse_http_reply("application/json", "not json", 1).is_err());
        assert!(parse_http_reply("text/event-stream", "", 1).is_err());
    }

    #[test]
    fn an_http_server_needs_a_real_http_url() {
        for bad in ["", "   ", "not a url", "ftp://box/mcp", "box:8787/mcp"] {
            assert!(HttpLink::new(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(HttpLink::new("http://100.127.91.127:8787/mcp").is_ok());
        assert!(HttpLink::new("https://example.test/mcp").is_ok());
    }

    fn http_config(url: &str, timeout_secs: Option<u64>) -> McpServerConfig {
        let mut config = McpServerConfig::new("stub".to_string(), "stub".to_string());
        config.enabled = true;
        config.transport = Transport::Http { url: url.to_string() };
        config.request_timeout_secs = timeout_secs;
        config
    }

    async fn stub_url(query: &str) -> String {
        let addr = mcp_test_server::spawn_mcp_only_server().await.expect("the stub server starts");
        format!("http://{}/mcp{}", addr, query)
    }

    async fn connect_http(url: &str, timeout_secs: Option<u64>) -> Result<McpToolRouterActor, String> {
        let (_tx, rx) = mpsc::channel(1);
        let actor = McpToolRouterActor::new(rx);
        actor.connect_http_server(http_config(url, timeout_secs)).await?;
        Ok(actor)
    }

    #[tokio::test]
    async fn an_http_server_connects_lists_its_tools_and_answers_a_call() {
        let actor = connect_http(&stub_url("").await, None).await.expect("connects");
        let tools = actor.list_tools("stub").await.unwrap();
        assert!(tools.iter().any(|t| t.name == "test_echo") && tools.iter().any(|t| t.name == "test_sleep"), "{tools:?}");
        let result = actor.execute_tool("stub", "test_echo", json!({"message": "hello"})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&result), "Echo: hello");
        let err = actor.execute_tool("stub", "test_error", json!({"error_type": "runtime"})).await.unwrap();
        assert!(err.is_error, "a tool's own error stays an is_error result, not a transport failure");
    }

    #[tokio::test]
    async fn an_event_stream_reply_works_end_to_end() {
        let actor = connect_http(&stub_url("?reply=sse").await, None).await.expect("connects over SSE-framed replies");
        let result = actor.execute_tool("stub", "test_echo", json!({"message": "via sse"})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&result), "Echo: via sse");
    }

    #[tokio::test]
    async fn a_non_200_reply_names_the_status_and_the_url() {
        let url = stub_url("?status=503").await;
        let err = connect_http(&url, None).await.err().expect("the server refuses");
        assert!(err.contains("HTTP 503") && err.contains(&url), "{err}");
        let gone = connect_http("http://127.0.0.1:9/mcp", None).await.err().expect("nothing listens there");
        assert!(gone.contains("Could not reach"), "{gone}");
    }

    #[tokio::test]
    async fn a_long_http_call_times_out_at_the_servers_limit_naming_the_setting_and_succeeds_under_a_longer_one() {
        let url = stub_url("").await;
        let short = connect_http(&url, Some(1)).await.unwrap();
        let err = short.execute_tool("stub", "test_sleep", json!({"seconds": 3})).await.unwrap_err();
        assert!(err.contains("timed out after 1s") && err.contains("request_timeout_secs"), "{err}");
        let long = connect_http(&url, Some(10)).await.unwrap();
        let ok = long.execute_tool("stub", "test_sleep", json!({"seconds": 2})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&ok), "Slept 2 seconds");
    }

    #[tokio::test]
    async fn a_call_that_runs_for_minutes_does_not_freeze_the_rest_of_the_router() {
        let (_tx, rx) = mpsc::channel(1);
        let actor = Arc::new(McpToolRouterActor::new(rx));
        let url = stub_url("").await;
        actor.connect_http_server(http_config(&url, Some(30))).await.unwrap();
        let mut other = http_config(&url, Some(30));
        other.id = "other".to_string();
        actor.connect_http_server(other).await.unwrap();

        let slow = { let actor = actor.clone(); tokio::spawn(async move { actor.execute_tool("stub", "test_sleep", json!({"seconds": 3})).await }) };
        tokio::time::sleep(Duration::from_millis(300)).await; // the slow call is now in flight

        let started = std::time::Instant::now();
        let descriptions = actor.get_all_tool_descriptions().await;
        assert_eq!(descriptions.len(), 2);
        assert!(actor.list_tools("stub").await.is_ok());
        let quick = actor.execute_tool("other", "test_echo", json!({"message": "still here"})).await.unwrap();
        assert_eq!(crate::tool_execution::mcp_result_text(&quick), "Echo: still here");
        assert!(started.elapsed() < Duration::from_secs(2), "other work waited {:?} behind a slow call", started.elapsed());
        assert!(slow.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn the_connection_test_for_an_http_server_lists_tools_and_reports_a_bad_url() {
        let (_tx, rx) = mpsc::channel(1);
        let actor = McpToolRouterActor::new(rx);
        let tools = actor.test_server_config(http_config(&stub_url("").await, None)).await.unwrap();
        assert!(tools.len() >= 5, "{tools:?}");
        let err = actor.test_server_config(http_config("", None)).await.unwrap_err();
        assert!(err.contains("No URL"), "{err}");
    }

    #[tokio::test]
    async fn the_actor_loop_keeps_answering_while_a_tool_call_runs() {
        // Through the real message channel: a slow ExecuteTool must not hold up a GetAllToolDescriptions behind it.
        let (tx, rx) = mpsc::channel(8);
        let actor = McpToolRouterActor::new(rx);
        tokio::spawn(actor.run());
        let url = stub_url("").await;

        let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
        tx.send(McpHostMsg::ConnectServer { config: http_config(&url, Some(30)), respond_to: connected_tx }).await.unwrap();
        connected_rx.await.unwrap().expect("connects");

        let (slow_tx, slow_rx) = tokio::sync::oneshot::channel();
        tx.send(McpHostMsg::ExecuteTool {
            server_id: "stub".to_string(),
            tool_name: "test_sleep".to_string(),
            arguments: json!({"seconds": 3}),
            respond_to: slow_tx,
        })
        .await
        .unwrap();

        let started = std::time::Instant::now();
        let (desc_tx, desc_rx) = tokio::sync::oneshot::channel();
        tx.send(McpHostMsg::GetAllToolDescriptions { respond_to: desc_tx }).await.unwrap();
        let descriptions = tokio::time::timeout(Duration::from_secs(2), desc_rx)
            .await
            .expect("the tool list must not wait behind a running tool call")
            .unwrap();
        assert_eq!(descriptions.len(), 1);
        assert!(started.elapsed() < Duration::from_secs(2));
        let slow = slow_rx.await.unwrap().expect("the slow call still completes");
        assert_eq!(crate::tool_execution::mcp_result_text(&slow), "Slept 3 seconds");
    }
}

#[cfg(test)]
mod spawn_error_tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    // rc18 (Pat): "cargo: program not found" named the program but not what to install.
    #[test]
    fn a_missing_program_says_what_to_install_for_the_common_launchers() {
        let missing = Error::from(ErrorKind::NotFound);
        let cargo = describe_spawn_error("cargo", &missing);
        assert!(cargo.contains("'cargo'") && cargo.contains("not found") && cargo.contains("Rust"), "{cargo}");
        assert!(describe_spawn_error("npx", &missing).contains("Node.js"));
        assert!(describe_spawn_error("C:\\tools\\uvx.exe", &missing).contains("uv"));
        assert!(describe_spawn_error("python3", &missing).contains("Python"));
        let other = describe_spawn_error("my-server", &missing);
        assert!(other.contains("'my-server'") && other.contains("Settings > Tools"), "{other}");
    }

    #[test]
    fn other_spawn_failures_keep_the_operating_system_reason() {
        let denied = describe_spawn_error("tool", &Error::from(ErrorKind::PermissionDenied));
        assert!(denied.contains("Failed to spawn MCP server process 'tool'") && denied.to_lowercase().contains("permission denied"), "{denied}");
    }
}
