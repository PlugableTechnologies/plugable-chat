use crate::protocol::{ChatSummary, VectorMsg};
use arrow_array::types::Float32Type;
use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::StreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, Table};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Embedding width of the chat table's `vector` column (BGE-Base-EN-v1.5).
const CHAT_VECTOR_DIM: usize = 768;

/// Receives user-facing storage problems (the app turns these into an event for the UI).
pub type StorageErrorReporter = Arc<dyn Fn(String) + Send + Sync>;

/// One row of the `chats` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRow {
    pub id: String,
    pub title: String,
    pub content: String,
    pub messages: String,
    pub pinned: bool,
    pub model: Option<String>,
    pub vector: Vec<f32>,
}

/// A chat saved before an embedding was available carries an all-zero vector; the backfill
/// pass replaces it once the embedding model is ready.
pub fn is_placeholder_vector(v: &[f32]) -> bool {
    v.iter().all(|x| *x == 0.0)
}

pub struct ChatVectorStoreActor {
    vector_msg_rx: mpsc::Receiver<VectorMsg>,
    /// `None` when no storage location could be opened at all; the actor then answers every
    /// request with an empty result instead of dying, and the user has already been told.
    chat_table: Option<Table>,
}

impl ChatVectorStoreActor {
    pub async fn new(
        vector_msg_rx: mpsc::Receiver<VectorMsg>,
        db_path: &str,
        report_error: StorageErrorReporter,
    ) -> Self {
        let chat_table = open_chats_table(db_path, &report_error).await;
        Self {
            vector_msg_rx,
            chat_table,
        }
    }

    pub async fn run(mut self) {
        while let Some(msg) = self.vector_msg_rx.recv().await {
            let Some(chat_table) = self.chat_table.clone() else {
                reply_unavailable(msg);
                continue;
            };

            // Writes run inline, in arrival order: an upsert is delete-then-add, so two
            // concurrent upserts of one chat would leave duplicate rows. Saving on every send
            // and every completion makes that overlap likely.
            if is_write(&msg) {
                handle_write(&chat_table, msg).await;
                continue;
            }

            // Reads are spawned so a slow query never clogs the mailbox.
            tokio::spawn(async move {
                match msg {
                    VectorMsg::SearchChatsByEmbedding {
                        query_vector,
                        limit,
                        respond_to,
                    } => {
                        let search_results =
                            search_chats_by_embedding(chat_table, query_vector, limit).await;
                        let _ = respond_to.send(search_results);
                    }
                    VectorMsg::FetchAllChats { respond_to } => {
                        let zero_embedding_vector = vec![0.0; CHAT_VECTOR_DIM];
                        let search_results =
                            search_chats_by_embedding(chat_table, zero_embedding_vector, 100)
                                .await;
                        let _ = respond_to.send(search_results);
                    }
                    VectorMsg::FetchChatMessages { id, respond_to } => {
                        let chat_messages_json = fetch_chat_messages_json(chat_table, id).await;
                        let _ = respond_to.send(chat_messages_json);
                    }
                    VectorMsg::ListChatsMissingEmbedding { respond_to } => {
                        let _ = respond_to.send(list_chats_missing_embedding(&chat_table).await);
                    }
                    _ => {}
                }
            });
        }
    }
}

fn is_write(msg: &VectorMsg) -> bool {
    matches!(
        msg,
        VectorMsg::UpsertChatRecord { .. }
            | VectorMsg::UpdateChatTitleAndPin { .. }
            | VectorMsg::DeleteChatById { .. }
            | VectorMsg::SetChatEmbedding { .. }
    )
}

