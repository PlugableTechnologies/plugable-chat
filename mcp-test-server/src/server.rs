//! MCP Test Server for Plugable Chat (dev edition with UI + CLI)
//!
//! - Serves MCP over stdio
//! - Hosts a lightweight web UI with red/green status + logs
//! - Provides a CLI switch to auto-run all tests and print a ready-made prompt

use axum::{
    extract::State as AxumState,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};

// -----------------------------------------------------------------------------
// Constants & prompt helpers
// -----------------------------------------------------------------------------

pub const DEFAULT_PROMPT: &str = "Connect to the dev MCP test server and run all tests. Report red/green for each test, a summary, and any errors or logs you see.";
pub const DEFAULT_HOST: &str = "127.0.0.1";
// Use a less common default port to reduce clashes with local services.
pub const DEFAULT_PORT: u16 = 43030;

// -----------------------------------------------------------------------------
// CLI
// -----------------------------------------------------------------------------

#[derive(Parser, Debug, Clone)]
#[command(name = "mcp-test-server", about = "Dev MCP test server with UI")]
pub struct CliArgs {
    /// Host interface for the web UI
    #[arg(long, default_value = DEFAULT_HOST)]
    pub host: String,
    /// Port for the web UI
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
    /// Auto-run the full test sweep on startup
    #[arg(long, default_value_t = false, value_parser = clap::builder::BoolishValueParser::new(), action = clap::ArgAction::Set)]
    pub run_all_on_start: bool,
    /// Print a ready-made prompt to stdout
    #[arg(long, default_value_t = true, value_parser = clap::builder::BoolishValueParser::new(), action = clap::ArgAction::Set)]
    pub print_prompt: bool,
    /// Automatically open the UI in the browser
    #[arg(long, default_value_t = true, value_parser = clap::builder::BoolishValueParser::new(), action = clap::ArgAction::Set)]
    pub open_ui: bool,
    /// Disable hosting the UI (stdio MCP only)
    #[arg(long, default_value_t = true, value_parser = clap::builder::BoolishValueParser::new(), action = clap::ArgAction::Set)]
    pub serve_ui: bool,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            run_all_on_start: false,
            print_prompt: true,
            open_ui: true,
            serve_ui: true,
        }
    }
}

