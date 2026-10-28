//! `--dump-chats <file>`: read every saved chat back out of the chat store and write it as JSON, then
//! exit. The soak test runs it after the app was killed, to show the store still opens and holds what
//! was saved; it reads through the same vector actor the app uses, so it shows what a reload would show.

use std::path::PathBuf;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::protocol::VectorMsg;

/// One chat as written to the dump. `messages_valid` is false when the saved text is not a JSON
/// array of messages, which the screen would show as a chat it cannot open.
pub fn dump_entry(id: &str, title: &str, saved_messages: Option<&str>) -> Value {
    let parsed = saved_messages.and_then(|t| serde_json::from_str::<Value>(t).ok()).filter(Value::is_array);
    json!({
        "id": id,
        "title": title,
        "messages_valid": parsed.is_some(),
        "messages": parsed.unwrap_or(Value::Array(vec![])),
    })
}

async fn read_all(vector_tx: &tokio::sync::mpsc::Sender<VectorMsg>) -> Result<Vec<Value>, String> {
    let (tx, rx) = oneshot::channel();
    vector_tx.send(VectorMsg::FetchAllChats { respond_to: tx }).await.map_err(|e| e.to_string())?;
    let chats = rx.await.map_err(|_| "the chat store did not answer".to_string())?;
    let mut out = Vec::new();
    for c in chats {
        let (tx, rx) = oneshot::channel();
        vector_tx.send(VectorMsg::FetchChatMessages { id: c.id.clone(), respond_to: tx }).await.map_err(|e| e.to_string())?;
        let saved = rx.await.map_err(|_| "the chat store did not answer".to_string())?;
        out.push(dump_entry(&c.id, &c.title, saved.as_deref()));
    }
    Ok(out)
}

/// Writes the dump to `path` and ends the process: 0 when written, 1 when the store could not be read.
pub fn spawn(app: tauri::AppHandle, vector_tx: tokio::sync::mpsc::Sender<VectorMsg>, path: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let code = match read_all(&vector_tx).await {
            Ok(chats) => match std::fs::write(&path, serde_json::to_string_pretty(&chats).unwrap_or_else(|_| "[]".into())) {
                Ok(()) => {
                    println!("[ChatDump] wrote {} chat(s) to {}", chats.len(), path.display());
                    0
                }
                Err(e) => {
                    println!("[ChatDump] could not write {}: {e}", path.display());
                    1
                }
            },
            Err(e) => {
                println!("[ChatDump] could not read the chat store: {e}");
                1
            }
        };
        crate::turn_record::exit_app(&app, code);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_chat_is_dumped_with_its_messages_and_a_damaged_one_is_marked_invalid() {
        let ok = dump_entry("a", "T", Some(r#"[{"role":"user","content":"q"}]"#));
        assert_eq!(ok["messages_valid"], true);
        assert_eq!(ok["messages"][0]["content"], "q");
        for bad in [Some("{nope"), Some(r#"{"role":"user"}"#), None] {
            let e = dump_entry("b", "T", bad);
            assert_eq!(e["messages_valid"], false, "{bad:?}");
            assert_eq!(e["messages"], json!([]));
        }
    }

    #[tokio::test]
    async fn every_chat_in_the_store_is_read_back_by_id() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                match m {
                    VectorMsg::FetchAllChats { respond_to } => {
                        let c = |id: &str| crate::protocol::ChatSummary { id: id.into(), title: format!("t-{id}"), preview: String::new(), score: 0.0, pinned: false, model: None };
                        let _ = respond_to.send(vec![c("one"), c("two")]);
                    }
                    VectorMsg::FetchChatMessages { id, respond_to } => {
                        let _ = respond_to.send(Some(format!(r#"[{{"role":"user","content":"from {id}"}}]"#)));
                    }
                    _ => {}
                }
            }
        });
        let all = read_all(&tx).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["messages"][0]["content"], "from two");
        assert_eq!(all[0]["title"], "t-one");
    }
}