/// Answer a request when there is no table, so callers never hang on a dropped sender.
fn reply_unavailable(msg: VectorMsg) {
    println!("VectorActor ERROR: chat storage unavailable; request dropped");
    match msg {
        VectorMsg::SearchChatsByEmbedding { respond_to, .. } | VectorMsg::FetchAllChats { respond_to } => {
            let _ = respond_to.send(Vec::new());
        }
        VectorMsg::FetchChatMessages { respond_to, .. } => {
            let _ = respond_to.send(None);
        }
        VectorMsg::UpdateChatTitleAndPin { respond_to, .. } | VectorMsg::DeleteChatById { respond_to, .. } => {
            let _ = respond_to.send(false);
        }
        VectorMsg::UpsertChatRecord { done, .. } => {
            if let Some(done) = done {
                let _ = done.send(false);
            }
        }
        VectorMsg::ListChatsMissingEmbedding { respond_to } => {
            let _ = respond_to.send(Vec::new());
        }
        VectorMsg::SetChatEmbedding { .. } => {}
    }
}

async fn handle_write(chat_table: &Table, msg: VectorMsg) {
    match msg {
        VectorMsg::UpsertChatRecord {
            id,
            title,
            content,
            messages,
            embedding_vector,
            pinned,
            model,
            done,
        } => {
            let existing = fetch_full_chat_record(chat_table.clone(), id.clone()).await;
            let row = merge_upsert(existing, id, title, content, messages, embedding_vector, pinned, model);
            let ok = upsert_chat_rows(chat_table, std::slice::from_ref(&row)).await;
            if let Some(done) = done {
                let _ = done.send(ok);
            }
        }
        VectorMsg::SetChatEmbedding { id, vector } => {
            if vector.len() != CHAT_VECTOR_DIM {
                println!("VectorActor WARNING: ignoring embedding of wrong size {}", vector.len());
                return;
            }
            if let Some(mut row) = fetch_full_chat_record(chat_table.clone(), id).await {
                row.vector = vector;
                upsert_chat_rows(chat_table, std::slice::from_ref(&row)).await;
            }
        }
        VectorMsg::UpdateChatTitleAndPin {
            id,
            title,
            pinned,
            respond_to,
        } => {
            println!(
                "VectorActor: Updating metadata (id: {}, title: {:?}, pinned: {:?})",
                &id[..8.min(id.len())],
                title,
                pinned
            );
            if let Some(mut row) = fetch_full_chat_record(chat_table.clone(), id.clone()).await {
                row.title = title.unwrap_or(row.title);
                row.pinned = pinned.unwrap_or(row.pinned);
                let ok = upsert_chat_rows(chat_table, std::slice::from_ref(&row)).await;
                let _ = respond_to.send(ok);
            } else {
                println!(
                    "VectorActor ERROR: Chat {} not found for metadata update",
                    &id[..8.min(id.len())]
                );
                let _ = respond_to.send(false);
            }
        }
        VectorMsg::DeleteChatById { id, respond_to } => {
            println!("VectorActor: Deleting chat (id: {})", id);
            match chat_table.delete(&format!("id = '{}'", escape_sql(&id))).await {
                Ok(_) => {
                    println!("VectorActor: Successfully deleted chat {}", id);
                    let _ = respond_to.send(true);
                }
                Err(e) => {
                    println!("VectorActor ERROR: Failed to delete chat {}: {}", id, e);
                    let _ = respond_to.send(false);
                }
            }
        }
        _ => {}
    }
}

/// Build the row to write: unspecified fields keep what the stored chat already has, so a save
/// that has no embedding (or does not know the pin state) never wipes either.
#[allow(clippy::too_many_arguments)]
fn merge_upsert(
    existing: Option<ChatRow>,
    id: String,
    title: String,
    content: String,
    messages: String,
    embedding_vector: Option<Vec<f32>>,
    pinned: Option<bool>,
    model: Option<String>,
) -> ChatRow {
    let (old_pinned, old_model, old_vector) = match existing {
        Some(e) => (e.pinned, e.model, Some(e.vector)),
        None => (false, None, None),
    };
    let vector = embedding_vector
        .filter(|v| v.len() == CHAT_VECTOR_DIM)
        .or_else(|| old_vector.filter(|v| v.len() == CHAT_VECTOR_DIM))
        .unwrap_or_else(|| vec![0.0; CHAT_VECTOR_DIM]);
    ChatRow {
        id,
        title,
        content,
        messages,
        pinned: pinned.unwrap_or(old_pinned),
        model: model.or(old_model),
        vector,
    }
}

