//! Startup Coordinator Actor
//!
//! Coordinates the application startup sequence and tracks subsystem status.
//! This actor is the central point for:
//! - Tracking overall startup state
//! - Receiving status updates from other actors
//! - Handling the frontend handshake protocol
//! - Emitting startup progress events

use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

use crate::protocol::{
    ModelInfo, ModelState, ResourceStatus, StartupProgressEvent, StartupSnapshot, StartupState,
    SubsystemStatus,
};

/// Messages for the Startup Coordinator Actor
pub enum StartupMsg {
    /// Report status update from Foundry actor
    ReportFoundryStatus {
        status: ResourceStatus,
        model_state: Option<ModelState>,
        available_models: Option<Vec<String>>,
        model_info: Option<Vec<ModelInfo>>,
        current_model: Option<String>,
    },
    /// Report status update from embedding initialization
    ReportEmbeddingStatus { status: ResourceStatus },
    /// Report status update from MCP host
    ReportMcpStatus { status: ResourceStatus },
    /// Report settings loaded
    ReportSettingsStatus { status: ResourceStatus },
    /// Frontend signals it's ready and requests current state snapshot
    FrontendReady {
        respond_to: oneshot::Sender<StartupSnapshot>,
    },
    /// Get current startup state (for diagnostics)
    GetStartupState {
        respond_to: oneshot::Sender<StartupState>,
    },
    /// Get full startup snapshot
    GetSnapshot {
        respond_to: oneshot::Sender<StartupSnapshot>,
    },
}

/// The Startup Coordinator Actor
pub struct StartupCoordinatorActor {
    /// Channel to receive messages
    msg_rx: mpsc::Receiver<StartupMsg>,
    /// App handle for emitting events
    app_handle: AppHandle,
    /// Current startup state
    startup_state: StartupState,
    /// Subsystem statuses
    subsystem_status: SubsystemStatus,
    /// Cached model state from Foundry
    model_state: ModelState,
    /// Cached available models
    available_models: Vec<String>,
    /// Cached model info
    model_info: Vec<ModelInfo>,
    /// Current model ID
    current_model: Option<String>,
    /// Whether frontend has connected
    frontend_connected: bool,
}

impl StartupCoordinatorActor {
    pub fn new(msg_rx: mpsc::Receiver<StartupMsg>, app_handle: AppHandle) -> Self {
        Self {
            msg_rx,
            app_handle,
            startup_state: StartupState::Initializing,
            subsystem_status: SubsystemStatus::default(),
            model_state: ModelState::Initializing,
            available_models: Vec::new(),
            model_info: Vec::new(),
            current_model: None,
            frontend_connected: false,
        }
    }

