//! GPU execution-provider registration: the start-up step and the in-session retry.
//!
//! One code path serves both so the UI sees the same `ep-registration-progress` events and the
//! diagnostics (`gpu_diagnostics`) are updated the same way. A failed first run used to leave
//! the app on CPU models until restart; `retry_with_backoff` lets the user (or a button)
//! try again without restarting.

use super::backend::sdk::{EpRegistrationSummary, SdkBackend};
use crate::gpu_diagnostics::{self, EpFailureKind};
use crate::test_state::{self, Phase};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Waits between retry attempts: network trouble is usually a captive portal, a proxy that is
/// still authenticating, or a flaky link, so a short wait first and longer ones after.
pub const RETRY_DELAYS: [Duration; 3] = [Duration::from_secs(5), Duration::from_secs(20), Duration::from_secs(60)];

static SDK: OnceLock<Arc<SdkBackend>> = OnceLock::new();
static RETRY_RUNNING: AtomicBool = AtomicBool::new(false);
/// Set once the start-up registration finished, so the hardware watcher never races it.
static STARTUP_REGISTRATION_DONE: AtomicBool = AtomicBool::new(false);

pub(crate) fn mark_startup_registration_done() {
    STARTUP_REGISTRATION_DONE.store(true, Ordering::SeqCst);
}

/// Called once by the gateway actor so commands can reach the same SDK instance.
pub(crate) fn remember_sdk(sdk: &Arc<SdkBackend>) {
    let _ = SDK.set(Arc::clone(sdk));
}

pub(crate) fn sdk() -> Option<Arc<SdkBackend>> {
    SDK.get().cloned()
}

/// True when another attempt could change the outcome without the user doing anything.
pub fn should_retry(summary: &EpRegistrationSummary) -> bool {
    !summary.cancelled && summary.failures.iter().any(|f| f.kind.is_retryable())
}

/// One-line status for the UI. Empty when there was nothing to register.
pub fn done_message(summary: &EpRegistrationSummary) -> String {
    if summary.cancelled {
        return "GPU acceleration setup cancelled; using CPU models.".to_string();
    }
    if summary.registered.is_empty() && summary.failed.is_empty() && summary.failures.is_empty() {
        return String::new();
    }
    if summary.failures.is_empty() {
        return format!("GPU acceleration ready ({}).", summary.registered.join(", "));
    }
    let reasons: Vec<String> = summary
        .failures
        .iter()
        .map(|f| format!("{} ({})", f.ep, kind_label(f.kind)))
        .collect();
    format!(
        "GPU acceleration ready: {}; could not set up: {}.",
        if summary.registered.is_empty() { "none".to_string() } else { summary.registered.join(", ") },
        reasons.join(", ")
    )
}

pub fn kind_label(kind: EpFailureKind) -> &'static str {
    match kind {
        EpFailureKind::Network => "network problem",
        EpFailureKind::Driver => "graphics driver too old or missing",
        EpFailureKind::MissingDll => "a required library is missing",
        EpFailureKind::Cancelled => "cancelled",
        EpFailureKind::Unsupported => "not supported by this hardware",
        EpFailureKind::EngineDeclined => "the model engine declined to register it",
        EpFailureKind::Unknown => "unknown error",
    }
}

/// Register providers once: emit progress, record diagnostics and the test-state phase.
pub async fn run_registration(sdk: &Arc<SdkBackend>, app: &AppHandle, attempt: u32) -> EpRegistrationSummary {
    let cancel = super::ep_registration_cancel_flag();
    cancel.store(false, Ordering::SeqCst);
    test_state::record(Phase::EpRegistration, test_state::status::STARTED, &format!("attempt {attempt}"));
    crate::preflight::set_registering(true);

    let progress_handle = app.clone();
    let summary = sdk
        .register_execution_providers(
            move |ep, percent| {
                let _ = progress_handle.emit(
                    "ep-registration-progress",
                    json!({ "phase": "downloading", "ep": ep, "percent": percent }),
                );
            },
            cancel,
        )
        .await;

    let mut all_registered = summary.already_registered.clone();
    all_registered.extend(summary.registered.iter().cloned());
    gpu_diagnostics::record_attempt(&all_registered, &summary.failures);
    crate::preflight::set_registering(false);

    let message = done_message(&summary);
    let _ = app.emit(
        "ep-registration-progress",
        json!({
            "phase": "done",
            "message": message,
            "registered": summary.registered,
            "failed": summary.failed,
            "failures": summary.failures,
            "attempt": attempt,
            "seconds": summary.seconds,
        }),
    );

    let (status, detail) = if summary.failures.is_empty() {
        (test_state::status::OK, message)
    } else {
        let detail = summary
            .failures
            .iter()
            .map(|f| format!("{} [{:?}] {}", f.ep, f.kind, f.message))
            .collect::<Vec<_>>()
            .join("; ");
        (test_state::status::FAILED, detail)
    };
    test_state::record(Phase::EpRegistration, status, &detail);
    summary
}