// -----------------------------------------------------------------------------
// Core state & models
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum TestStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResult {
    id: String,
    name: String,
    status: TestStatus,
    message: String,
    started_at: String,
    finished_at: String,
    duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummary {
    total: usize,
    passed: usize,
    failed: usize,
    duration_ms: u128,
    last_started_at: Option<String>,
    last_finished_at: Option<String>,
    running: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntry {
    timestamp: String,
    level: String,
    message: String,
}

#[derive(Debug, Default)]
struct SharedState {
    tests: RwLock<Vec<TestResult>>, // latest run per test id
    logs: RwLock<Vec<LogEntry>>,    // rolling log for UI
    running: Mutex<bool>,
    last_summary: RwLock<Option<RunSummary>>, // cached summary for quick fetch
}

impl SharedState {
    fn new() -> Self {
        Self {
            tests: RwLock::new(Vec::new()),
            logs: RwLock::new(Vec::new()),
            running: Mutex::new(false),
            last_summary: RwLock::new(None),
        }
    }

    async fn log(&self, level: &str, message: impl Into<String>) {
        let entry = LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            level: level.to_string(),
            message: message.into(),
        };
        let mut logs = self.logs.write().await;
        if logs.len() >= 500 {
            logs.drain(0..100);
        }
        logs.push(entry);
    }

    async fn upsert_test(&self, result: TestResult) {
        let mut tests = self.tests.write().await;
        if let Some(pos) = tests.iter().position(|t| t.id == result.id) {
            tests[pos] = result;
        } else {
            tests.push(result);
        }
    }

    async fn set_tests(&self, results: Vec<TestResult>) {
        let mut tests = self.tests.write().await;
        *tests = results;
    }

    async fn set_summary(&self, summary: RunSummary) {
        let mut s = self.last_summary.write().await;
        *s = Some(summary);
    }

    async fn summary(&self) -> RunSummary {
        if let Some(existing) = self.last_summary.read().await.clone() {
            return existing;
        }
        RunSummary {
            total: 0,
            passed: 0,
            failed: 0,
            duration_ms: 0,
            last_started_at: None,
            last_finished_at: None,
            running: false,
        }
    }

    async fn is_running(&self) -> bool {
        *self.running.lock().await
    }
}

// -----------------------------------------------------------------------------
// JSON-RPC structs (MCP)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Serialize)]
struct Tool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct Content {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

// -----------------------------------------------------------------------------
// Atomic counters (retained for compatibility with existing tools)
// -----------------------------------------------------------------------------

static TESTS_RUN: AtomicU64 = AtomicU64::new(0);
static TESTS_PASSED: AtomicU64 = AtomicU64::new(0);
static TESTS_FAILED: AtomicU64 = AtomicU64::new(0);

fn reset_counters() {
    TESTS_RUN.store(0, Ordering::Relaxed);
    TESTS_PASSED.store(0, Ordering::Relaxed);
    TESTS_FAILED.store(0, Ordering::Relaxed);
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

pub async fn run_with_args(args: CliArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = Arc::new(SharedState::new());

    println!(
        "[mcp-test-server] starting (host={}, port={}, serve_ui={}, open_ui={}, run_all_on_start={}, print_prompt={})",
        args.host, args.port, args.serve_ui, args.open_ui, args.run_all_on_start, args.print_prompt
    );

    if args.print_prompt {
        eprintln!("[mcp-test-server] Suggested prompt: {}", DEFAULT_PROMPT);
    }

    if args.run_all_on_start {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let _ = run_all_tests(state_clone, TriggerSource::Cli).await;
        });
    }

    let http_handle = if args.serve_ui {
        let state_clone = state.clone();
        let host = args.host.clone();
        let port = args.port;
        let open_flag = args.open_ui;
        Some(tokio::spawn(async move {
            if let Err(e) = serve_ui(host, port, state_clone, open_flag).await {
                eprintln!("[mcp-test-server] UI server exited: {}", e);
            }
        }))
    } else {
        None
    };

    let stdio_state = state.clone();
    let stdio_handle = tokio::spawn(async move {
        if let Err(e) = run_stdio_loop(stdio_state).await {
            eprintln!("[mcp-test-server] MCP loop exited: {}", e);
        }
    });

    if let Some(handle) = http_handle {
        let _ = handle.await;
    }
    let _ = stdio_handle.await;
    Ok(())
}

// -----------------------------------------------------------------------------
// MCP stdio loop
// -----------------------------------------------------------------------------

async fn run_stdio_loop(state: Arc<SharedState>) -> io::Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    println!("[mcp-test-server] stdio MCP loop ready on stdin/stdout");

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        println!("[mcp-test-server] MCP recv: {}", trimmed);
        state.log("info", format!("MCP recv: {}", trimmed)).await;

        match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(request) => {
                let response = handle_request(request, state.clone()).await;
                let response_str = serde_json::to_string(&response).unwrap_or_else(|e| {
                    format!("{{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{{\"code\":-32000,\"message\":\"Serialize error: {}\"}}}}", e)
                });
                state
                    .log("info", format!("MCP send: {}", response_str))
                    .await;
                stdout.write_all(response_str.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
            Err(e) => {
                state.log("error", format!("Parse error: {}", e)).await;
                let error_response = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                        data: None,
                    }),
                };
                let response_str = serde_json::to_string(&error_response).unwrap();
                stdout.write_all(response_str.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
    }

    Ok(())
}

