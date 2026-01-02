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

pub struct ChatVectorStoreActor {
    vector_msg_rx: mpsc::Receiver<VectorMsg>,
    chat_table: Table,
}

impl ChatVectorStoreActor {
    pub async fn new(vector_msg_rx: mpsc::Receiver<VectorMsg>, db_path: &str) -> Self {
        let db_connection = connect(db_path)
            .execute()
            .await
            .expect("Failed to connect to LanceDB");

        // Ensure table exists
        let chat_table = ensure_chats_table_schema(&db_connection).await;

        Self {
            vector_msg_rx,
            chat_table,
        }
    }

    pub async fn run(mut self) {
        while let Some(msg) = self.vector_msg_rx.recv().await {
            // Clone table handle for parallel execution (it's cheap, just an Arc internally)
            let chat_table = self.chat_table.clone();

            // Spawn a detached task for every request.
            // This ensures the actor mailbox never clogs, even if a query takes 100ms.
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
                        let zero_embedding_vector = vec![0.0; 768];
                        let search_results =
                            search_chats_by_embedding(chat_table, zero_embedding_vector, 100)
                                .await;
                        let _ = respond_to.send(search_results);
                    }
                    VectorMsg::UpsertChatRecord {
                        id,
                        title,
                        content,
                        messages,
                        embedding_vector,
                        pinned,
                        model,
                    } => {
                        if let Some(vector_values) = embedding_vector {
                            upsert_chat_record_with_embedding(
                                &chat_table,
                                id,
                                title,
                                content,
                                messages,
                                vector_values,
                                pinned,
                                model,
                            )
                            .await;
                        } else {
                            println!("VectorActor WARNING: No vector provided, skipping upsert!");
                        }
                    }
                    VectorMsg::FetchChatMessages { id, respond_to } => {
                        let chat_messages_json = fetch_chat_messages_json(chat_table, id).await;
                        let _ = respond_to.send(chat_messages_json);
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
                        // We need to clone table for async block if we were spawning, but we are in spawned block
                        let chat_table_clone = chat_table.clone();
                        if let Some((_, old_title, content, messages, vector, old_pinned, model)) =
                            fetch_full_chat_record(chat_table_clone.clone(), id.clone()).await
                        {
                            let new_title = title.unwrap_or(old_title.clone());
                            let new_pinned = pinned.unwrap_or(old_pinned);
                            println!(
                                "VectorActor: Found chat to update: '{}' -> '{}', pinned: {} -> {}",
                                old_title, new_title, old_pinned, new_pinned
                            );
                            upsert_chat_record_with_embedding(
                                &chat_table_clone,
                                id,
                                new_title,
                                content,
                                messages,
                                vector,
                                new_pinned,
                                model,
                            )
                            .await;
                            let _ = respond_to.send(true);
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
                        let filter = format!("id = '{}'", id);
                        println!("VectorActor: Delete filter: {}", filter);
                        match chat_table.delete(&filter).await {
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
                }
            });
        }
    }
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
                    let preview = if content.len() > 50 {
                        format!("{}...", &content[0..50])
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
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 768),
            true,
        ),
    ]))
}

async fn ensure_chats_table_schema(db_connection: &Connection) -> Table {
    let expected_schema = expected_chats_table_schema();

    // Try to open existing table
    let result = db_connection.open_table("chats").execute().await;

    match result {
        Ok(table) => {
            // Check if the existing table has the expected schema
            match table.schema().await {
                Ok(existing_schema) => {
                    let existing_field_count = existing_schema.fields().len();
                    let expected_field_count = expected_schema.fields().len();

                    // Check vector field dimension specifically
                    let existing_dim = existing_schema
                        .field_with_name("vector")
                        .ok()
                        .and_then(|f| match f.data_type() {
                            DataType::FixedSizeList(_, dim) => Some(*dim),
                            _ => None,
                        });

                    let expected_dim = Some(768i32);

                    if existing_field_count != expected_field_count || existing_dim != expected_dim {
                        println!(
                            "VectorActor: Schema mismatch detected! Dim: {:?} -> {:?}, Fields: {} -> {}. Recreating table...",
                            existing_dim,
                            expected_dim,
                            existing_field_count,
                            expected_field_count
                        );

                        // Drop and recreate the table
                        if let Err(e) = db_connection.drop_table("chats").await {
                            println!("VectorActor WARNING: Failed to drop old table: {}", e);
                        }

                        let batch = RecordBatch::new_empty(expected_schema.clone());
                        db_connection
                            .create_table(
                                "chats",
                                RecordBatchIterator::new(
                                    vec![batch].into_iter().map(Ok),
                                    expected_schema,
                                ),
                            )
                            .execute()
                            .await
                            .expect("Failed to create chats table after schema migration")
                    } else {
                        table
                    }
                }
                Err(e) => {
                    println!(
                        "VectorActor WARNING: Failed to get schema, using existing table: {}",
                        e
                    );
                    table
                }
            }
        }
        Err(_) => {
            // Create the table if it doesn't exist
            println!("VectorActor: Creating new chats table");
            let batch = RecordBatch::new_empty(expected_schema.clone());

            db_connection
                .create_table(
                    "chats",
                    RecordBatchIterator::new(vec![batch].into_iter().map(Ok), expected_schema),
                )
                .execute()
                .await
                .expect("Failed to create chats table")
        }
    }
}