/// Retry until every failure is permanent, the user cancels, or the delays run out.
pub async fn retry_with_backoff(
    sdk: &Arc<SdkBackend>,
    app: &AppHandle,
    delays: &[Duration],
) -> EpRegistrationSummary {
    // A retry often follows installing a driver or the VC++ runtime.
    gpu_diagnostics::invalidate_probe();
    let mut attempt = 1;
    let mut summary = run_registration(sdk, app, attempt).await;
    for delay in delays {
        if !should_retry(&summary) {
            break;
        }
        tokio::time::sleep(*delay).await;
        // No point downloading with the network down; the watcher tries again once it is back.
        if !online().await {
            crate::app_log::log("gpu", "GPU setup retry skipped: no network");
            continue;
        }
        attempt += 1;
        summary = run_registration(sdk, app, attempt).await;
    }
    summary
}

/// Clears the "retry running" flag on every exit path, including a panic in the retry.
struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        RETRY_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// What a retry request did.
pub struct RetryOutcome {
    /// False when another retry was already running (the caller should say so, not pretend).
    pub started: bool,
    pub summary: Option<EpRegistrationSummary>,
}

/// Run a full GPU setup attempt now and wait for it: register providers (with backoff for network
/// trouble), make the model actor re-read its catalog, and ask Windows about the hardware again.
///
/// The Retry button used to return before any of this finished and refresh the model list at once,
/// so the list was always read too early. The catalog is refreshed whether or not a provider
/// newly registered: a previous run's providers may already be in place with the list stale.
pub async fn retry_now(app: AppHandle) -> RetryOutcome {
    use tauri::Manager;
    let Some(sdk) = sdk() else {
        return RetryOutcome { started: false, summary: None };
    };
    if RETRY_RUNNING.swap(true, Ordering::SeqCst) {
        return RetryOutcome { started: false, summary: None };
    }
    let _running = RunningGuard;
    crate::preflight::mark_retry_requested();
    crate::app_log::log("gpu", "GPU setup retry requested");

    let summary = retry_with_backoff(&sdk, &app, &RETRY_DELAYS).await;

    if let Some(foundry_tx) = app.try_state::<crate::app_state::ActorHandles>().map(|h| h.foundry_tx.clone()) {
        // The catalog may have gained GPU variants; make the actor re-read it.
        let (tx, rx) = tokio::sync::oneshot::channel();
        if foundry_tx.send(crate::protocol::FoundryMsg::RefreshConnectionInfo { respond_to: tx }).await.is_ok() {
            let _ = rx.await;
        }
        // Read the catalog now (this records how many GPU models it lists), so the assessment
        // below is built from the list as it stands after this attempt, not from the previous one.
        let (tx, rx) = tokio::sync::oneshot::channel();
        if foundry_tx.send(crate::protocol::FoundryMsg::GetCatalogModels { respond_to: tx }).await.is_ok() {
            let _ = rx.await;
        }
    }
    // Re-ask Windows and rebuild the assessment now that the catalog is current.
    let _ = tokio::task::spawn_blocking(crate::preflight::refresh_blocking).await;

    // Tell listeners the model list is current: they re-read it on `done`.
    let _ = app.emit(
        "ep-registration-progress",
        json!({
            "phase": "done",
            "message": done_message(&summary),
            "registered": summary.registered,
            "failed": summary.failed,
            "failures": summary.failures,
            "seconds": summary.seconds,
            "refreshed": true,
        }),
    );
    RetryOutcome { started: true, summary: Some(summary) }
}

/// Start a retry in the background (start-up path). Returns `false` if one is already running or
/// no SDK backend exists.
pub fn spawn_retry(app: AppHandle) -> bool {
    if sdk().is_none() || RETRY_RUNNING.load(Ordering::SeqCst) {
        return false;
    }
    tauri::async_runtime::spawn(async move {
        let _ = retry_now(app).await;
    });
    true
}