async fn handle_request(request: JsonRpcRequest, state: Arc<SharedState>) -> JsonRpcResponse {
    let id = request.id.unwrap_or(Value::Null);

    match request.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mcp-test-server", "version": "0.2.0"}
            })),
            error: None,
        },
        "notifications/initialized" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({})),
            error: None,
        },
        "tools/list" => {
            let tools = get_tools();
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({ "tools": tools })),
                error: None,
            }
        }
        "tools/call" => {
            let params = request.params.unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            // Special-case run_all_tests to avoid recursive async fn sizing
            if tool_name == "run_all_tests" {
                let summary = run_all_tests(state.clone(), TriggerSource::Tool).await;
                return JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(json!({
                        "content": [{ "type": "text", "text": format!(
                            "Run complete: {} passed / {} failed ({} total) in {} ms",
                            summary.passed, summary.failed, summary.total, summary.duration_ms
                        )}],
                        "isError": summary.failed > 0
                    })),
                    error: None,
                };
            }

            TESTS_RUN.fetch_add(1, Ordering::Relaxed);

            match execute_tool(tool_name, arguments, state.clone()).await {
                Ok(result) => {
                    TESTS_PASSED.fetch_add(1, Ordering::Relaxed);
                    JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: Some(json!({
                            "content": [{ "type": "text", "text": result }],
                            "isError": false
                        })),
                        error: None,
                    }
                }
                Err(error) => {
                    TESTS_FAILED.fetch_add(1, Ordering::Relaxed);
                    JsonRpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: Some(json!({
                            "content": [{ "type": "text", "text": error }],
                            "isError": true
                        })),
                        error: None,
                    }
                }
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", request.method),
                data: None,
            }),
        },
    }
}

// -----------------------------------------------------------------------------
// Tooling
// -----------------------------------------------------------------------------

fn get_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "test_echo".to_string(),
            description: "Echo test - returns the input message.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string", "description": "The message to echo back"}
                },
                "required": ["message"]
            }),
        },
        Tool {
            name: "test_math".to_string(),
            description: "Math test - performs simple arithmetic operations.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["add", "subtract", "multiply", "divide"],
                        "description": "The math operation to perform"
                    },
                    "a": {"type": "number", "description": "First operand"},
                    "b": {"type": "number", "description": "Second operand"}
                },
                "required": ["operation", "a", "b"]
            }),
        },
        Tool {
            name: "test_json".to_string(),
            description: "JSON test - returns a structured JSON response.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "include_nested": {
                        "type": "boolean",
                        "description": "Whether to include nested objects in the response"
                    }
                }
            }),
        },
        Tool {
            name: "test_error".to_string(),
            description: "Error test - intentionally returns an error.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "error_type": {
                        "type": "string",
                        "enum": ["validation", "runtime", "timeout"],
                        "description": "The type of error to simulate"
                    }
                },
                "required": ["error_type"]
            }),
        },
        Tool {
            name: "get_test_status".to_string(),
            description: "Returns the current test status with pass/fail counts.".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "get_test_prompts".to_string(),
            description: "Returns sample prompts for testing MCP integration.".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "reset_test_counts".to_string(),
            description: "Resets all test counters to zero.".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
    ]
}

