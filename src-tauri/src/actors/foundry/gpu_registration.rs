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
        EpFailureKind::Unknown => "unknown error",
    }
}

/// Register providers once: emit progress, record diagnostics and the test-state phase.
pub async fn run_registration(sdk: &Arc<SdkBackend>, app: &AppHandle, attempt: u32) -> EpRegistrationSummary {
    let cancel = super::ep_registration_cancel_flag();
    cancel.store(false, Ordering::SeqCst);
    test_state::record(Phase::EpRegistration, test_state::status::STARTED, &format!("attempt {attempt}"));

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
        attempt += 1;
        summary = run_registration(sdk, app, attempt).await;
    }
    summary
}

/// Start a background retry. Returns `false` if one is already running or no SDK backend exists.
pub fn spawn_retry(app: AppHandle) -> bool {
    use tauri::Manager;
    let Some(sdk) = sdk() else {
        return false;
    };
    let Some(foundry_tx) = app.try_state::<crate::app_state::ActorHandles>().map(|h| h.foundry_tx.clone()) else {
        return false;
    };
    if RETRY_RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    tauri::async_runtime::spawn(async move {
        let summary = retry_with_backoff(&sdk, &app, &RETRY_DELAYS).await;
        if !summary.registered.is_empty() {
            // The catalog gained GPU variants; make the actor re-read it.
            let (tx, rx) = tokio::sync::oneshot::channel();
            if foundry_tx
                .send(crate::protocol::FoundryMsg::RefreshConnectionInfo { respond_to: tx })
                .await
                .is_ok()
            {
                let _ = rx.await;
            }
            // Repeat `done` now that the actor's catalog is current: a listener that re-reads
            // the model list on `done` would otherwise have read it before the refresh.
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
        }
        RETRY_RUNNING.store(false, Ordering::SeqCst);
    });
    true
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
}
