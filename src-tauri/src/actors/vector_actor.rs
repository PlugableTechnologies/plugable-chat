use crate::protocol::{VectorMsg, ChatSummary};
use lancedb::{connect, Table, Connection};
use lancedb::query::{QueryBase, ExecutableQuery};
use arrow_schema::{DataType, Field, Schema};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, StringArray, Float32Array, BooleanArray, FixedSizeListArray};
use arrow_array::types::Float32Type;
use std::sync::Arc;
use tokio::sync::mpsc;
use futures::StreamExt;

pub struct VectorActor {
    rx: mpsc::Receiver<VectorMsg>,
    table: Table,
}

impl VectorActor {
    pub async fn new(rx: mpsc::Receiver<VectorMsg>, db_path: &str) -> Self {
        let db = connect(db_path).execute().await.expect("Failed to connect to LanceDB");
        
        // Ensure table exists
        let table = setup_table(&db).await;

        Self { rx, table }
    }

    pub async fn run(mut self) {
        println!("VectorActor loop starting");
        while let Some(msg) = self.rx.recv().await {
            // Clone table handle for parallel execution (it's cheap, just an Arc internally)
            let table = self.table.clone();
            
            // Spawn a detached task for every request.
            // This ensures the actor mailbox never clogs, even if a query takes 100ms.
            tokio::spawn(async move {
                match msg {
                    VectorMsg::SearchHistory { query_vector, limit, respond_to } => {
                        println!("VectorActor: Searching history (limit: {})", limit);
                        let results = perform_search(table, query_vector, limit).await;
                        let _ = respond_to.send(results);
                    }
                    VectorMsg::GetAllChats { respond_to } => {
                        println!("VectorActor: Getting all chats");
                        let zero_vector = vec![0.0; 384];
                        let results = perform_search(table, zero_vector, 100).await;
                        let _ = respond_to.send(results);
                    }
                    VectorMsg::UpsertChat { id, title, content, messages, vector, pinned } => {
                        println!("VectorActor: Upserting chat (id: {}, title: {})", id, title);
                        if let Some(vec) = vector {
                           let _ = perform_upsert(&table, id, title, content, messages, vec, pinned).await;
                        }
                    }
                    VectorMsg::GetChat { id, respond_to } => {
                         let messages = perform_get_chat(table, id).await;
                         let _ = respond_to.send(messages);
                    }
                    VectorMsg::UpdateChatMetadata { id, title, pinned, respond_to } => {
                        println!("VectorActor: Updating metadata (id: {})", id);
                        // We need to clone table for async block if we were spawning, but we are in spawned block
                        let table_clone = table.clone();
                        if let Some((_, old_title, content, messages, vector, old_pinned)) = perform_get_full_chat(table_clone.clone(), id.clone()).await {
                            let new_title = title.unwrap_or(old_title);
                            let new_pinned = pinned.unwrap_or(old_pinned);
                            perform_upsert(&table_clone, id, new_title, content, messages, vector, new_pinned).await;
                            let _ = respond_to.send(true);
                        } else {
                            let _ = respond_to.send(false);
                        }
                    }
                    VectorMsg::DeleteChat { id, respond_to } => {
                        println!("VectorActor: Deleting chat (id: {})", id);
                        let result = table.delete(&format!("id = '{}'", id)).await;
                        let _ = respond_to.send(result.is_ok());
                    }
                }
            });
        }
    }
}

async fn perform_search(table: Table, vector: Vec<f32>, limit: usize) -> Vec<ChatSummary> {
    // LanceDB Async Query
    let query_result = table
        .query()
        .nearest_to(vector); // Vector search

    let query = match query_result {
        Ok(q) => q,
        Err(_) => return vec![],
    };

    let stream = query
        .limit(limit)
        .execute()
        .await;

    let mut results = Vec::new();

    if let Ok(mut stream) = stream {
        while let Some(batch) = stream.next().await {
            if let Ok(batch) = batch {
                let ids = batch.column_by_name("id").unwrap().as_any().downcast_ref::<StringArray>().unwrap();
                let titles = batch.column_by_name("title").unwrap().as_any().downcast_ref::<StringArray>().unwrap();
                let contents = batch.column_by_name("content").unwrap().as_any().downcast_ref::<StringArray>().unwrap();
                // Handle optional pinned column for backward compatibility
                let pinned_col = batch.column_by_name("pinned");
                let pinned_vals = if let Some(col) = pinned_col {
                    col.as_any().downcast_ref::<BooleanArray>()
                } else {
                    None
                };

                for i in 0..batch.num_rows() {
                    let id = ids.value(i).to_string();
                    let title = titles.value(i).to_string();
                    let content = contents.value(i).to_string();
                    let pinned = pinned_vals.map(|p| p.value(i)).unwrap_or(false);
                    
                    // Simple preview generation
                    let preview = if content.len() > 50 {
                        format!("{}...", &content[0..50])
                    } else {
                        content.clone()
                    };

                    results.push(ChatSummary {
                        id,
                        title,
                        preview,
                        score: 0.0, // TODO: Get score from distance
                        pinned,
                    });
                }
            }
        }
    }
    
    results
}