async fn execute_tool(name: &str, args: Value, _state: Arc<SharedState>) -> Result<String, String> {
    match name {
        "test_echo" => {
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: message".to_string())?;
            Ok(format!("Echo: {}", message))
        }
        "test_math" => {
            let operation = args
                .get("operation")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: operation".to_string())?;
            let a = args
                .get("a")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "Missing required parameter: a".to_string())?;
            let b = args
                .get("b")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "Missing required parameter: b".to_string())?;

            let result = match operation {
                "add" => a + b,
                "subtract" => a - b,
                "multiply" => a * b,
                "divide" => {
                    if b == 0.0 {
                        return Err("Division by zero".to_string());
                    }
                    a / b
                }
                _ => return Err(format!("Unknown operation: {}", operation)),
            };

            Ok(format!("{} {} {} = {}", a, operation, b, result))
        }
        "test_json" => {
            let include_nested = args
                .get("include_nested")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let mut response = json!({
                "status": "success",
                "timestamp": Utc::now().to_rfc3339(),
                "data": {
                    "string_field": "Hello, MCP!",
                    "number_field": 42,
                    "boolean_field": true,
                    "array_field": [1, 2, 3, 4, 5]
                }
            });

            if include_nested {
                response["data"]["nested"] = json!({
                    "level1": {"level2": {"level3": "deeply nested value"}}
                });
            }

            Ok(serde_json::to_string_pretty(&response).unwrap())
        }
        "test_error" => {
            let error_type = args
                .get("error_type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter: error_type".to_string())?;

            match error_type {
                "validation" => Err("Validation error: Input failed schema validation".to_string()),
                "runtime" => Err("Runtime error: An unexpected condition occurred".to_string()),
                "timeout" => Err("Timeout error: Operation exceeded time limit".to_string()),
                _ => Err(format!("Unknown error type: {}", error_type)),
            }
        }
        "get_test_status" => {
            let run = TESTS_RUN.load(Ordering::Relaxed);
            let passed = TESTS_PASSED.load(Ordering::Relaxed);
            let failed = TESTS_FAILED.load(Ordering::Relaxed);

            let status = if failed == 0 && passed > 0 {
                "🟢 ALL TESTS PASSING"
            } else if failed > 0 {
                "🔴 SOME TESTS FAILED"
            } else {
                "⚪ NO TESTS RUN YET"
            };

            Ok(format!(
                "{}\n\nTest Summary:\n  Total: {}\n  Passed: {} ✓\n  Failed: {} ✗\n  Pass Rate: {:.1}%",
                status,
                run,
                passed,
                failed,
                if run > 0 {
                    (passed as f64 / run as f64) * 100.0
                } else {
                    0.0
                }
            ))
        }
        "get_test_prompts" => Ok(format!(
            r#"Here are sample prompts to test the MCP integration:

1. Basic Echo Test
   "Please use the test_echo tool to echo the message 'Hello from MCP!'"

2. Math Test
   "Use the test_math tool to calculate 15 multiplied by 7"

3. JSON Response Test
   "Call test_json with include_nested set to true and show me the result"

4. Error Handling Test
   "Use test_error with error_type 'validation' to test error handling"

5. Status Check
   "Check the current test status using get_test_status"

Use this master prompt to run everything:
"{}""#,
            DEFAULT_PROMPT
        )),
        "reset_test_counts" => {
            reset_counters();
            Ok("Test counters have been reset to zero.".to_string())
        }
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

// -----------------------------------------------------------------------------
// Test runner
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TestCase {
    id: &'static str,
    name: &'static str,
    expect_error: bool,
    description: &'static str,
}

enum TriggerSource {
    Cli,
    Http,
    Tool,
}

fn test_cases() -> Vec<TestCase> {
    vec![
        TestCase {
            id: "echo",
            name: "Echo returns message",
            expect_error: false,
            description: "Connectivity and echo path",
        },
        TestCase {
            id: "math_multiply",
            name: "Math multiply",
            expect_error: false,
            description: "Arithmetic path multiply",
        },
        TestCase {
            id: "math_div_zero",
            name: "Math divide by zero errors",
            expect_error: true,
            description: "Division by zero should error",
        },
        TestCase {
            id: "json_nested",
            name: "JSON nested output",
            expect_error: false,
            description: "Structured JSON formatting",
        },
        TestCase {
            id: "error_validation",
            name: "Intentional error surface",
            expect_error: true,
            description: "Expected validation error surfaces",
        },
        TestCase {
            id: "status",
            name: "Status summary",
            expect_error: false,
            description: "Aggregated status responds",
        },
        TestCase {
            id: "prompts",
            name: "Prompts helper",
            expect_error: false,
            description: "Sample prompts are returned",
        },
    ]
}