/// Chat ids are UUIDs, but an id arrives from the frontend, so keep a quote from ending the filter.
fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

async fn search_chats_by_embedding(
    chat_table: Table,
    embedding_vector: Vec<f32>,
    limit: usize,
) -> Vec<ChatSummary> {
    // LanceDB Async Query - results are automatically sorted by similarity (closest first)
    let embedding_query = chat_table.query().nearest_to(embedding_vector); // Vector search

    let query = match embedding_query {
        Ok(q) => q,
        Err(e) => {
            println!("VectorActor ERROR: Failed to create vector query: {}", e);
            return vec![];
        }
    };

    let query_stream = query.limit(limit).execute().await;

    let mut search_results = Vec::new();

    if let Ok(mut query_stream) = query_stream {
        while let Some(batch) = query_stream.next().await {
            if let Ok(batch) = batch {
                let ids = batch
                    .column_by_name("id")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                let titles = batch
                    .column_by_name("title")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                let contents = batch
                    .column_by_name("content")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();

                // Handle optional pinned column for backward compatibility
                let pinned_col = batch.column_by_name("pinned");
                let pinned_vals = if let Some(col) = pinned_col {
                    col.as_any().downcast_ref::<BooleanArray>()
                } else {
                    None
                };

                // Handle optional model column
                let model_col = batch.column_by_name("model");
                let model_vals = if let Some(col) = model_col {
                    col.as_any().downcast_ref::<StringArray>()
                } else {
                    None
                };

                // LanceDB includes _distance column with similarity scores (lower = more similar)
                let distance_col = batch.column_by_name("_distance");
                let distance_vals = if let Some(col) = distance_col {
                    col.as_any().downcast_ref::<Float32Array>()
                } else {
                    None
                };

                for i in 0..batch.num_rows() {
                    let id = ids.value(i).to_string();
                    let title = titles.value(i).to_string();
                    let content = contents.value(i).to_string();
                    let pinned = pinned_vals.map(|p| p.value(i)).unwrap_or(false);
                    let model = model_vals.map(|m| m.value(i).to_string());
                    // Convert distance to similarity score (1 / (1 + distance)) for display
                    let distance = distance_vals.map(|d| d.value(i)).unwrap_or(0.0);
                    let score = 1.0 / (1.0 + distance);

                    // Simple preview generation
                    let preview = if content.chars().count() > 50 {
                        format!("{}...", content.chars().take(50).collect::<String>())
                    } else {
                        content.clone()
                    };

                    search_results.push(ChatSummary {
                        id,
                        title,
                        preview,
                        score,
                        pinned,
                        model,
                    });
                }
            }
        }
    }

    search_results
}

fn expected_chats_table_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("messages", DataType::Utf8, false),
        Field::new("pinned", DataType::Boolean, false),
        Field::new("model", DataType::Utf8, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                CHAT_VECTOR_DIM as i32,
            ),
            true,
        ),
    ]))
}

/// Open the chat database, falling back to a temporary location when the preferred one cannot
/// be used. Every degradation is reported; nothing is swallowed.
async fn open_chats_table(db_path: &str, report_error: &StorageErrorReporter) -> Option<Table> {
    match open_chats_table_at(db_path).await {
        Ok(table) => return Some(table),
        Err(e) => {
            println!("VectorActor ERROR: {e}");
            let fallback = std::env::temp_dir().join("plugable-chat-lancedb");
            report_error(format!(
                "Chat history could not be opened ({e}). Using a temporary location, so chats may not survive a restart."
            ));
            match open_chats_table_at(&fallback.to_string_lossy()).await {
                Ok(table) => Some(table),
                Err(e2) => {
                    println!("VectorActor ERROR: fallback also failed: {e2}");
                    report_error(format!(
                        "Chat history is unavailable ({e2}). New chats will not be saved."
                    ));
                    None
                }
            }
        }
    }
}

