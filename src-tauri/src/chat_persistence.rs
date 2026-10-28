//! Saving chats so they survive a restart, a crash and a closed window.
//!
//! A chat is written (messages and model included) when the user sends, when the assistant
//! finishes or is cancelled, and on window close. The embedding that powers semantic search is
//! optional: when the embedding model is not ready the chat is saved with a placeholder vector
//! and filled in later by `backfill_embeddings`.

use crate::protocol::{ChatMessage, VectorMsg};
use fastembed::TextEmbedding;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{mpsc, oneshot, RwLock};

/// Upper bound on the text kept for search/preview; the embedder truncates far below this.
const MAX_SEARCH_CONTENT_CHARS: usize = 20_000;
/// How long window close waits for the writes to reach disk.
pub const FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

pub type SharedEmbeddingModel = Arc<RwLock<Option<Arc<TextEmbedding>>>>;

/// The system prompt is rebuilt every turn, so it is not part of the saved transcript.
pub fn persistable_messages(history: &[ChatMessage]) -> Vec<&ChatMessage> {
    history.iter().filter(|m| m.role != "system").collect()
}

pub fn messages_json(history: &[ChatMessage]) -> String {
    serde_json::to_string(&persistable_messages(history)).unwrap_or_else(|_| "[]".to_string())
}

/// Plain text of the conversation for search and the sidebar preview.
pub fn search_content(history: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in persistable_messages(history) {
        let label = match m.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };
        if m.content.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(label);
        out.push_str(": ");
        out.push_str(&m.content);
        if out.chars().count() >= MAX_SEARCH_CONTENT_CHARS {
            return out.chars().take(MAX_SEARCH_CONTENT_CHARS).collect();
        }
    }
    out
}

/// `history` with the finished (or cancelled) assistant reply appended, unless the loop
/// already recorded it or there is nothing to record.
pub fn with_final_assistant_reply(history: &[ChatMessage], reply: &str) -> Vec<ChatMessage> {
    let mut out = history.to_vec();
    let already_last = out
        .last()
        .map_or(false, |m| m.role == "assistant" && m.tool_calls.is_none() && m.content == reply);
    if !reply.trim().is_empty() && !already_last {
        out.push(ChatMessage {
            role: "assistant".to_string(),
            content: reply.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        });
    }
    out
}

pub fn build_upsert(
    chat_id: &str,
    title: &str,
    history: &[ChatMessage],
    model: &str,
    embedding: Option<Vec<f32>>,
    done: Option<oneshot::Sender<bool>>,
) -> VectorMsg {
    VectorMsg::UpsertChatRecord {
        id: chat_id.to_string(),
        title: title.to_string(),
        content: search_content(history),
        messages: messages_json(history),
        embedding_vector: embedding,
        pinned: None,
        model: (!model.is_empty()).then(|| model.to_string()),
        done,
    }
}

/// Save a chat and wait until the write has finished. Returns whether it succeeded.
pub async fn save_chat(
    vector_tx: &mpsc::Sender<VectorMsg>,
    chat_id: &str,
    title: &str,
    history: &[ChatMessage],
    model: &str,
    embedding: Option<Vec<f32>>,
) -> bool {
    let (tx, rx) = oneshot::channel();
    if vector_tx
        .send(build_upsert(chat_id, title, history, model, embedding, Some(tx)))
        .await
        .is_err()
    {
        println!("[ChatPersistence] vector actor is gone; chat {chat_id} not saved");
        return false;
    }
    rx.await.unwrap_or(false)
}

/// Embed `text` with the CPU model if it is loaded.
pub async fn embed_text(model: &SharedEmbeddingModel, text: String) -> Option<Vec<f32>> {
    let loaded = model.read().await.clone()?;
    tokio::task::spawn_blocking(move || loaded.embed(vec![text], None).ok()?.into_iter().next())
        .await
        .ok()
        .flatten()
}

/// How often the background loop looks for chats still waiting for an embedding.
pub const BACKFILL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