async fn run_all_tests(state: Arc<SharedState>, trigger: TriggerSource) -> RunSummary {
    let mut guard = state.running.lock().await;
    if *guard {
        state
            .log(
                "warn",
                "Run-all requested but already running; ignoring new request",
            )
            .await;
        println!("[mcp-test-server] run-all ignored: already running");
        return state.summary().await;
    }
    *guard = true;
    drop(guard);

    let label = trigger_label(&trigger);
    state
        .log("info", format!("Run-all started ({})", label))
        .await;
    println!("[mcp-test-server] run-all started ({})", label);
    reset_counters();

    let run_started = Utc::now().to_rfc3339();
    state
        .set_summary(RunSummary {
            total: 0,
            passed: 0,
            failed: 0,
            duration_ms: 0,
            last_started_at: Some(run_started.clone()),
            last_finished_at: None,
            running: true,
        })
        .await;

    let cases = test_cases();
    let mut results: Vec<TestResult> = Vec::new();
    let start_clock = Instant::now();

    for case in cases.iter() {
        let case_start = Instant::now();
        state
            .log(
                "info",
                format!("[{}] starting: {}", case.id, case.description),
            )
            .await;

        let outcome = match case.id {
            "echo" => {
                execute_tool(
                    "test_echo",
                    json!({"message": "Hello from test runner"}),
                    state.clone(),
                )
                .await
            }
            "math_multiply" => {
                execute_tool(
                    "test_math",
                    json!({"operation": "multiply", "a": 6, "b": 7}),
                    state.clone(),
                )
                .await
            }
            "math_div_zero" => {
                execute_tool(
                    "test_math",
                    json!({"operation": "divide", "a": 1, "b": 0}),
                    state.clone(),
                )
                .await
            }
            "json_nested" => {
                execute_tool("test_json", json!({"include_nested": true}), state.clone()).await
            }
            "error_validation" => {
                execute_tool(
                    "test_error",
                    json!({"error_type": "validation"}),
                    state.clone(),
                )
                .await
            }
            "status" => execute_tool("get_test_status", json!({}), state.clone()).await,
            "prompts" => execute_tool("get_test_prompts", json!({}), state.clone()).await,
            _ => Err("Unknown test case".to_string()),
        };

        let duration_ms = case_start.elapsed().as_millis();
        let finished = Utc::now().to_rfc3339();

        let (status, message) = match (outcome, case.expect_error) {
            (Ok(msg), false) => (TestStatus::Pass, msg),
            (Ok(msg), true) => (
                TestStatus::Fail,
                format!("Expected error but got success: {}", msg),
            ),
            (Err(err), true) => (TestStatus::Pass, err),
            (Err(err), false) => (TestStatus::Fail, err),
        };

        let result = TestResult {
            id: case.id.to_string(),
            name: case.name.to_string(),
            status: status.clone(),
            message,
            started_at: run_started.clone(),
            finished_at: finished,
            duration_ms,
        };

        if matches!(status, TestStatus::Pass) {
            TESTS_PASSED.fetch_add(1, Ordering::Relaxed);
        } else {
            TESTS_FAILED.fetch_add(1, Ordering::Relaxed);
        }
        TESTS_RUN.fetch_add(1, Ordering::Relaxed);

        state.upsert_test(result.clone()).await;
        results.push(result.clone());
        state
            .log(
                "info",
                format!(
                    "[{}] {} ({} ms)",
                    case.id,
                    match status {
                        TestStatus::Pass => "PASS",
                        TestStatus::Fail => "FAIL",
                    },
                    duration_ms
                ),
            )
            .await;
    }

    let elapsed = start_clock.elapsed().as_millis();
    let passed = results
        .iter()
        .filter(|r| matches!(r.status, TestStatus::Pass))
        .count();
    let failed = results.len() - passed;

    let summary = RunSummary {
        total: results.len(),
        passed,
        failed,
        duration_ms: elapsed,
        last_started_at: Some(run_started.clone()),
        last_finished_at: Some(Utc::now().to_rfc3339()),
        running: false,
    };

    state.set_tests(results).await;
    state.set_summary(summary.clone()).await;
    state
        .log(
            "info",
            format!(
                "Run-all finished: {} passed / {} failed in {} ms",
                summary.passed, summary.failed, summary.duration_ms
            ),
        )
        .await;
    println!(
        "[mcp-test-server] run-all finished: total={}, passed={}, failed={}, duration_ms={}",
        summary.total, summary.passed, summary.failed, summary.duration_ms
    );

    let mut guard_done = state.running.lock().await;
    *guard_done = false;
    summary
}