async fn open_chats_table_at(db_path: &str) -> Result<Table, String> {
    let db_connection = connect(db_path)
        .execute()
        .await
        .map_err(|e| format!("could not connect to chat storage at {db_path}: {e}"))?;
    ensure_chats_table_schema(&db_connection, std::path::Path::new(db_path)).await
}

/// Name for the copy of an old table kept when its schema is replaced.
fn backup_dir_name(unix_secs: u64) -> String {
    format!("chats.lance.backup-{unix_secs}")
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    batch.column_by_name(name)?.as_any().downcast_ref::<StringArray>()
}

/// Read rows from a batch of any past or present schema; absent columns take defaults, and a
/// vector of the wrong width becomes a placeholder so it is re-embedded later.
fn rows_from_batch(batch: &RecordBatch) -> Vec<ChatRow> {
    let (Some(ids), Some(titles)) = (str_col(batch, "id"), str_col(batch, "title")) else {
        return Vec::new();
    };
    let contents = str_col(batch, "content");
    let messages = str_col(batch, "messages");
    let models = str_col(batch, "model");
    let pinned = batch
        .column_by_name("pinned")
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>());
    let vectors = batch
        .column_by_name("vector")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());

    (0..batch.num_rows())
        .map(|i| {
            let vector = vectors
                .filter(|v| !v.is_null(i))
                .and_then(|v| {
                    v.value(i)
                        .as_any()
                        .downcast_ref::<Float32Array>()
                        .map(|f| f.values().to_vec())
                })
                .filter(|v| v.len() == CHAT_VECTOR_DIM)
                .unwrap_or_else(|| vec![0.0; CHAT_VECTOR_DIM]);
            ChatRow {
                id: ids.value(i).to_string(),
                title: titles.value(i).to_string(),
                content: contents.map(|c| c.value(i).to_string()).unwrap_or_default(),
                messages: messages.map(|c| c.value(i).to_string()).unwrap_or_default(),
                pinned: pinned.map(|p| p.value(i)).unwrap_or(false),
                model: models.filter(|m| !m.is_null(i)).map(|m| m.value(i).to_string()),
                vector,
            }
        })
        .collect()
}

async fn read_all_rows(table: &Table) -> Vec<ChatRow> {
    let mut rows = Vec::new();
    if let Ok(mut stream) = table.query().execute().await {
        while let Some(Ok(batch)) = stream.next().await {
            rows.extend(rows_from_batch(&batch));
        }
    }
    rows
}

fn build_batch(schema: Arc<Schema>, rows: &[ChatRow]) -> Result<RecordBatch, String> {
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        rows.iter()
            .map(|r| Some(r.vector.iter().map(|v| Some(*v)).collect::<Vec<_>>())),
        CHAT_VECTOR_DIM as i32,
    );
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.id.as_str()))),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.title.as_str()))),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.content.as_str()))),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.messages.as_str()))),
            Arc::new(BooleanArray::from(rows.iter().map(|r| r.pinned).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.model.clone()).collect::<Vec<Option<String>>>(),
            )),
            Arc::new(vector_array),
        ],
    )
    .map_err(|e| format!("could not build chat record batch: {e}"))
}

async fn create_chats_table(db_connection: &Connection, rows: &[ChatRow]) -> Result<Table, String> {
    let schema = expected_chats_table_schema();
    let batch = if rows.is_empty() {
        RecordBatch::new_empty(schema.clone())
    } else {
        build_batch(schema.clone(), rows)?
    };
    db_connection
        .create_table("chats", RecordBatchIterator::new(vec![Ok(batch)], schema))
        .execute()
        .await
        .map_err(|e| format!("could not create chats table: {e}"))
}