static BACKFILL_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Embed each pending chat in turn and hand the vector to `store`. Stops at the first chat that
/// cannot be embedded (the model went away) and reports how many were stored; the next pass
/// picks up the rest.
async fn backfill_pending<E, EF, S, SF>(pending: Vec<(String, String)>, mut embed: E, mut store: S) -> usize
where
    E: FnMut(String) -> EF,
    EF: std::future::Future<Output = Option<Vec<f32>>>,
    S: FnMut(String, Vec<f32>) -> SF,
    SF: std::future::Future<Output = ()>,
{
    let mut stored = 0;
    for (id, content) in pending {
        match embed(content).await {
            Some(vector) => {
                store(id, vector).await;
                stored += 1;
            }
            None => break,
        }
    }
    stored
}

/// Fill in embeddings for chats that were saved while the embedding model was unavailable.
/// Safe to call from several places (recovery, timer): overlapping calls do nothing.
pub async fn backfill_embeddings(vector_tx: &mpsc::Sender<VectorMsg>, model: &SharedEmbeddingModel) {
    if BACKFILL_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    backfill_embeddings_once(vector_tx, model).await;
    BACKFILL_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
}

async fn backfill_embeddings_once(vector_tx: &mpsc::Sender<VectorMsg>, model: &SharedEmbeddingModel) {
    let (tx, rx) = oneshot::channel();
    if vector_tx
        .send(VectorMsg::ListChatsMissingEmbedding { respond_to: tx })
        .await
        .is_err()
    {
        return;
    }
    let Ok(pending) = rx.await else { return };
    if pending.is_empty() {
        return;
    }
    let total = pending.len();
    println!("[ChatPersistence] Backfilling embeddings for {total} chat(s)");
    let stored = backfill_pending(
        pending,
        |content| embed_text(model, content),
        |id, vector| async move {
            let _ = vector_tx.send(VectorMsg::SetChatEmbedding { id, vector }).await;
        },
    )
    .await;
    if stored < total {
        println!("[ChatPersistence] embedder unavailable after {stored} of {total}; will retry");
    }
}

/// Keep trying to fill missing embeddings for the life of the app: a failed first pass, a model
/// that arrives late (download finished, retry succeeded) and chats saved meanwhile are all
/// picked up within `BACKFILL_INTERVAL`, without waiting for a restart.
pub fn spawn_backfill_loop(app: tauri::AppHandle, model: SharedEmbeddingModel) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(BACKFILL_INTERVAL).await;
            if model.read().await.is_none() {
                continue;
            }
            let Some(handles) = app.try_state::<crate::app_state::ActorHandles>() else { continue };
            let vector_tx = handles.vector_tx.clone();
            backfill_embeddings(&vector_tx, &model).await;
        }
    });
}

// ---------------------------------------------------------------------------
// In-flight chats: what window close has to save
// ---------------------------------------------------------------------------

/// A chat whose turn is running. The agentic loop keeps this current so that closing the
/// window mid-turn still saves everything said so far.
#[derive(Debug, Clone)]
pub struct InflightChat {
    pub title: String,
    pub model: String,
    pub history: Vec<ChatMessage>,
}

fn inflight() -> &'static Mutex<HashMap<String, InflightChat>> {
    static INFLIGHT: OnceLock<Mutex<HashMap<String, InflightChat>>> = OnceLock::new();
    INFLIGHT.get_or_init(Default::default)
}

pub fn track_inflight(chat_id: &str, title: &str, model: &str, history: &[ChatMessage]) {
    if let Ok(mut map) = inflight().lock() {
        map.insert(
            chat_id.to_string(),
            InflightChat {
                title: title.to_string(),
                model: model.to_string(),
                history: history.to_vec(),
            },
        );
    }
}

pub fn untrack_inflight(chat_id: &str) {
    if let Ok(mut map) = inflight().lock() {
        map.remove(chat_id);
    }
}