/// True when a network connection can be made. Uses literal addresses so a dead DNS cannot stall it.
pub async fn online() -> bool {
    if crate::engine_health::test_fault("offline") {
        return false;
    }
    tokio::task::spawn_blocking(|| {
        use std::net::{SocketAddr, TcpStream};
        let timeout = Duration::from_secs(2);
        ["1.1.1.1:443", "8.8.8.8:443"]
            .iter()
            .filter_map(|a| a.parse::<SocketAddr>().ok())
            .any(|a| TcpStream::connect_timeout(&a, timeout).is_ok())
    })
    .await
    .unwrap_or(false)
}

/// Whether the background watcher may start GPU setup now. It downloads and re-registers native
/// providers, so it never runs while a chat is being answered, while GPU acceleration is switched
/// off, or when there is no network (a drop mid-session is exactly when the engine must be left
/// alone; rc15 crashed inside the WebGPU provider after an unplugged cable).
pub fn watcher_may_act(online: bool, chatting: bool, gpu_off: bool) -> bool {
    online && !chatting && !gpu_off
}

/// Polling interval while the machine might still change (first half hour), then relaxed.
const WATCH_FAST: Duration = Duration::from_secs(30);
const WATCH_SLOW: Duration = Duration::from_secs(300);
const WATCH_FAST_TICKS: u32 = 60;
/// While degraded and a retry could help, try again every this many ticks (5 minutes when fast).
const WATCH_RETRY_EVERY_TICKS: u32 = 10;

/// Why the watcher decided to run a GPU setup attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchDecision {
    Idle,
    /// A display adapter appeared, disappeared or changed state (an eGPU connected late, a
    /// driver finished installing, the machine woke from sleep).
    HardwareChanged,
    /// Still degraded and a retry may help: try again on a slow cadence.
    PeriodicRetry,
}

pub fn watch_decision(
    previous: &[String],
    current: &[String],
    assessment: &crate::gpu_assessment::GpuAssessment,
    ticks_since_retry: u32,
) -> WatchDecision {
    use crate::gpu_assessment::GpuStateKind as K;
    if previous != current {
        return WatchDecision::HardwareChanged;
    }
    let degraded = matches!(
        assessment.state,
        K::HardwareNotUsed | K::AcceleratedNoModels | K::EngineUnavailable | K::CatalogFailed | K::ScanFailed
    );
    if degraded && assessment.retry_may_help && ticks_since_retry >= WATCH_RETRY_EVERY_TICKS {
        return WatchDecision::PeriodicRetry;
    }
    WatchDecision::Idle
}

