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
                        println!("VectorActor: Upserting chat (id: {}, title: {}, has_vector: {})", 
                            &id[..8.min(id.len())], title, vector.is_some());
                        if let Some(vec) = vector {
                            println!("VectorActor: Vector length: {}", vec.len());
                            perform_upsert(&table, id, title, content, messages, vec, pinned).await;
                        } else {
                            println!("VectorActor WARNING: No vector provided, skipping upsert!");
                        }
                    }
                    VectorMsg::GetChat { id, respond_to } => {
                         let messages = perform_get_chat(table, id).await;
                         let _ = respond_to.send(messages);
                    }
                    VectorMsg::UpdateChatMetadata { id, title, pinned, respond_to } => {
                        println!("VectorActor: Updating metadata (id: {}, title: {:?}, pinned: {:?})", 
                            &id[..8.min(id.len())], title, pinned);
                        // We need to clone table for async block if we were spawning, but we are in spawned block
                        let table_clone = table.clone();
                        if let Some((_, old_title, content, messages, vector, old_pinned)) = perform_get_full_chat(table_clone.clone(), id.clone()).await {
                            let new_title = title.unwrap_or(old_title.clone());
                            let new_pinned = pinned.unwrap_or(old_pinned);
                            println!("VectorActor: Found chat to update: '{}' -> '{}', pinned: {} -> {}", 
                                old_title, new_title, old_pinned, new_pinned);
                            perform_upsert(&table_clone, id, new_title, content, messages, vector, new_pinned).await;
                            let _ = respond_to.send(true);
                        } else {
                            println!("VectorActor ERROR: Chat {} not found for metadata update", &id[..8.min(id.len())]);
                            let _ = respond_to.send(false);
                        }
                    }
                    VectorMsg::DeleteChat { id, respond_to } => {
                        println!("VectorActor: Deleting chat (id: {})", id);
                        let filter = format!("id = '{}'", id);
                        println!("VectorActor: Delete filter: {}", filter);
                        match table.delete(&filter).await {
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

async fn perform_search(table: Table, vector: Vec<f32>, limit: usize) -> Vec<ChatSummary> {
    // LanceDB Async Query - results are automatically sorted by similarity (closest first)
    let query_result = table
        .query()
        .nearest_to(vector); // Vector search

    let query = match query_result {
        Ok(q) => q,
        Err(e) => {
            println!("VectorActor ERROR: Failed to create vector query: {}", e);
            return vec![];
        }
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
                    // Convert distance to similarity score (1 / (1 + distance)) for display
                    let distance = distance_vals.map(|d| d.value(i)).unwrap_or(0.0);
                    let score = 1.0 / (1.0 + distance);
                    
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
                        score,
                        pinned,
                    });
                }
            }
        }
    }
    
    println!("VectorActor: Search returned {} results", results.len());
    results
}

fn get_expected_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("messages", DataType::Utf8, false),
        Field::new("pinned", DataType::Boolean, false),
        Field::new("vector", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            384
        ), true),
    ]))
}

async fn setup_table(db: &Connection) -> Table {
    let expected_schema = get_expected_schema();
    let expected_field_count = expected_schema.fields().len();
    
    // Try to open existing table
    let result = db.open_table("chats").execute().await;
    
    match result {
        Ok(table) => {
            // Check if the existing table has the expected schema
            match table.schema().await {
                Ok(existing_schema) => {
                    let existing_field_count = existing_schema.fields().len();
                    if existing_field_count != expected_field_count {
                        println!("VectorActor: Schema mismatch detected! Table has {} fields, expected {}. Recreating table...", 
                            existing_field_count, expected_field_count);
                        
                        // Drop and recreate the table
                        if let Err(e) = db.drop_table("chats").await {
                            println!("VectorActor WARNING: Failed to drop old table: {}", e);
                        }
                        
                        let batch = RecordBatch::new_empty(expected_schema.clone());
                        db.create_table("chats", RecordBatchIterator::new(vec![batch].into_iter().map(Ok), expected_schema))
                            .execute()
                            .await
                            .expect("Failed to create chats table after schema migration")
                    } else {
                        println!("VectorActor: Table schema is up to date ({} fields)", existing_field_count);
                        table
                    }
                }
                Err(e) => {
                    println!("VectorActor WARNING: Failed to get schema, using existing table: {}", e);
                    table
                }
            }
        }
        Err(_) => {
            // Create the table if it doesn't exist
            println!("VectorActor: Creating new chats table with {} fields", expected_field_count);
            let batch = RecordBatch::new_empty(expected_schema.clone());
            
            db.create_table("chats", RecordBatchIterator::new(vec![batch].into_iter().map(Ok), expected_schema))
                .execute()
                .await
                .expect("Failed to create chats table")
        }
    }
}

async fn perform_upsert(table: &Table, id: String, title: String, content: String, messages: String, vector: Vec<f32>, pinned: bool) {
    println!("VectorActor: perform_upsert starting for id={}", &id[..8.min(id.len())]);
    
    let schema = match table.schema().await {
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
    
    let vector_values = Float32Array::from(vector);
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vec![Some(vector_values.values().iter().map(|v| Some(*v)).collect::<Vec<_>>())],
        384
    );

    let batch = match RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(id_array),
            Arc::new(title_array),
            Arc::new(content_array),
            Arc::new(messages_array),
            Arc::new(pinned_array),
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
    if let Err(e) = table.delete(&format!("id = '{}'", id)).await {
        println!("VectorActor WARNING: Delete before upsert failed (may be ok if new): {}", e);
    }
    
    match table.add(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema)))
        .execute()
        .await 
    {
        Ok(_) => println!("VectorActor: Successfully saved chat '{}' to LanceDB", title),
        Err(e) => println!("VectorActor ERROR: Failed to add chat to LanceDB: {}", e),
    }
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

