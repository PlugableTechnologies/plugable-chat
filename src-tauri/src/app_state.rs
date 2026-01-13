//! Shared Tauri application state types.
//!
//! This module defines all the state structs managed by Tauri and shared
//! across commands. These types hold actor channels, configuration, and
//! runtime state for the application.

use crate::actors::database_toolbox_actor::DatabaseToolboxMsg;
use crate::actors::python_actor::PythonMsg;
use crate::actors::schema_vector_actor::SchemaVectorMsg;
use crate::actors::startup_actor::StartupMsg;
use crate::protocol::{FoundryMsg, McpHostMsg, RagMsg, VectorMsg};
use crate::settings::AppSettings;
use crate::settings_state_machine::SettingsStateMachine;
use crate::tool_capability::ToolLaunchFilter;
use crate::tool_registry::SharedToolRegistry;
use fastembed::TextEmbedding;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

/// GPU resource guard to serialize all GPU operations.
/// 
/// Only one GPU-intensive operation can run at a time to avoid
/// Metal/CUDA memory contention between LLM inference and embedding models.
/// This prevents silent model eviction that causes operations to hang.
pub struct GpuResourceGuard {
    /// The mutex that serializes GPU access
    pub mutex: Mutex<()>,
    /// Current operation description (for status feedback to UI)
    pub current_operation: RwLock<Option<String>>,
}

impl GpuResourceGuard {
    pub fn new() -> Self {
        Self {
            mutex: Mutex::new(()),
            current_operation: RwLock::new(None),
        }
    }
}

impl Default for GpuResourceGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Approval decision for tool calls
#[derive(Debug, Clone)]
pub enum ToolApprovalDecision {
    Approved,
    Rejected,
}

/// Pending tool approval state - maps approval keys to response channels
pub type PendingApprovals = Arc<RwLock<HashMap<String, oneshot::Sender<ToolApprovalDecision>>>>;

/// Actor message channel handles managed by Tauri.
///
/// This struct holds senders for all actor channels, allowing commands
/// to communicate with background actors.
pub struct ActorHandles {
    pub vector_tx: mpsc::Sender<VectorMsg>,
    pub foundry_tx: mpsc::Sender<FoundryMsg>,
    pub rag_tx: mpsc::Sender<RagMsg>,
    pub mcp_host_tx: mpsc::Sender<McpHostMsg>,
    pub python_tx: mpsc::Sender<PythonMsg>,
    pub database_toolbox_tx: mpsc::Sender<DatabaseToolboxMsg>,
    pub schema_tx: mpsc::Sender<SchemaVectorMsg>,
    /// Startup coordinator for frontend handshake
    pub startup_tx: mpsc::Sender<StartupMsg>,
    #[allow(dead_code)]
    pub logging_persistence: Arc<LoggingPersistence>,
    /// GPU resource guard for serializing GPU operations (LLM inference, embeddings)
    pub gpu_guard: Arc<GpuResourceGuard>,
}

/// Shared tool registry state
pub struct ToolRegistryState {
    pub registry: SharedToolRegistry,
}

/// Shared embedding models for vector operations.
///
/// We maintain two separate embedding models to avoid GPU memory contention:
/// - GPU model: Used for background RAG indexing (CoreML on Mac, CUDA on Windows)
/// - CPU model: Used for search during chat (avoids evicting the LLM from GPU)
pub struct EmbeddingModelState {
    /// GPU-accelerated model for background RAG document indexing.
    /// Uses CoreML on macOS, CUDA/DirectML on Windows.
    pub gpu_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
    /// CPU-only model for search operations during active chat.
    /// Avoids GPU contention that would evict the pre-warmed LLM.
    pub cpu_model: Arc<RwLock<Option<Arc<TextEmbedding>>>>,
}

/// Shared settings state
pub struct SettingsState {
    pub settings: Arc<RwLock<AppSettings>>,
}

/// Shared settings state machine (Tier 1 of the three-tier hierarchy)
pub struct SettingsStateMachineState {
    pub machine: Arc<RwLock<SettingsStateMachine>>,
}

/// Shared state for persistent logging of prompts and tools to avoid noise
pub struct LoggingPersistence {
    pub last_logged_system_prompt: Arc<RwLock<Option<String>>>,
    pub last_logged_tools_json: Arc<RwLock<Option<String>>>,
}

impl Default for LoggingPersistence {
    fn default() -> Self {
        Self {
            last_logged_system_prompt: Arc::new(RwLock::new(None)),
            last_logged_tools_json: Arc::new(RwLock::new(None)),
        }
    }
}

/// Pending tool approvals state
pub struct ToolApprovalState {
    pub pending: PendingApprovals,
}

/// Cancellation state for stream abort
pub struct CancellationState {
    /// Current generation's cancel signal
    pub cancel_signal: Arc<RwLock<Option<tokio::sync::watch::Sender<bool>>>>,
    /// Current generation ID for matching
    pub current_generation_id: Arc<RwLock<u32>>,
}

/// Progress tracking for a single turn in the chat
#[derive(Clone, Debug, Default, Serialize)]
pub struct TurnProgress {
    /// Whether a turn is actively being processed
    pub active: bool,
    pub chat_id: Option<String>,
    pub generation_id: u32,
    pub assistant_response: String,
    pub last_token_index: usize,
    pub finished: bool,
    pub had_tool_calls: bool,
    pub timestamp_ms: u128,
}

/// Event payload for system prompt updates
#[derive(Clone, Debug, Serialize)]
pub struct SystemPromptEvent {
    pub chat_id: String,
    pub generation_id: u32,
    pub prompt: String,
}

/// Tracks the latest turn progress for reconnect/replay
pub struct TurnTrackerState {
    pub progress: Arc<RwLock<TurnProgress>>,
}

/// Heartbeat state for monitoring frontend responsiveness
#[derive(Clone)]
pub struct HeartbeatState {
    pub last_frontend_beat: Arc<RwLock<Option<Instant>>>,
    pub logged_unresponsive: Arc<RwLock<bool>>,
    pub logged_never_seen: Arc<RwLock<bool>>,
    pub start_instant: Instant,
}

impl Default for HeartbeatState {
    fn default() -> Self {
        Self {
            last_frontend_beat: Arc::new(RwLock::new(None)),
            logged_unresponsive: Arc::new(RwLock::new(false)),
            logged_never_seen: Arc::new(RwLock::new(false)),
            start_instant: Instant::now(),
        }
    }
}

/// CLI launch overrides (non-persistent)
#[derive(Debug, Clone, Default)]
pub struct LaunchOverrides {
    pub model: Option<String>,
    pub initial_prompt: Option<String>,
    /// Tabular data files to attach for Python analysis (from --table-file)
    pub table_files: Vec<String>,
}

/// Global launch configuration state
pub struct LaunchConfigState {
    pub tool_filter: ToolLaunchFilter,
    pub launch_overrides: LaunchOverrides,
}
