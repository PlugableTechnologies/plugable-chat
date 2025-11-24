use crate::protocol::{VectorMsg, ChatSummary};
use lancedb::{connect, Table, Connection};
use lancedb::query::{QueryBase, ExecutableQuery};
use arrow_schema::{DataType, Field, Schema};
use arrow_array::{RecordBatch, RecordBatchIterator};
use std::sync::Arc;
use tokio::sync::mpsc;

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
                        // Ignore errors if receiver dropped (UI navigated away)
                        let _ = respond_to.send(results);
                    }
                    VectorMsg::GetAllChats { respond_to } => {
                        println!("VectorActor: Getting all chats");
                        // For now, since LanceDB doesn't have a simple "select *" without vector query, 
                        // we can query nearest to a zero vector with high limit, or implement a proper scan.
                        // LanceDB's rust SDK is still evolving. A zero vector search is a common workaround for "all" if we want ranked by something,
                        // but here we probably want chronological.
                        // Let's try a zero vector search for now as a placeholder to get data flowing.
                        let zero_vector = vec![0.0; 384];
                        let results = perform_search(table, zero_vector, 100).await;
                        let _ = respond_to.send(results);
                    }
                    VectorMsg::UpsertChat { id, title, content, vector } => {
                        println!("VectorActor: Upserting chat (id: {}, title: {})", id, title);
                        if let Some(vec) = vector {
                           let _ = perform_upsert(table, id, title, content, vec).await;
                        }
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

    if stream.is_err() { return vec![]; }
    
    // Process Arrow RecordBatches (Simplified for brevity)
    // In production, you would iterate the stream and map columns to ChatSummary structs
    // For now returning empty vec until we implement the record batch mapping
    vec![] 
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

async fn perform_upsert(_table: Table, _id: String, _title: String, _content: String, _vector: Vec<f32>) {
    // Convert Rust Vecs to Arrow Arrays and perform .add()
}

