//! Startup Coordinator Actor
//!
//! Coordinates the application startup sequence and tracks subsystem status.
//! This actor is the central point for:
//! - Tracking overall startup state
//! - Receiving status updates from other actors
//! - Handling the frontend handshake protocol
//! - Emitting startup progress events

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

use crate::protocol::{
    ModelInfo, ModelState, ResourceStatus, StartupProgressEvent, StartupSnapshot, StartupState,
    SubsystemStatus,
};

/// A model failure (no model could be selected or loaded) that lasts this long ends the
/// "Connecting to Foundry" spinner: the app is waiting for something only the user can fix.
pub const MODEL_FAILURE_GRACE: Duration = Duration::from_secs(15);
/// Longest GPU provider registration allowed before the engine carries on without it. The
/// start-up deadline must outlast it: a slow first run (about 1.5 GB of providers) is not a failure.
pub const EP_REGISTRATION_LIMIT_SECS: u64 = 900;
/// Start-up that makes no progress for this long (no status report from the engine, the search
/// model, MCP, or the GPU provider download) is reported as failed instead of spinning for ever.
/// Longer than `EP_REGISTRATION_LIMIT_SECS` so a legitimate slow registration never trips it.
pub const STARTUP_STALL_DEADLINE: Duration = Duration::from_secs(EP_REGISTRATION_LIMIT_SECS + 300);
/// Absolute ceiling for a start-up that keeps reporting progress but never finishes.
pub const STARTUP_HARD_CEILING: Duration = Duration::from_secs(2 * 60 * 60);

const _: () = assert!(STARTUP_STALL_DEADLINE.as_secs() > EP_REGISTRATION_LIMIT_SECS);