async fn upsert_chat_record_with_embedding(
    chat_table: &Table,
    id: String,
    title: String,
    content: String,
    messages: String,
    embedding_vector: Vec<f32>,
    pinned: bool,
    model: Option<String>,
) {
    let schema = match chat_table.schema().await {
        Ok(s) => s,
        Err(e) => {
            println!("VectorActor ERROR: Failed to get schema: {}", e);
            return;
        }
    };

    let id_array = StringArray::from(vec![id.clone()]);
    let title_array = StringArray::from(vec![title.clone()]);
    let content_array = StringArray::from(vec![content]);
    let messages_array = StringArray::from(vec![messages]);
    let pinned_array = BooleanArray::from(vec![pinned]);
    let model_array = match model {
        Some(m) => StringArray::from(vec![Some(m)]),
        None => StringArray::from(vec![Option::<String>::None]),
    };

    let vector_values = Float32Array::from(embedding_vector);
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vec![Some(
            vector_values
                .values()
                .iter()
                .map(|v| Some(*v))
                .collect::<Vec<_>>(),
        )],
        768,
    );

    let batch = match RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(id_array),
            Arc::new(title_array),
            Arc::new(content_array),
            Arc::new(messages_array),
            Arc::new(pinned_array),
            Arc::new(model_array),
            Arc::new(vector_array),
        ],
    ) {
        Ok(b) => b,
        Err(e) => {
            println!("VectorActor ERROR: Failed to create RecordBatch: {}", e);
            return;
        }
    };

    // Perform upsert by deleting existing record (if any) and adding new one
    // This is a workaround for merge_insert API issues in lancedb 0.4
    if let Err(e) = chat_table.delete(&format!("id = '{}'", id)).await {
        println!(
            "VectorActor WARNING: Delete before upsert failed (may be ok if new): {}",
            e
        );
    }

    match chat_table
        .add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
        .execute()
        .await
    {
        Ok(_) => {
            println!(
                "VectorActor: Successfully saved chat '{}' to LanceDB",
                title
            )
        },
        Err(e) => println!("VectorActor ERROR: Failed to add chat to LanceDB: {}", e),
    }
}

async fn fetch_chat_messages_json(chat_table: Table, id: String) -> Option<String> {
    let query = chat_table
        .query()
        .only_if(format!("id = '{}'", id))
        .limit(1);
    let mut query_stream = query.execute().await.ok()?;
    if let Some(Ok(batch)) = query_stream.next().await {
        let messages = batch
            .column_by_name("messages")?
            .as_any()
            .downcast_ref::<StringArray>()?;
        if messages.len() > 0 {
            return Some(messages.value(0).to_string());
        }
    }
    None
}

async fn fetch_full_chat_record(
    chat_table: Table,
    id: String,
) -> Option<(String, String, String, String, Vec<f32>, bool, Option<String>)> {
    let query = chat_table
        .query()
        .only_if(format!("id = '{}'", id))
        .limit(1);
    let mut query_stream = query.execute().await.ok()?;
    if let Some(Ok(batch)) = query_stream.next().await {
        if batch.num_rows() == 0 {
            return None;
        }

        let ids = batch
            .column_by_name("id")?
            .as_any()
            .downcast_ref::<StringArray>()?;
        let titles = batch
            .column_by_name("title")?
            .as_any()
            .downcast_ref::<StringArray>()?;
        let contents = batch
            .column_by_name("content")?
            .as_any()
            .downcast_ref::<StringArray>()?;
        let messages_col = batch
            .column_by_name("messages")?
            .as_any()
            .downcast_ref::<StringArray>()?;

        let pinned_col = batch.column_by_name("pinned");
        let pinned = if let Some(col) = pinned_col {
            col.as_any().downcast_ref::<BooleanArray>()?.value(0)
        } else {
            false
        };

        let model_col = batch.column_by_name("model");
        let model = if let Some(col) = model_col {
            let arr = col.as_any().downcast_ref::<StringArray>()?;
            if arr.is_null(0) {
                None
            } else {
                Some(arr.value(0).to_string())
            }
        } else {
            None
        };

        let vectors = batch
            .column_by_name("vector")?
            .as_any()
            .downcast_ref::<FixedSizeListArray>()?;
        let vector_val = vectors.value(0);
        let float_array = vector_val.as_any().downcast_ref::<Float32Array>()?;
        let vector: Vec<f32> = float_array.values().to_vec();

        return Some((
            ids.value(0).to_string(),
            titles.value(0).to_string(),
            contents.value(0).to_string(),
            messages_col.value(0).to_string(),
            vector,
            pinned,
            model,
        ));
    }
    None
}