fn trigger_label(trigger: &TriggerSource) -> &'static str {
    match trigger {
        TriggerSource::Cli => "cli",
        TriggerSource::Http => "http",
        TriggerSource::Tool => "tool",
    }
}

// -----------------------------------------------------------------------------
// HTTP UI
// -----------------------------------------------------------------------------

async fn serve_ui(
    host: String,
    port: u16,
    state: Arc<SharedState>,
    open_ui: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Router::new()
        .route("/", get(ui_handler))
        .route("/api/status", get(status_handler))
        .route("/api/logs", get(logs_handler))
        .route("/api/run-all", post(run_all_handler))
        .with_state(state.clone());

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    println!(
        "[mcp-test-server] Web UI listening on http://{} (open_ui={})",
        actual_addr, open_ui
    );
    state
        .log(
            "info",
            format!(
                "Web UI available at http://{} (open_ui={})",
                actual_addr, open_ui
            ),
        )
        .await;

    if open_ui {
        let _ = open::that(format!("http://{}", actual_addr));
    }

    axum::serve(listener, app).await?;
    Ok(())
}

async fn ui_handler() -> Html<String> {
    println!("[mcp-test-server] GET / (ui root)");
    Html(UI_HTML.to_string())
}

async fn status_handler(AxumState(state): AxumState<Arc<SharedState>>) -> Json<Value> {
    println!("[mcp-test-server] /api/status requested");
    let tests = state.tests.read().await.clone();
    let mut summary = state.summary().await;
    summary.running = state.is_running().await;

    println!(
        "[mcp-test-server] /api/status response: total={}, passed={}, failed={}, running={}",
        summary.total, summary.passed, summary.failed, summary.running
    );

    Json(json!({
        "summary": summary,
        "tests": tests,
        "prompt": DEFAULT_PROMPT,
    }))
}

async fn logs_handler(AxumState(state): AxumState<Arc<SharedState>>) -> Json<Value> {
    println!("[mcp-test-server] /api/logs requested");
    let logs = state.logs.read().await.clone();
    println!(
        "[mcp-test-server] /api/logs response: {} entries",
        logs.len()
    );
    Json(json!({"logs": logs}))
}

async fn run_all_handler(AxumState(state): AxumState<Arc<SharedState>>) -> Json<Value> {
    println!("[mcp-test-server] /api/run-all requested");
    let state_clone = state.clone();
    tokio::spawn(async move {
        println!("[mcp-test-server] run-all task spawned");
        let summary = run_all_tests(state_clone, TriggerSource::Http).await;
        println!(
            "[mcp-test-server] run-all task finished: total={}, passed={}, failed={}, duration_ms={}",
            summary.total, summary.passed, summary.failed, summary.duration_ms
        );
    });
    Json(json!({"status": "accepted"}))
}

// -----------------------------------------------------------------------------
// UI markup (inline for simplicity)
// -----------------------------------------------------------------------------