async fn setup_table(db: &Connection) -> Table {
    // Define Arrow Schema for: id (utf8), title (utf8), vector (fixed_size_list<384>)
    // If table doesn't exist, create it. If it does, open it.
    let result = db.open_table("chats").execute().await;
    
    match result {
        Ok(table) => table,
        Err(_) => {
             // Create the table if it doesn't exist
             let schema = Arc::new(Schema::new(vec![
                 Field::new("id", DataType::Utf8, false),
                 Field::new("title", DataType::Utf8, false),
                 Field::new("content", DataType::Utf8, false),
                 Field::new("messages", DataType::Utf8, false),
                 Field::new("pinned", DataType::Boolean, false),
                 Field::new("vector", DataType::FixedSizeList(
                     Arc::new(Field::new("item", DataType::Float32, true)),
                     384
                 ), true),
             ]));
             
             let batch = RecordBatch::new_empty(schema.clone());
             
             db.create_table("chats", RecordBatchIterator::new(vec![batch].into_iter().map(Ok), schema))
                 .execute()
                 .await
                 .expect("Failed to create chats table")
        }
    }
}

async fn perform_upsert(table: &Table, id: String, title: String, content: String, messages: String, vector: Vec<f32>, pinned: bool) {
    let schema = table.schema().await.unwrap();
    
    let id_array = StringArray::from(vec![id.clone()]);
    let title_array = StringArray::from(vec![title]);
    let content_array = StringArray::from(vec![content]);
    let messages_array = StringArray::from(vec![messages]);
    let pinned_array = BooleanArray::from(vec![pinned]);
    
    let vector_values = Float32Array::from(vector);
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vec![Some(vector_values.values().iter().map(|v| Some(*v)).collect::<Vec<_>>())],
        384
    );

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(id_array),
            Arc::new(title_array),
            Arc::new(content_array),
            Arc::new(messages_array),
            Arc::new(pinned_array),
            Arc::new(vector_array),
        ],
    ).unwrap();

    // Perform upsert by deleting existing record (if any) and adding new one
    // This is a workaround for merge_insert API issues in lancedb 0.4
    let _ = table.delete(&format!("id = '{}'", id)).await;
    
    let _ = table.add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
        .execute()
        .await;
}

async fn perform_get_chat(table: Table, id: String) -> Option<String> {
    let query = table.query().only_if(format!("id = '{}'", id)).limit(1);
    let mut stream = query.execute().await.ok()?;
    if let Some(Ok(batch)) = stream.next().await {
        let messages = batch.column_by_name("messages")?.as_any().downcast_ref::<StringArray>()?;
        if messages.len() > 0 {
            return Some(messages.value(0).to_string());
        }
    }
    None
}

async fn perform_get_full_chat(table: Table, id: String) -> Option<(String, String, String, String, Vec<f32>, bool)> {
    let query = table.query().only_if(format!("id = '{}'", id)).limit(1);
    let mut stream = query.execute().await.ok()?;
    if let Some(Ok(batch)) = stream.next().await {
        if batch.num_rows() == 0 { return None; }
        
        let ids = batch.column_by_name("id")?.as_any().downcast_ref::<StringArray>()?;
        let titles = batch.column_by_name("title")?.as_any().downcast_ref::<StringArray>()?;
        let contents = batch.column_by_name("content")?.as_any().downcast_ref::<StringArray>()?;
        let messages_col = batch.column_by_name("messages")?.as_any().downcast_ref::<StringArray>()?;
        
        let pinned_col = batch.column_by_name("pinned");
        let pinned = if let Some(col) = pinned_col {
            col.as_any().downcast_ref::<BooleanArray>()?.value(0)
        } else {
            false
        };

        let vectors = batch.column_by_name("vector")?.as_any().downcast_ref::<FixedSizeListArray>()?;
        let vector_val = vectors.value(0);
        let float_array = vector_val.as_any().downcast_ref::<Float32Array>()?;
        let vector: Vec<f32> = float_array.values().to_vec();

        return Some((
            ids.value(0).to_string(),
            titles.value(0).to_string(),
            contents.value(0).to_string(),
            messages_col.value(0).to_string(),
            vector,
            pinned
        ));
    }
    None
}