    /// Run the actor message loop
    pub async fn run(mut self) {
        println!("[StartupActor] Starting startup coordinator...");

        // Mark settings as ready (loaded synchronously at startup)
        self.subsystem_status.settings = ResourceStatus::Ready;
        self.transition_state(StartupState::ConnectingToFoundry);

        while let Some(msg) = self.msg_rx.recv().await {
            match msg {
                StartupMsg::ReportFoundryStatus {
                    status,
                    model_state,
                    available_models,
                    model_info,
                    current_model,
                } => {
                    self.subsystem_status.foundry_service = status.clone();

                    if let Some(ms) = model_state {
                        self.model_state = ms.clone();
                        // Update model subsystem status based on model state
                        self.subsystem_status.model = match &ms {
                            ModelState::Ready { .. } => ResourceStatus::Ready,
                            ModelState::Initializing => ResourceStatus::Initializing,
                            ModelState::LoadingModel { .. } => ResourceStatus::Initializing,
                            ModelState::SwitchingModel { .. } => ResourceStatus::Initializing,
                            ModelState::UnloadingModel { .. } => ResourceStatus::Initializing,
                            ModelState::Error { message, .. } => ResourceStatus::Failed {
                                message: message.clone(),
                            },
                            ModelState::ServiceUnavailable { message } => ResourceStatus::Failed {
                                message: message.clone(),
                            },
                            ModelState::ServiceRestarting => ResourceStatus::Initializing,
                            ModelState::Reconnecting => ResourceStatus::Initializing,
                        };
                    }

                    if let Some(models) = available_models {
                        self.available_models = models;
                    }

                    if let Some(info) = model_info {
                        self.model_info = info;
                    }

                    if let Some(model) = current_model {
                        self.current_model = Some(model);
                    }

                    // Check if we should transition to AwaitingFrontend
                    self.check_ready_for_frontend();
                    self.emit_progress("Foundry status updated");
                }

                StartupMsg::ReportEmbeddingStatus { status } => {
                    self.subsystem_status.cpu_embedding = status;
                    self.emit_progress("Embedding status updated");
                }

                StartupMsg::ReportMcpStatus { status } => {
                    self.subsystem_status.mcp_servers = status;
                    self.emit_progress("MCP status updated");
                }

                StartupMsg::ReportSettingsStatus { status } => {
                    self.subsystem_status.settings = status;
                    self.emit_progress("Settings status updated");
                }

                StartupMsg::FrontendReady { respond_to } => {
                    println!("[StartupActor] Frontend ready received");
                    println!("[StartupActor]   Current state: {:?}", self.startup_state);
                    println!("[StartupActor]   Foundry status: {:?}", self.subsystem_status.foundry_service);
                    println!("[StartupActor]   Model status: {:?}", self.subsystem_status.model);
                    println!("[StartupActor]   Available models: {:?}", self.available_models);
                    println!("[StartupActor]   Current model: {:?}", self.current_model);
                    
                    self.frontend_connected = true;

                    // Only transition to Ready if backend is ACTUALLY ready
                    // (foundry connected AND model selected)
                    let backend_ready = self.subsystem_status.foundry_service.is_ready()
                        && self.subsystem_status.model.is_ready();
                    
                    if backend_ready {
                        self.transition_state(StartupState::Ready);
                    } else {
                        println!("[StartupActor]   Backend not ready yet, staying in {:?}", self.startup_state);
                    }

                    let snapshot = self.create_snapshot();
                    println!("[StartupActor] Sending snapshot with startup_state={:?}", self.startup_state);
                    let _ = respond_to.send(snapshot);
                }

                StartupMsg::GetStartupState { respond_to } => {
                    let _ = respond_to.send(self.startup_state.clone());
                }

                StartupMsg::GetSnapshot { respond_to } => {
                    let snapshot = self.create_snapshot();
                    let _ = respond_to.send(snapshot);
                }
            }
        }

        println!("[StartupActor] Shutdown");
    }

    /// Check if backend is ready and we should await frontend
    fn check_ready_for_frontend(&mut self) {
        let foundry_ready = self.subsystem_status.foundry_service.is_ready();
        let model_ready = self.subsystem_status.model.is_ready();
        let backend_fully_ready = foundry_ready && model_ready;
        
        // Transition to AwaitingFrontend when foundry is connected AND model is ready
        if matches!(self.startup_state, StartupState::ConnectingToFoundry) && backend_fully_ready {
            if self.frontend_connected {
                // Frontend already connected, go straight to Ready
                self.transition_state(StartupState::Ready);
            } else {
                self.transition_state(StartupState::AwaitingFrontend);
            }
        }

        // If frontend already connected and we just became ready, transition to Ready
        if self.frontend_connected
            && matches!(self.startup_state, StartupState::AwaitingFrontend | StartupState::ConnectingToFoundry)
            && backend_fully_ready
        {
            self.transition_state(StartupState::Ready);
        }
    }

    /// Transition to a new startup state
    fn transition_state(&mut self, new_state: StartupState) {
        if self.startup_state != new_state {
            println!(
                "[StartupActor] Transition: {:?} -> {:?}",
                self.startup_state, new_state
            );
            self.startup_state = new_state;
            self.emit_progress("State changed");
        }
    }

    /// Create a snapshot of current state
    fn create_snapshot(&self) -> StartupSnapshot {
        StartupSnapshot::new(
            self.startup_state.clone(),
            self.subsystem_status.clone(),
            self.model_state.clone(),
            self.available_models.clone(),
            self.model_info.clone(),
            self.current_model.clone(),
        )
    }

    /// Emit startup progress event to frontend
    fn emit_progress(&self, message: &str) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let event = StartupProgressEvent {
            startup_state: self.startup_state.clone(),
            subsystem_status: self.subsystem_status.clone(),
            message: message.to_string(),
            timestamp,
        };

        if let Err(e) = self.app_handle.emit("startup-progress", &event) {
            eprintln!("[StartupActor] Failed to emit progress: {:?}", e);
        }
    }
}