pub fn inflight_snapshot() -> Vec<(String, InflightChat)> {
    inflight()
        .lock()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// Save every running chat (no embedding: shutdown must be quick). Waits up to `timeout`.
pub async fn flush_inflight(vector_tx: &mpsc::Sender<VectorMsg>, timeout: std::time::Duration) {
    let chats = inflight_snapshot();
    if chats.is_empty() {
        return;
    }
    let saves = chats
        .iter()
        .map(|(id, c)| save_chat(vector_tx, id, &c.title, &c.history, &c.model, None));
    let all = futures::future::join_all(saves);
    if tokio::time::timeout(timeout, all).await.is_err() {
        println!("[ChatPersistence] timed out saving running chats at shutdown");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc as TestArc, Mutex as TestMutex};

    #[tokio::test]
    async fn backfill_stops_at_first_failure_and_a_later_pass_finishes() {
        let pending = vec![("a".to_string(), "x".to_string()), ("b".to_string(), "y".to_string()), ("c".to_string(), "z".to_string())];
        let stored_ids = TestArc::new(TestMutex::new(Vec::<String>::new()));
        let ids = stored_ids.clone();
        // First pass: the model disappears after one chat.
        let mut calls = 0;
        let n = backfill_pending(
            pending.clone(),
            |_| {
                calls += 1;
                let ok = calls <= 1;
                async move { ok.then(|| vec![1.0]) }
            },
            |id, _| {
                let ids = ids.clone();
                async move { ids.lock().unwrap().push(id) }
            },
        )
        .await;
        assert_eq!(n, 1);
        // Second pass (timer / recovery): everything still missing is stored.
        let ids = stored_ids.clone();
        let n = backfill_pending(
            pending[1..].to_vec(),
            |_| async { Some(vec![1.0]) },
            |id, _| {
                let ids = ids.clone();
                async move { ids.lock().unwrap().push(id) }
            },
        )
        .await;
        assert_eq!(n, 2);
        assert_eq!(*stored_ids.lock().unwrap(), vec!["a", "b", "c"]);
    }

    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn system_prompt_is_not_saved_but_everything_else_is() {
        let h = vec![msg("system", "SECRET"), msg("user", "hi"), msg("assistant", "hello")];
        let json = messages_json(&h);
        assert!(!json.contains("SECRET"));
        let back: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0]["role"], "user");
        assert_eq!(back[1]["content"], "hello");
    }

    #[test]
    fn saved_messages_round_trip_into_chat_messages() {
        let h = vec![msg("user", "q"), msg("assistant", "a")];
        let back: Vec<ChatMessage> = serde_json::from_str(&messages_json(&h)).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].content, "a");
    }

    #[test]
    fn search_content_labels_turns_and_skips_tool_chatter() {
        let h = vec![msg("system", "s"), msg("user", "q"), msg("tool", "raw"), msg("assistant", "a")];
        assert_eq!(search_content(&h), "User: q\n\nAssistant: a");
    }

    #[test]
    fn search_content_is_bounded_on_a_char_boundary() {
        let long = "é".repeat(MAX_SEARCH_CONTENT_CHARS * 2);
        let c = search_content(&[msg("user", &long)]);
        assert_eq!(c.chars().count(), MAX_SEARCH_CONTENT_CHARS);
    }

    #[test]
    fn final_reply_is_appended_once() {
        let h = vec![msg("user", "q")];
        let once = with_final_assistant_reply(&h, "a");
        assert_eq!(once.len(), 2);
        let twice = with_final_assistant_reply(&once, "a");
        assert_eq!(twice.len(), 2);
    }

    #[test]
    fn an_empty_reply_adds_nothing() {
        let h = vec![msg("user", "q")];
        assert_eq!(with_final_assistant_reply(&h, "  ").len(), 1);
    }

    #[test]
    fn upsert_carries_messages_model_and_no_pin() {
        let h = vec![msg("user", "q")];
        match build_upsert("c", "T", &h, "phi-4", None, None) {
            VectorMsg::UpsertChatRecord { id, messages, model, pinned, embedding_vector, content, .. } => {
                assert_eq!(id, "c");
                assert!(messages.contains("\"q\""));
                assert_eq!(model.as_deref(), Some("phi-4"));
                assert!(pinned.is_none() && embedding_vector.is_none());
                assert_eq!(content, "User: q");
            }
            _ => panic!("wrong message"),
        }
    }

    #[test]
    fn a_checkpoint_taken_after_a_tool_round_keeps_the_call_and_its_result() {
        // What the agentic loop saves between rounds: no final reply yet, no embedding.
        let h = vec![
            msg("system", "SECRET"),
            msg("user", "how many rows?"),
            msg("assistant", "<tool_call>{\"name\":\"count\"}</tool_call>"),
            msg("user", "<tool_response>\n3\n</tool_response>"),
        ];
        match build_upsert("c", "T", &h, "phi-4", None, None) {
            VectorMsg::UpsertChatRecord { messages, embedding_vector, .. } => {
                assert!(embedding_vector.is_none());
                assert!(messages.contains("count") && messages.contains("<tool_response>"));
                assert!(!messages.contains("SECRET"));
            }
            _ => panic!("wrong message"),
        }
    }

    #[test]
    fn a_turn_cut_off_by_a_network_drop_saves_the_words_and_every_tool_result_in_the_shape_the_screen_reads() {
        use crate::protocol::{ParsedToolCall, ToolFormat};
        let call = |tool: &str| ParsedToolCall { server: "box".into(), tool: tool.into(), arguments: serde_json::json!({}), raw: String::new(), id: None };
        let done = crate::tool_parsing::format_tool_result(&call("count_rows"), "{\"rows\":3}", false, ToolFormat::Hermes, None, None);
        let cut = crate::tool_parsing::format_tool_result(&call("slow_lookup"), crate::turn_failure::INTERRUPTED_TOOL_RESULT, true, ToolFormat::Hermes, None, None);
        let history = vec![
            msg("system", "SECRET"),
            msg("user", "how many rows, and what about the lookup?"),
            msg("assistant", "<tool_call>{\"name\":\"count_rows\"}</tool_call>"),
            msg("user", &done),
            msg("assistant", "<tool_call>{\"name\":\"slow_lookup\"}</tool_call>"),
            msg("user", &cut),
        ];
        let saved = crate::turn_failure::history_after_failed_turn(&history, "There are 3 rows. For the lookup I was about to <tool_call>{\"na");
        let json = messages_json(&saved);
        let back: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 6, "system prompt dropped, nothing else lost: {json}");
        assert_eq!(back[5]["role"], "assistant");
        assert_eq!(back[5]["content"], "There are 3 rows. For the lookup I was about to");
        assert!(back[4]["content"].as_str().unwrap().contains("Interrupted"));
        assert!(!json.contains("SECRET"));
        assert!(!json.contains("RETRY REQUIRED"), "an interrupted call must not tell the model to retry at once");
        println!("FAILED_TURN_SAVED_JSON={json}");
    }

    #[test]
    fn an_empty_model_name_keeps_the_stored_model() {
        match build_upsert("c", "T", &[], "", None, None) {
            VectorMsg::UpsertChatRecord { model, .. } => assert!(model.is_none()),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn save_chat_waits_for_the_actor_and_reports_the_result() {
        let (tx, mut rx) = mpsc::channel(4);
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if let VectorMsg::UpsertChatRecord { done: Some(d), .. } = m {
                    let _ = d.send(true);
                }
            }
        });
        assert!(save_chat(&tx, "c", "T", &[msg("user", "q")], "m", None).await);
    }

    #[tokio::test]
    async fn save_chat_is_false_when_the_actor_is_gone() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        assert!(!save_chat(&tx, "c", "T", &[], "m", None).await);
    }

    #[tokio::test]
    async fn flush_saves_every_tracked_chat_then_untrack_forgets_it() {
        let (tx, mut rx) = mpsc::channel(8);
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if let VectorMsg::UpsertChatRecord { id, done, .. } = m {
                    sink.lock().unwrap().push(id);
                    if let Some(d) = done {
                        let _ = d.send(true);
                    }
                }
            }
        });
        track_inflight("flush-a", "A", "m", &[msg("user", "q")]);
        track_inflight("flush-b", "B", "m", &[msg("user", "q2")]);
        flush_inflight(&tx, std::time::Duration::from_secs(2)).await;
        {
            let seen = seen.lock().unwrap();
            assert!(seen.contains(&"flush-a".to_string()) && seen.contains(&"flush-b".to_string()));
        }
        untrack_inflight("flush-a");
        untrack_inflight("flush-b");
        assert!(!inflight_snapshot().iter().any(|(id, _)| id.starts_with("flush-")));
    }
}