const UI_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>MCP Test Server</title>
  <style>
    body { font-family: system-ui, -apple-system, sans-serif; background: #0f172a; color: #e2e8f0; margin: 0; padding: 16px; }
    h1 { margin: 0 0 12px 0; }
    button { background: #0ea5e9; color: #0b1224; border: none; padding: 10px 14px; border-radius: 8px; cursor: pointer; font-weight: 600; }
    button:disabled { opacity: 0.5; cursor: not-allowed; }
    .card { background: #1e293b; border: 1px solid #334155; border-radius: 12px; padding: 12px; margin-bottom: 12px; }
    .row { display: flex; gap: 12px; flex-wrap: wrap; }
    .badge { padding: 4px 8px; border-radius: 999px; font-weight: 700; }
    .pass { background: #22c55e33; color: #22c55e; }
    .fail { background: #ef444433; color: #ef4444; }
    .pending { background: #cbd5e133; color: #cbd5e1; }
    table { width: 100%; border-collapse: collapse; }
    th, td { padding: 8px; text-align: left; border-bottom: 1px solid #334155; }
    pre { background: #0b1224; padding: 8px; border-radius: 8px; overflow-x: auto; }
    .log { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12px; }
  </style>
</head>
<body>
  <h1>MCP Test Server (dev)</h1>
  <div class="row">
    <button id="runAll">Run all tests</button>
    <div class="badge pending" id="running">Idle</div>
  </div>
  <div class="card">
    <div id="summary">Loading summary...</div>
    <div style="margin-top:8px"><strong>Prompt:</strong> <code id="prompt"></code></div>
  </div>
  <div class="card">
    <h3 style="margin-top:0">Tests</h3>
    <table id="tests"></table>
  </div>
  <div class="card">
    <h3 style="margin-top:0">Logs (latest)</h3>
    <div id="logs"></div>
  </div>
  <div class="card">
    <h3 style="margin-top:0">Debug</h3>
    <div id="debug" class="log"></div>
  </div>
<script>
function appendDebug(msg) {
  const el = document.getElementById('debug');
  const time = new Date().toISOString();
  el.innerHTML = `<div>[${time}] ${msg}</div>` + el.innerHTML;
}

appendDebug('ui boot');

async function fetchJSON(url, options) {
  const res = await fetch(url, { cache: 'no-store', ...options });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`${res.status} ${res.statusText}${text ? ' - ' + text : ''}`);
  }
  return res.json();
}

window.addEventListener('error', (e) => {
  appendDebug(`window error: ${e.message}`);
});
window.addEventListener('unhandledrejection', (e) => {
  appendDebug(`unhandled rejection: ${e.reason}`);
});

async function refresh() {
  try {
    const data = await fetchJSON('/api/status');
    const summary = data.summary || {};
    document.getElementById('prompt').textContent = data.prompt || '';

    const running = summary.running;
    const runBadge = document.getElementById('running');
    runBadge.textContent = running ? 'Running' : 'Idle';
    runBadge.className = 'badge ' + (running ? 'pending' : 'pass');
    document.getElementById('runAll').disabled = running;

    const duration = summary.duration_ms ?? summary.durationMs ?? 0;
    document.getElementById('summary').textContent = `Total ${summary.total || 0} | Passed ${summary.passed || 0} | Failed ${summary.failed || 0} | Duration ${duration} ms`;

    const testsEl = document.getElementById('tests');
    const rows = (data.tests || []).map(t => {
      const statusClass = t.status === 'Pass' || t.status === 'pass' ? 'pass' : (t.status === 'Fail' || t.status === 'fail' ? 'fail' : 'pending');
      const dur = t.duration_ms ?? t.durationMs ?? 0;
      return `<tr><td>${t.name}</td><td><span class="badge ${statusClass}">${t.status}</span></td><td>${t.message}</td><td>${dur} ms</td></tr>`;
    }).join('');
    testsEl.innerHTML = '<tr><th>Test</th><th>Status</th><th>Message</th><th>Duration</th></tr>' + rows;
    appendDebug(`status ok: total=${summary.total || 0}, passed=${summary.passed || 0}, failed=${summary.failed || 0}, running=${running}`);
  } catch (e) {
    console.error('status error', e);
    appendDebug(`status error: ${e}`);
  }

  try {
    const data = await fetchJSON('/api/logs');
    const logsEl = document.getElementById('logs');
    logsEl.innerHTML = (data.logs || []).slice(-50).reverse().map(l => `<div class="log">[${l.timestamp}] [${l.level}] ${l.message}</div>`).join('');
    appendDebug(`logs ok: entries=${(data.logs || []).length}`);
  } catch (e) {
    console.error('log error', e);
    appendDebug(`logs error: ${e}`);
  }
}

async function runAll() {
  document.getElementById('runAll').disabled = true;
  appendDebug('run-all requested');
  try {
    await fetchJSON('/api/run-all', { method: 'POST' });
    appendDebug('run-all accepted');
  } catch (e) {
    appendDebug(`run-all error: ${e}`);
    console.error('run-all error', e);
  }
  setTimeout(refresh, 500);
}

document.getElementById('runAll').addEventListener('click', runAll);
setInterval(refresh, 1000);
refresh();
</script>
</body>
</html>"#;
