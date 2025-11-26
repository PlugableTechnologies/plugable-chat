use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub score: f32, // Similarity score
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub enum VectorMsg {
    /// Index a new chat or update an existing one
    UpsertChat {
        id: String,
        title: String,
        content: String,
        messages: String, // JSON string of full history
        // The actor will handle embedding generation internally via Foundry
        // or receive a pre-computed vector.
        vector: Option<Vec<f32>>, 
        pinned: bool,
    },
    /// Search for similar chats
    SearchHistory {
        query_vector: Vec<f32>, 
        limit: usize,
        // Channel to send results back to the caller (Orchestrator)
        respond_to: oneshot::Sender<Vec<ChatSummary>>,
    },
    /// Get all chats
    GetAllChats {
        respond_to: oneshot::Sender<Vec<ChatSummary>>,
    },
    /// Get a specific chat's messages
    GetChat {
        id: String,
        respond_to: oneshot::Sender<Option<String>>, // Returns JSON string of messages
    },
    /// Delete a chat
    DeleteChat {
        id: String,
        respond_to: oneshot::Sender<bool>,
    },
    /// Update chat metadata (title, pinned)
    UpdateChatMetadata {
        id: String,
        title: Option<String>,
        pinned: Option<bool>,
        respond_to: oneshot::Sender<bool>,
    },
}

pub enum FoundryMsg {
    /// Generate an embedding for a string
    GetEmbedding {
        text: String,
        respond_to: oneshot::Sender<Vec<f32>>,
    },
    /// Chat with the model (streaming)
    Chat {
        history: Vec<ChatMessage>,
        reasoning_effort: String,
        respond_to: tokio::sync::mpsc::UnboundedSender<String>,
    },
    /// Get available models
    GetModels {
        respond_to: oneshot::Sender<Vec<String>>,
    },
    /// Set the active model
    SetModel {
        model_id: String,
        respond_to: oneshot::Sender<bool>,
    },
}

pub enum McpMsg {
    ExecuteTool {
        tool_name: String,
        args: serde_json::Value,
    },
}