/// Open the `chats` table, creating it or migrating an older layout.
///
/// A layout that differs from the current one is never dropped blind: the old table directory
/// is copied aside first and every readable row is carried into the new table (vectors that no
/// longer fit become placeholders and are re-embedded later). If the copy cannot be made, the
/// old table is left untouched and an error is returned.
async fn ensure_chats_table_schema(db_connection: &Connection, db_dir: &std::path::Path) -> Result<Table, String> {
    let expected_schema = expected_chats_table_schema();

    let table = match db_connection.open_table("chats").execute().await {
        Ok(table) => table,
        Err(_) => {
            println!("VectorActor: Creating new chats table");
            return create_chats_table(db_connection, &[]).await;
        }
    };

    let existing_schema = match table.schema().await {
        Ok(s) => s,
        Err(e) => {
            println!("VectorActor WARNING: Failed to get schema, using existing table: {}", e);
            return Ok(table);
        }
    };

    let existing_dim = existing_schema
        .field_with_name("vector")
        .ok()
        .and_then(|f| match f.data_type() {
            DataType::FixedSizeList(_, dim) => Some(*dim),
            _ => None,
        });
    let matches = existing_schema.fields().len() == expected_schema.fields().len()
        && existing_dim == Some(CHAT_VECTOR_DIM as i32);
    if matches {
        return Ok(table);
    }

    println!(
        "VectorActor: Schema mismatch detected (vector dim {:?} -> {}, fields {} -> {}). Backing up and migrating...",
        existing_dim,
        CHAT_VECTOR_DIM,
        existing_schema.fields().len(),
        expected_schema.fields().len()
    );

    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let source = db_dir.join("chats.lance");
    let backup = db_dir.join(backup_dir_name(unix_secs));
    copy_dir_recursive(&source, &backup).map_err(|e| {
        format!(
            "chat history uses an older layout and could not be backed up to {} ({e}); it was left untouched",
            backup.display()
        )
    })?;
    println!("VectorActor: Backed up old chats table to {}", backup.display());

    let rows = read_all_rows(&table).await;
    println!("VectorActor: Migrating {} chat(s)", rows.len());
    db_connection
        .drop_table("chats", &[])
        .await
        .map_err(|e| format!("could not replace the old chats table ({e}); a backup is at {}", backup.display()))?;
    create_chats_table(db_connection, &rows)
        .await
        .map_err(|e| format!("{e}; a backup of your chats is at {}", backup.display()))
}

/// Replace the stored rows with these ids by `rows` (delete, then add). Returns success.
async fn upsert_chat_rows(chat_table: &Table, rows: &[ChatRow]) -> bool {
    let schema = match chat_table.schema().await {
        Ok(s) => s,
        Err(e) => {
            println!("VectorActor ERROR: Failed to get schema: {}", e);
            return false;
        }
    };
    let batch = match build_batch(schema.clone(), rows) {
        Ok(b) => b,
        Err(e) => {
            println!("VectorActor ERROR: {e}");
            return false;
        }
    };

    // delete-then-add: merge_insert is not used because of API issues in this lancedb version.
    for row in rows {
        if let Err(e) = chat_table.delete(&format!("id = '{}'", escape_sql(&row.id))).await {
            println!(
                "VectorActor WARNING: Delete before upsert failed (may be ok if new): {}",
                e
            );
        }
    }

    match chat_table
        .add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
        .execute()
        .await
    {
        Ok(_) => {
            println!("VectorActor: Successfully saved {} chat record(s) to LanceDB", rows.len());
            true
        }
        Err(e) => {
            println!("VectorActor ERROR: Failed to add chat to LanceDB: {}", e);
            false
        }
    }
}