/// Watch for hardware coming and going, and retry GPU setup when that could change the outcome.
/// Without this an eGPU authorised after launch was never noticed until the user pressed Retry.
pub fn spawn_hardware_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick: u32 = 0;
        let mut since_retry: u32 = 0;
        // The scan taken at launch is the baseline, so hardware that shows up before the first
        // poll still counts as a change.
        let mut previous: Option<Vec<String>> = crate::preflight::adapter_fingerprint();
        loop {
            tokio::time::sleep(if tick < WATCH_FAST_TICKS { WATCH_FAST } else { WATCH_SLOW }).await;
            tick += 1;
            since_retry += 1;
            if !STARTUP_REGISTRATION_DONE.load(Ordering::SeqCst) || RETRY_RUNNING.load(Ordering::SeqCst) {
                continue;
            }
            let Ok(assessment) = tokio::task::spawn_blocking(crate::preflight::refresh_blocking).await else { continue };
            let Some(current) = crate::preflight::adapter_fingerprint() else { continue };
            let Some(prev) = previous.replace(current.clone()) else { continue };
            let decision = watch_decision(&prev, &current, &assessment, since_retry);
            if decision != WatchDecision::Idle {
                if !watcher_may_act(online().await, crate::run_state::is_chatting(), crate::engine_health::gpu_off()) {
                    // Keep the old fingerprint so the change is still seen on a later tick.
                    previous = Some(prev);
                    continue;
                }
                crate::app_log::log("gpu", &format!("hardware watcher: {decision:?}, retrying GPU setup"));
                since_retry = 0;
                let _ = retry_now(app.clone()).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_diagnostics::make_failure;

    fn summary(registered: &[&str], failures: Vec<crate::gpu_diagnostics::EpFailure>) -> EpRegistrationSummary {
        EpRegistrationSummary {
            registered: registered.iter().map(|s| s.to_string()).collect(),
            failed: failures.iter().map(|f| f.ep.clone()).collect(),
            failures,
            ..Default::default()
        }
    }

    #[test]
    fn retries_network_failures_but_not_driver_or_missing_dll() {
        assert!(should_retry(&summary(&[], vec![make_failure("CUDA", "HTTP 503")])));
        assert!(should_retry(&summary(&[], vec![make_failure("CUDA", "odd")])));
        assert!(!should_retry(&summary(&[], vec![make_failure("CUDA", "os error 126")])));
        assert!(!should_retry(&summary(&[], vec![make_failure("CUDA", "driver too old")])));
        assert!(!should_retry(&summary(&["CUDA"], vec![])));
    }

    #[test]
    fn never_retries_after_a_cancel() {
        let mut s = summary(&[], vec![make_failure("CUDA", "HTTP 503")]);
        s.cancelled = true;
        assert!(!should_retry(&s));
        assert!(done_message(&s).contains("cancelled"));
    }

    #[test]
    fn done_message_names_the_cause_of_each_failure() {
        let s = summary(&["CPU"], vec![make_failure("CUDA", "os error 126")]);
        let m = done_message(&s);
        assert!(m.contains("CUDA") && m.contains("library is missing"), "{m}");
    }

    #[test]
    fn done_message_is_empty_when_nothing_happened() {
        assert_eq!(done_message(&EpRegistrationSummary::default()), "");
    }

    #[test]
    fn done_message_reports_success() {
        assert!(done_message(&summary(&["CUDA"], vec![])).starts_with("GPU acceleration ready (CUDA)"));
    }

    fn assessment(state: crate::gpu_assessment::GpuStateKind, retry: bool) -> crate::gpu_assessment::GpuAssessment {
        crate::gpu_assessment::GpuAssessment { state, headline: String::new(), detail: vec![], hints: vec![], retry_may_help: retry }
    }

    // R2 (Pat): an external GPU that Windows lists after the app has started.
    #[test]
    fn r2_a_late_adapter_triggers_a_retry() {
        use crate::gpu_assessment::GpuStateKind::*;
        let before = vec!["Intel UHD|PCI|0".to_string()];
        let after = vec!["Intel UHD|PCI|0".to_string(), "NVIDIA RTX|PCI|0".to_string()];
        assert_eq!(watch_decision(&before, &after, &assessment(NoHardware, false), 0), WatchDecision::HardwareChanged);
        // the same hardware, working, needs nothing
        assert_eq!(watch_decision(&after, &after, &assessment(Ready, false), 99), WatchDecision::Idle);
    }

    #[test]
    fn a_device_manager_problem_clearing_counts_as_a_change() {
        use crate::gpu_assessment::GpuStateKind::*;
        let broken = vec!["NVIDIA RTX|PCI|43".to_string()];
        let fixed = vec!["NVIDIA RTX|PCI|0".to_string()];
        assert_eq!(watch_decision(&broken, &fixed, &assessment(HardwareNotUsed, false), 0), WatchDecision::HardwareChanged);
    }

    #[test]
    fn the_watcher_leaves_the_engine_alone_when_offline_chatting_or_gpu_off() {
        assert!(watcher_may_act(true, false, false));
        assert!(!watcher_may_act(false, false, false));
        assert!(!watcher_may_act(true, true, false));
        assert!(!watcher_may_act(true, false, true));
    }

    #[test]
    fn a_degraded_state_is_retried_slowly_but_a_permanent_one_is_not() {
        use crate::gpu_assessment::GpuStateKind::*;
        let same = vec!["NVIDIA RTX|PCI|0".to_string()];
        assert_eq!(watch_decision(&same, &same, &assessment(HardwareNotUsed, true), 3), WatchDecision::Idle);
        assert_eq!(watch_decision(&same, &same, &assessment(HardwareNotUsed, true), WATCH_RETRY_EVERY_TICKS), WatchDecision::PeriodicRetry);
        // a missing driver needs the user, so polling the engine would only waste the network
        assert_eq!(watch_decision(&same, &same, &assessment(HardwareNotUsed, false), 99), WatchDecision::Idle);
        assert_eq!(watch_decision(&same, &same, &assessment(NoHardware, false), 99), WatchDecision::Idle);
    }
}
