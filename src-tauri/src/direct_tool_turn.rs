//! A turn answered by an MCP tool directly: the message is the tool's one text field and no local
//! model is involved. The turn is saved like any other (the question, then the tool's text as the
//! assistant reply), so history and reload show it; each message is an independent stateless call.
//!
//! Free of Tauri types so every branch is unit-testable; `commands::mcp::call_mcp_tool_directly` adds the events.

use crate::chat_persistence::{self, SharedEmbeddingModel};
use crate::protocol::{ChatMessage, McpHostMsg, VectorMsg};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

/// What to call and what to say to it.
#[derive(Debug, Clone)]
pub struct DirectToolRequest {
    pub chat_id: String,
    pub title: String,
    pub message: String,
    /// The conversation so far, from the screen (no system prompt is ever saved).
    pub history: Vec<ChatMessage>,
    pub server_id: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectToolEnd {
    /// The tool's text, shown as the assistant reply.
    Replied(String),
    /// Why there is no reply: the tool's own error text, a timeout, a server that went away.
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DirectToolTurn {
    pub end: DirectToolEnd,
    /// `server - tool`, as the drop-down shows it (the request's names when the tool was no longer offered).
    pub label: String,
    pub arguments: Value,
    pub duration: Duration,
    /// Whether the finished chat reached the store.
    pub saved: bool,
}

/// The arguments the tool is called with: the message under the tool's one field.
pub fn direct_tool_arguments(field_name: &str, message: &str) -> Value {
    json!({ field_name: message })
}

fn chat_message(role: &str, content: &str) -> ChatMessage {
    ChatMessage { role: role.to_string(), content: content.to_string(), system_prompt: None, tool_calls: None, tool_call_id: None }
}

async fn find_offered_tool(mcp_host_tx: &mpsc::Sender<McpHostMsg>, server_id: &str, tool_name: &str) -> Option<crate::mcp_direct_tools::DirectTool> {
    let (tx, rx) = oneshot::channel();
    mcp_host_tx.send(McpHostMsg::ListDirectTools { respond_to: tx }).await.ok()?;
    rx.await.ok()?.into_iter().find(|t| t.server_id == server_id && t.tool_name == tool_name)
}

/// One call through the MCP host. The server's own `request_timeout_secs` ends a call that hangs.
async fn call_tool(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    server_id: &str,
    tool_name: &str,
    arguments: Value,
    mut cancel: Option<watch::Receiver<bool>>,
) -> DirectToolEnd {
    let (tx, rx) = oneshot::channel();
    let request = McpHostMsg::ExecuteTool { server_id: server_id.to_string(), tool_name: tool_name.to_string(), arguments, respond_to: tx };
    if mcp_host_tx.send(request).await.is_err() {
        return DirectToolEnd::Failed("The tool host is not running.".to_string());
    }
    let answer = async {
        match rx.await {
            Ok(result) => result,
            Err(_) => Err("The tool host stopped before it answered.".to_string()),
        }
    };
    let result = match cancel.as_mut() {
        Some(cancel) => tokio::select! {
            result = answer => result,
            _ = cancel.wait_for(|stopped| *stopped) => return DirectToolEnd::Cancelled,
        },
        None => answer.await,
    };
    match result {
        Err(error) => DirectToolEnd::Failed(error),
        Ok(result) => {
            let text = crate::tool_execution::mcp_result_text(&result);
            if result.is_error {
                DirectToolEnd::Failed(if text.trim().is_empty() { "The tool reported an error.".to_string() } else { text })
            } else if text.trim().is_empty() {
                DirectToolEnd::Failed("The tool answered with no text.".to_string())
            } else {
                DirectToolEnd::Replied(text)
            }
        }
    }
}

/// Run one directly-called turn: save the question, call the tool, save the answer (or, when the call
/// failed, only the question: a failed turn never gets a made-up reply).
pub async fn run_direct_tool_turn(
    mcp_host_tx: &mpsc::Sender<McpHostMsg>,
    vector_tx: &mpsc::Sender<VectorMsg>,
    embedding_model: &SharedEmbeddingModel,
    request: &DirectToolRequest,
    cancel: Option<watch::Receiver<bool>>,
) -> DirectToolTurn {
    let started = Instant::now();
    let mut history = request.history.clone();
    history.push(chat_message("user", &request.message));
    // The question is on disk before a slow tool answers, so a crash or a closed window cannot lose it.
    // No model name: the chat keeps the one it has, and a tool is not a model the screen could load.
    chat_persistence::save_chat(vector_tx, &request.chat_id, &request.title, &history, "", None).await;

    let offered = find_offered_tool(mcp_host_tx, &request.server_id, &request.tool_name).await;
    let (label, arguments, end) = match offered {
        Some(tool) => {
            let arguments = direct_tool_arguments(&tool.field_name, &request.message);
            let end = call_tool(mcp_host_tx, &request.server_id, &request.tool_name, arguments.clone(), cancel).await;
            (tool.label, arguments, end)
        }
        None => (
            format!("{} - {}", request.server_id, request.tool_name),
            Value::Null,
            DirectToolEnd::Failed("That tool is no longer connected. Check Settings > Tools, or choose another entry in the drop-down.".to_string()),
        ),
    };

    if let DirectToolEnd::Replied(text) = &end {
        history.push(chat_message("assistant", text));
    }
    let embedding = chat_persistence::embed_text(embedding_model, chat_persistence::search_content(&history)).await;
    let saved = chat_persistence::save_chat(vector_tx, &request.chat_id, &request.title, &history, "", embedding).await;
    DirectToolTurn { end, label, arguments, duration: started.elapsed(), saved }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actors::mcp_host_actor::McpToolRouterActor;
    use crate::settings::{McpServerConfig, Transport};
    use std::sync::{Arc, Mutex};
    use tokio::sync::RwLock;

    /// A stand-in for the chat store that remembers every save of a chat.
    fn fake_vector_store() -> (mpsc::Sender<VectorMsg>, Arc<Mutex<Vec<(String, String, Option<String>)>>>) {
        let (tx, mut rx) = mpsc::channel(8);
        let saves = Arc::new(Mutex::new(Vec::new()));
        let sink = saves.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let VectorMsg::UpsertChatRecord { id, messages, model, done, .. } = msg {
                    sink.lock().unwrap().push((id, messages, model));
                    if let Some(done) = done {
                        let _ = done.send(true);
                    }
                }
            }
        });
        (tx, saves)
    }

    fn no_embedding_model() -> SharedEmbeddingModel {
        Arc::new(RwLock::new(None))
    }

    async fn start_host() -> mpsc::Sender<McpHostMsg> {
        start_host_with(None).await
    }

    /// The host as the app runs it, optionally told to call a hook whenever the set of servers changes.
    async fn start_host_with(hook: Option<Arc<dyn Fn() + Send + Sync>>) -> mpsc::Sender<McpHostMsg> {
        let (tx, rx) = mpsc::channel(8);
        let mut actor = McpToolRouterActor::new(rx);
        if let Some(hook) = hook {
            actor = actor.with_servers_changed_hook(hook);
        }
        tokio::spawn(actor.run());
        tx
    }

    async fn connect(host: &mpsc::Sender<McpHostMsg>, config: McpServerConfig) {
        let (tx, rx) = oneshot::channel();
        host.send(McpHostMsg::ConnectServer { config, respond_to: tx }).await.unwrap();
        rx.await.unwrap().expect("the server connects");
    }

    fn http_config(id: &str, name: &str, url: &str, timeout_secs: Option<u64>) -> McpServerConfig {
        let mut config = McpServerConfig::new(id.to_string(), name.to_string());
        config.enabled = true;
        config.transport = Transport::Http { url: url.to_string() };
        config.request_timeout_secs = timeout_secs;
        config
    }

    async fn stub_url() -> String {
        let addr = mcp_test_server::spawn_mcp_only_server().await.expect("the stub server starts");
        format!("http://{}/mcp", addr)
    }

    fn request(server_id: &str, tool_name: &str, message: &str, history: Vec<ChatMessage>) -> DirectToolRequest {
        DirectToolRequest { chat_id: "chat-1".into(), title: "T".into(), message: message.into(), history, server_id: server_id.into(), tool_name: tool_name.into() }
    }

    async fn list_tools(host: &mpsc::Sender<McpHostMsg>) -> Vec<crate::mcp_direct_tools::DirectTool> {
        let (tx, rx) = oneshot::channel();
        host.send(McpHostMsg::ListDirectTools { respond_to: tx }).await.unwrap();
        rx.await.unwrap()
    }

    #[test]
    fn the_message_is_sent_as_the_tools_one_field() {
        assert_eq!(direct_tool_arguments("question", "How many orders?"), json!({"question": "How many orders?"}));
    }

    #[tokio::test]
    async fn a_connected_server_offers_its_single_text_tools_under_the_name_it_reports() {
        let host = start_host().await;
        connect(&host, http_config("stub", "Test box", &stub_url().await, None)).await;
        let labels: Vec<String> = list_tools(&host).await.into_iter().map(|t| t.label).collect();
        // The stub reports serverInfo.name "mcp-test-server"; its echo and error tools take one text field each,
        // while math (three fields), sleep (a number) and the status tools (none) do not qualify.
        assert_eq!(labels, vec!["mcp-test-server - test_echo", "mcp-test-server - test_error"]);
    }

    #[tokio::test]
    async fn the_servers_changing_is_announced_when_one_connects_and_when_it_disconnects() {
        let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = changes.clone();
        let host = start_host_with(Some(Arc::new(move || { counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }))).await;
        connect(&host, http_config("stub", "Test box", &stub_url().await, None)).await;
        assert_eq!(changes.load(std::sync::atomic::Ordering::SeqCst), 1);
        let (tx, rx) = oneshot::channel();
        host.send(McpHostMsg::DisconnectServer { server_id: "stub".into(), respond_to: tx }).await.unwrap();
        rx.await.unwrap().unwrap();
        assert_eq!(changes.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(list_tools(&host).await.is_empty(), "a disconnected server's tools are gone");
    }

    #[tokio::test]
    async fn a_direct_call_shows_the_tools_text_as_the_reply_and_saves_the_turn_without_a_model() {
        let host = start_host().await;
        connect(&host, http_config("stub", "Test box", &stub_url().await, None)).await;
        let (vector_tx, saves) = fake_vector_store();
        let earlier = vec![chat_message("user", "hello"), chat_message("assistant", "hi")];

        let turn = run_direct_tool_turn(&host, &vector_tx, &no_embedding_model(), &request("stub", "test_echo", "how many orders?", earlier), None).await;

        assert_eq!(turn.end, DirectToolEnd::Replied("Echo: how many orders?".to_string()));
        assert_eq!(turn.label, "mcp-test-server - test_echo");
        assert_eq!(turn.arguments, json!({"message": "how many orders?"}));
        assert!(turn.saved);
        let saves = saves.lock().unwrap();
        assert_eq!(saves.len(), 2, "the question is saved first, the finished turn after");
        let (id, messages, model) = saves.last().unwrap();
        assert_eq!(id, "chat-1");
        assert_eq!(model, &None, "no model is recorded for a direct call, so the chat keeps its own");
        let saved: Vec<Value> = serde_json::from_str(messages).unwrap();
        let roles_and_text: Vec<(String, String)> = saved.iter().map(|m| (m["role"].as_str().unwrap().into(), m["content"].as_str().unwrap().into())).collect();
        assert_eq!(
            roles_and_text,
            vec![
                ("user".into(), "hello".into()),
                ("assistant".into(), "hi".into()),
                ("user".into(), "how many orders?".into()),
                ("assistant".into(), "Echo: how many orders?".into()),
            ]
        );
        let first_save: Vec<Value> = serde_json::from_str(&saves[0].1).unwrap();
        assert_eq!(first_save.last().unwrap()["content"], "how many orders?", "the first save ends with the question and no reply");
    }

    #[tokio::test]
    async fn a_tool_that_reports_an_error_is_a_failed_turn_that_keeps_the_question_and_makes_up_no_reply() {
        let host = start_host().await;
        connect(&host, http_config("stub", "Test box", &stub_url().await, None)).await;
        let (vector_tx, saves) = fake_vector_store();

        let turn = run_direct_tool_turn(&host, &vector_tx, &no_embedding_model(), &request("stub", "test_error", "runtime", vec![]), None).await;

        let DirectToolEnd::Failed(why) = &turn.end else { panic!("expected a failure, got {:?}", turn.end) };
        assert!(!why.trim().is_empty());
        let saves = saves.lock().unwrap();
        let (_, messages, _) = saves.last().unwrap();
        let saved: Vec<Value> = serde_json::from_str(messages).unwrap();
        assert_eq!(saved.len(), 1, "only the question is saved: {messages}");
        assert_eq!(saved[0]["role"], "user");
    }

    #[tokio::test]
    async fn a_server_that_is_gone_fails_the_turn_and_says_where_to_look() {
        let host = start_host().await;
        let (vector_tx, _saves) = fake_vector_store();
        let turn = run_direct_tool_turn(&host, &vector_tx, &no_embedding_model(), &request("nope", "ask", "hello", vec![]), None).await;
        let DirectToolEnd::Failed(why) = turn.end else { panic!("expected a failure") };
        assert!(why.contains("no longer connected") && why.contains("Settings > Tools"), "{why}");
    }

    #[tokio::test]
    async fn stopping_while_the_tool_runs_ends_the_turn_as_cancelled_with_the_question_saved() {
        let host = start_host().await;
        connect(&host, http_config("stub", "Test box", &stub_url().await, None)).await;
        let (vector_tx, saves) = fake_vector_store();
        let (stop_tx, stop_rx) = watch::channel(true); // already stopped: the select must not wait for the tool
        let turn = run_direct_tool_turn(&host, &vector_tx, &no_embedding_model(), &request("stub", "test_echo", "hello", vec![]), Some(stop_rx)).await;
        drop(stop_tx);
        assert_eq!(turn.end, DirectToolEnd::Cancelled);
        assert_eq!(saves.lock().unwrap().len(), 2);
    }

    /// A stdio server named like the real one: `ask` takes `question` and answers after a pause when
    /// the question starts with "slow:" followed by the seconds.
    #[cfg(unix)]
    const ASK_SERVER: &str = r#"
import json, sys, time
for line in sys.stdin:
    msg = json.loads(line)
    method, mid = msg.get("method"), msg.get("id")
    if mid is None:
        continue
    if method == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "plugableiq", "version": "0"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "ask", "description": "Ask PlugableIQ a business question.", "inputSchema": {"type": "object", "properties": {"question": {"type": "string"}}, "required": ["question"]}}]}
    elif method == "tools/call":
        q = msg["params"]["arguments"]["question"]
        if q.startswith("slow:"):
            time.sleep(float(q[5:]))
        result = {"content": [{"type": "text", "text": "Answer to: " + q}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": mid, "result": result}), flush=True)
"#;

    #[cfg(unix)]
    async fn connect_ask_server(host: &mpsc::Sender<McpHostMsg>, timeout_secs: Option<u64>) -> bool {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            eprintln!("python3 is not available; skipping the stdio direct-call test");
            return false;
        }
        let mut config = McpServerConfig::new("iq".to_string(), "My IQ box".to_string());
        config.enabled = true;
        config.command = Some("python3".to_string());
        config.args = vec!["-u".to_string(), "-c".to_string(), ASK_SERVER.to_string()];
        config.request_timeout_secs = timeout_secs;
        connect(host, config).await;
        true
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stdio_server_is_listed_as_plugableiq_dash_ask_and_answers_a_direct_question() {
        let host = start_host().await;
        if !connect_ask_server(&host, Some(10)).await { return }
        let tools = list_tools(&host).await;
        assert_eq!(tools.len(), 1);
        assert_eq!((tools[0].label.as_str(), tools[0].field_name.as_str(), tools[0].server_id.as_str()), ("plugableiq - ask", "question", "iq"));
        let (vector_tx, _saves) = fake_vector_store();
        let turn = run_direct_tool_turn(&host, &vector_tx, &no_embedding_model(), &request("iq", "ask", "how many orders?", vec![]), None).await;
        assert_eq!(turn.end, DirectToolEnd::Replied("Answer to: how many orders?".to_string()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_direct_call_that_outlasts_the_servers_request_timeout_is_a_failed_turn_that_names_the_setting() {
        let host = start_host().await;
        if !connect_ask_server(&host, Some(1)).await { return }
        let (vector_tx, saves) = fake_vector_store();
        let turn = run_direct_tool_turn(&host, &vector_tx, &no_embedding_model(), &request("iq", "ask", "slow:3", vec![]), None).await;
        let DirectToolEnd::Failed(why) = &turn.end else { panic!("expected a timeout, got {:?}", turn.end) };
        assert!(why.contains("timed out after 1s") && why.contains("request_timeout_secs"), "{why}");
        let saved: Vec<Value> = serde_json::from_str(&saves.lock().unwrap().last().unwrap().1).unwrap();
        assert_eq!(saved.len(), 1, "the question stays; no reply is invented");

        // A longer limit lets the same call finish.
        let host2 = start_host_with(None).await;
        if !connect_ask_server(&host2, Some(10)).await { return }
        let ok = run_direct_tool_turn(&host2, &vector_tx, &no_embedding_model(), &request("iq", "ask", "slow:2", vec![]), None).await;
        assert_eq!(ok.end, DirectToolEnd::Replied("Answer to: slow:2".to_string()));
    }
}