/// Decides whether start-up should be declared failed. Pure so the rules are tested.
/// `model_failure` is the message of a failed model subsystem and how long it has been failed; progress
/// reported since then (a download moving) holds the verdict back.
/// `since_progress` is the time since anything last reported progress; `elapsed` is the time since
/// launch. Only the model (or engine) failing, a stall, or the hard ceiling fail start-up: a missing
/// search model or a slow GPU setup never does.
pub fn startup_verdict(
    connecting: bool,
    model_failure: Option<(&str, Duration)>,
    since_progress: Duration,
    elapsed: Duration,
) -> Option<String> {
    if !connecting {
        return None;
    }
    if let Some((message, failed_for)) = model_failure {
        // The gateway gives up connecting after about 45 s, but a first-run download keeps going and keeps
        // reporting; only a failure with no progress behind it for the grace period ends start-up.
        if failed_for >= MODEL_FAILURE_GRACE && since_progress >= MODEL_FAILURE_GRACE {
            return Some(message.to_string());
        }
    }
    if since_progress >= STARTUP_STALL_DEADLINE {
        return Some(format!(
            "Plugable Chat made no progress starting for {} minutes. Use Repair engine in Settings > Models, or restart the app.",
            since_progress.as_secs() / 60
        ));
    }
    if elapsed >= STARTUP_HARD_CEILING {
        return Some(format!(
            "Plugable Chat did not finish starting after {} minutes. Use Repair engine in Settings > Models, or restart the app.",
            elapsed.as_secs() / 60
        ));
    }
    None
}

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
    /// Something long-running (GPU provider download) is still making progress; keeps the
    /// stall deadline from firing. Carries no state change.
    ReportProgressHeartbeat,
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
    started: Instant,
    /// Last time any subsystem reported progress (see `startup_verdict`).
    last_progress: Instant,
    /// When the model subsystem last became Failed (cleared when it recovers).
    model_failed_since: Option<Instant>,
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
            started: Instant::now(),
            last_progress: Instant::now(),
            model_failed_since: None,
        }
    }

    /// Run the actor message loop
    pub async fn run(mut self) {
        println!("[StartupActor] Starting startup coordinator...");

        // Mark settings as ready (loaded synchronously at startup)
        self.subsystem_status.settings = ResourceStatus::Ready;
        self.transition_state(StartupState::ConnectingToFoundry);

        let mut watchdog = tokio::time::interval(Duration::from_secs(5));
        loop {
            let msg = tokio::select! {
                m = self.msg_rx.recv() => match m {
                    Some(m) => m,
                    None => break,
                },
                _ = watchdog.tick() => {
                    self.check_deadline();
                    continue;
                }
            };
            if matches!(
                msg,
                StartupMsg::ReportFoundryStatus { .. }
                    | StartupMsg::ReportEmbeddingStatus { .. }
                    | StartupMsg::ReportMcpStatus { .. }
                    | StartupMsg::ReportProgressHeartbeat
            ) {
                self.last_progress = Instant::now();
            }
            match msg {
                StartupMsg::ReportProgressHeartbeat => {}

                StartupMsg::ReportFoundryStatus {
                    status,
                    model_state,
                    available_models,
                    model_info,
                    current_model,
                } => {
                    self.subsystem_status.foundry_service = status.clone();
                    let prior_model_failed = self.subsystem_status.model.is_failed();

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

                    match (&self.subsystem_status.model, prior_model_failed) {
                        (m, false) if m.is_failed() => self.model_failed_since = Some(Instant::now()),
                        (m, _) if !m.is_failed() => self.model_failed_since = None,
                        _ => {}
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
        
        // Transition to AwaitingFrontend when foundry is connected AND model is ready.
        // A start-up that was declared failed recovers when the missing piece arrives.
        if matches!(self.startup_state, StartupState::ConnectingToFoundry | StartupState::Failed { .. }) && backend_fully_ready {
            if self.frontend_connected {
                // Frontend already connected, go straight to Ready
                self.transition_state(StartupState::Ready);
            } else {
                self.transition_state(StartupState::AwaitingFrontend);
            }
        }

        // If frontend already connected and we just became ready, transition to Ready
        if self.frontend_connected
            && matches!(self.startup_state, StartupState::AwaitingFrontend | StartupState::ConnectingToFoundry | StartupState::Failed { .. })
            && backend_fully_ready
        {
            self.transition_state(StartupState::Ready);
        }
    }

    /// Ends the spinner when start-up cannot finish on its own (see `startup_verdict`).
    fn check_deadline(&mut self) {
        let failure = match &self.subsystem_status.model {
            ResourceStatus::Failed { message } => self.model_failed_since.map(|t| (message.clone(), t.elapsed())),
            _ => None,
        };
        let connecting = matches!(self.startup_state, StartupState::ConnectingToFoundry | StartupState::Initializing);
        if let Some(message) =
            startup_verdict(connecting, failure.as_ref().map(|(m, d)| (m.as_str(), *d)), self.last_progress.elapsed(), self.started.elapsed())
        {
            self.transition_state(StartupState::Failed { message });
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

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH: Duration = Duration::from_secs(1);

    #[test]
    fn a_model_failure_ends_the_spinner_after_the_grace_period() {
        let m = Some(("No compatible model available.", Duration::from_secs(20)));
        let quiet = MODEL_FAILURE_GRACE + Duration::from_secs(5);
        assert_eq!(startup_verdict(true, m, quiet, Duration::from_secs(30)).as_deref(), Some("No compatible model available."));
        let fresh = Some(("x", Duration::from_secs(3)));
        assert_eq!(startup_verdict(true, fresh, quiet, Duration::from_secs(30)), None);
    }

    // rc18 (Bob, Pat): the gateway gave up connecting after about 45 s and the red "Startup failed (1/2)"
    // followed 15 s later, while the first-run model download was still moving.
    #[test]
    fn a_model_failure_is_not_a_start_up_failure_while_a_download_is_still_reporting_progress() {
        let failed_a_minute = Some(("model not ready", Duration::from_secs(60)));
        assert_eq!(startup_verdict(true, failed_a_minute, Duration::from_secs(2), Duration::from_secs(120)), None);
        // Progress that stops brings the verdict back after the same grace period.
        assert_eq!(
            startup_verdict(true, failed_a_minute, MODEL_FAILURE_GRACE, Duration::from_secs(120)).as_deref(),
            Some("model not ready")
        );
    }

    #[test]
    fn a_start_up_that_stops_making_progress_is_reported_at_the_stall_deadline() {
        let just_before = STARTUP_STALL_DEADLINE - Duration::from_secs(1);
        assert_eq!(startup_verdict(true, None, just_before, just_before), None);
        let v = startup_verdict(true, None, STARTUP_STALL_DEADLINE, STARTUP_STALL_DEADLINE).unwrap();
        assert!(v.contains("20 minutes") && v.contains("Repair"), "{v}");
    }

    // rc16 (Bob): a 453 s GPU provider registration plus a 4 GB model download is slow, not failed.
    #[test]
    fn a_slow_gpu_registration_that_keeps_reporting_progress_never_fails_start_up() {
        // 14 minutes in (inside the 15 minute registration limit), progress seen a moment ago.
        assert_eq!(startup_verdict(true, None, FRESH, Duration::from_secs(14 * 60)), None);
        // Even past the registration limit, a start-up still reporting progress is left alone.
        assert_eq!(startup_verdict(true, None, FRESH, Duration::from_secs(EP_REGISTRATION_LIMIT_SECS + 60)), None);
        // ...and silence shorter than the registration limit never trips the stall deadline.
        assert_eq!(startup_verdict(true, None, Duration::from_secs(EP_REGISTRATION_LIMIT_SECS), Duration::from_secs(1500)), None);
    }

    #[test]
    fn a_missing_search_model_is_not_a_start_up_failure() {
        // The verdict only sees the model subsystem; the embedding status is not an input at all.
        assert_eq!(startup_verdict(true, None, FRESH, Duration::from_secs(700)), None);
    }

    #[test]
    fn a_start_up_that_never_finishes_is_reported_at_the_hard_ceiling() {
        assert_eq!(startup_verdict(true, None, FRESH, STARTUP_HARD_CEILING - Duration::from_secs(1)), None);
        let v = startup_verdict(true, None, FRESH, STARTUP_HARD_CEILING).unwrap();
        assert!(v.contains("120 minutes"), "{v}");
    }

    #[test]
    fn nothing_is_declared_once_start_up_has_moved_on() {
        let m = Some(("x", Duration::from_secs(99)));
        assert_eq!(startup_verdict(false, m, Duration::from_secs(9999), Duration::from_secs(9999)), None);
    }
}