async fn fetch_chat_messages_json(chat_table: Table, id: String) -> Option<String> {
    fetch_full_chat_record(chat_table, id)
        .await
        .map(|r| r.messages)
        .filter(|m| !m.is_empty())
}

async fn fetch_full_chat_record(chat_table: Table, id: String) -> Option<ChatRow> {
    let query = chat_table
        .query()
        .only_if(format!("id = '{}'", escape_sql(&id)))
        .limit(1);
    let mut query_stream = query.execute().await.ok()?;
    if let Some(Ok(batch)) = query_stream.next().await {
        return rows_from_batch(&batch).into_iter().next();
    }
    None
}

/// Chats whose vector is still the placeholder, as `(id, text to embed)`.
async fn list_chats_missing_embedding(chat_table: &Table) -> Vec<(String, String)> {
    read_all_rows(chat_table)
        .await
        .into_iter()
        .filter(|r| is_placeholder_vector(&r.vector) && !r.content.trim().is_empty())
        .map(|r| (r.id, r.content))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, messages: &str) -> ChatRow {
        ChatRow {
            id: id.to_string(),
            title: format!("title {id}"),
            content: format!("content {id}"),
            messages: messages.to_string(),
            pinned: false,
            model: Some("m1".to_string()),
            vector: vec![0.5; CHAT_VECTOR_DIM],
        }
    }

    async fn new_table(dir: &std::path::Path) -> Table {
        open_chats_table_at(&dir.to_string_lossy()).await.unwrap()
    }

    #[tokio::test]
    async fn saves_messages_and_model_without_an_embedding() {
        let dir = tempfile::tempdir().unwrap();
        let table = new_table(dir.path()).await;
        let r = merge_upsert(None, "c1".into(), "T".into(), "C".into(), "[{\"role\":\"user\"}]".into(), None, None, Some("phi".into()));
        assert!(is_placeholder_vector(&r.vector));
        assert!(upsert_chat_rows(&table, &[r]).await);

        let got = fetch_full_chat_record(table.clone(), "c1".into()).await.unwrap();
        assert_eq!(got.messages, "[{\"role\":\"user\"}]");
        assert_eq!(got.model.as_deref(), Some("phi"));
        assert_eq!(fetch_chat_messages_json(table, "c1".into()).await.unwrap(), "[{\"role\":\"user\"}]");
    }

    #[tokio::test]
    async fn upsert_replaces_instead_of_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let table = new_table(dir.path()).await;
        assert!(upsert_chat_rows(&table, &[row("c1", "[1]")]).await);
        assert!(upsert_chat_rows(&table, &[row("c1", "[1,2]")]).await);
        let all = read_all_rows(&table).await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].messages, "[1,2]");
    }

    #[test]
    fn a_save_without_embedding_pin_or_model_keeps_what_is_stored() {
        let mut stored = row("c1", "[1]");
        stored.pinned = true;
        let merged = merge_upsert(Some(stored.clone()), "c1".into(), "T2".into(), "C2".into(), "[1,2]".into(), None, None, None);
        assert!(merged.pinned);
        assert_eq!(merged.model.as_deref(), Some("m1"));
        assert_eq!(merged.vector, stored.vector);
        assert_eq!(merged.title, "T2");
    }

    #[test]
    fn a_new_embedding_and_an_explicit_pin_win_over_stored_values() {
        let stored = row("c1", "[1]");
        let merged = merge_upsert(Some(stored), "c1".into(), "T".into(), "C".into(), "[]".into(), Some(vec![1.0; CHAT_VECTOR_DIM]), Some(true), Some("m2".into()));
        assert_eq!(merged.vector, vec![1.0; CHAT_VECTOR_DIM]);
        assert!(merged.pinned);
        assert_eq!(merged.model.as_deref(), Some("m2"));
    }

    #[test]
    fn an_embedding_of_the_wrong_width_is_replaced_by_a_placeholder() {
        let merged = merge_upsert(None, "c".into(), "T".into(), "C".into(), "[]".into(), Some(vec![1.0; 3]), None, None);
        assert!(is_placeholder_vector(&merged.vector) && merged.vector.len() == CHAT_VECTOR_DIM);
    }

    #[tokio::test]
    async fn lists_only_placeholder_chats_for_backfill() {
        let dir = tempfile::tempdir().unwrap();
        let table = new_table(dir.path()).await;
        let mut needs = row("needs", "[]");
        needs.vector = vec![0.0; CHAT_VECTOR_DIM];
        assert!(upsert_chat_rows(&table, &[needs, row("has", "[]")]).await);
        let missing = list_chats_missing_embedding(&table).await;
        assert_eq!(missing, vec![("needs".to_string(), "content needs".to_string())]);
    }

    /// Create a table with a previous layout (no `model` column, 384-wide vectors).
    async fn make_old_layout(dir: &std::path::Path) {
        let conn = connect(&dir.to_string_lossy()).execute().await.unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("messages", DataType::Utf8, false),
            Field::new("pinned", DataType::Boolean, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 384),
                true,
            ),
        ]));
        let vector = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            vec![Some(vec![Some(0.25f32); 384])],
            384,
        );
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["old1"])),
                Arc::new(StringArray::from(vec!["Old chat"])),
                Arc::new(StringArray::from(vec!["old content"])),
                Arc::new(StringArray::from(vec!["[{\"role\":\"user\",\"content\":\"hi\"}]"])),
                Arc::new(BooleanArray::from(vec![true])),
                Arc::new(vector),
            ],
        )
        .unwrap();
        conn.create_table("chats", RecordBatchIterator::new(vec![Ok(batch)], schema))
            .execute()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn schema_mismatch_backs_up_then_migrates_rows_and_never_loses_them() {
        let dir = tempfile::tempdir().unwrap();
        make_old_layout(dir.path()).await;

        let table = new_table(dir.path()).await;

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("chats.lance.backup-"))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup of the old table");
        assert!(backups[0].path().join("_versions").exists() || backups[0].path().read_dir().unwrap().next().is_some());

        assert_eq!(table.schema().await.unwrap(), expected_chats_table_schema());
        let rows = read_all_rows(&table).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "old1");
        assert_eq!(rows[0].messages, "[{\"role\":\"user\",\"content\":\"hi\"}]");
        assert!(rows[0].pinned);
        assert!(is_placeholder_vector(&rows[0].vector), "old-width vector becomes a placeholder to re-embed");
    }

    #[tokio::test]
    async fn matching_schema_is_left_alone_without_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let table = new_table(dir.path()).await;
        assert!(upsert_chat_rows(&table, &[row("c1", "[1]")]).await);
        drop(table);
        let table = new_table(dir.path()).await;
        assert_eq!(read_all_rows(&table).await.len(), 1);
        assert!(!std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("backup")));
    }

    #[tokio::test]
    async fn unusable_primary_location_falls_back_and_reports() {
        let blocker = tempfile::NamedTempFile::new().unwrap();
        // A path *under a regular file* can never be created.
        let bad = blocker.path().join("db");
        let errors = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = errors.clone();
        let report: StorageErrorReporter = Arc::new(move |m| sink.lock().unwrap().push(m));
        let table = open_chats_table(&bad.to_string_lossy(), &report).await;
        assert!(table.is_some(), "falls back to a temporary location");
        let errors = errors.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("temporary location"));
    }

    #[test]
    fn backup_name_carries_the_timestamp() {
        assert_eq!(backup_dir_name(1700000000), "chats.lance.backup-1700000000");
    }

    #[test]
    fn quotes_in_ids_cannot_break_out_of_the_filter() {
        assert_eq!(escape_sql("a'b"), "a''b");
    }
}
