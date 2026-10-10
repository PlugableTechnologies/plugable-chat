//! `--smoke`: run startup unattended and report whether it reached a usable state.
//!
//! The app starts normally (the window and the frontend run, which is what triggers the first
//! model download). This task watches the phases recorded in `test_state`, prints one JSON
//! summary line, and exits the process with 0 (ready) or 1 (failed or timed out). Clean-host
//! scripts read the same data from the `PLUGABLE_CHAT_TEST_STATE` file.

use crate::test_state::{self, Phase, SmokeOutcome};
use serde_json::json;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const POLL: Duration = Duration::from_millis(500);

/// Final summary: `{"smoke": "ok"|"failed", "detail": ..., "phases": {phase: status}}`.
pub fn summary_json(outcome: &SmokeOutcome, phases: &std::collections::HashMap<String, String>) -> String {
    let (verdict, detail) = match outcome {
        SmokeOutcome::Success => ("ok", String::new()),
        SmokeOutcome::Failure(why) => ("failed", why.clone()),
        SmokeOutcome::Waiting => ("failed", "timed out waiting for startup to finish".to_string()),
    };
    json!({ "smoke": verdict, "detail": detail, "phases": phases }).to_string()
}

pub fn exit_code(outcome: &SmokeOutcome) -> i32 {
    if matches!(outcome, SmokeOutcome::Success) { 0 } else { 1 }
}

/// Poll until the outcome is decided or `timeout` passes (reported as `Waiting`).
pub async fn wait_for_outcome(timeout: Duration, poll: Duration) -> SmokeOutcome {
    let started = Instant::now();
    loop {
        let outcome = test_state::smoke_outcome(&test_state::snapshot());
        if outcome != SmokeOutcome::Waiting || started.elapsed() >= timeout {
            return outcome;
        }
        tokio::time::sleep(poll).await;
    }
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let timeout = test_state::smoke_timeout();
        println!("[Smoke] waiting up to {}s for startup to finish", timeout.as_secs());
        let outcome = wait_for_outcome(timeout, POLL).await;

        let phases = test_state::snapshot();
        let summary = summary_json(&outcome, &phases);
        match &outcome {
            SmokeOutcome::Success => test_state::record(Phase::Ready, test_state::status::OK, "smoke passed"),
            SmokeOutcome::Failure(why) => test_state::record(Phase::Error, test_state::status::FAILED, why),
            SmokeOutcome::Waiting => test_state::record(Phase::Error, test_state::status::FAILED, "smoke timed out"),
        }
        println!("{summary}");
        crate::launch_marker::clear();
        app.exit(exit_code(&outcome));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn exit_code_is_zero_only_for_success() {
        assert_eq!(exit_code(&SmokeOutcome::Success), 0);
        assert_eq!(exit_code(&SmokeOutcome::Failure("x".into())), 1);
        assert_eq!(exit_code(&SmokeOutcome::Waiting), 1);
    }

    #[test]
    fn summary_is_one_json_line_with_verdict_and_phases() {
        let mut phases = HashMap::new();
        phases.insert("embedding".to_string(), "failed".to_string());
        let line = summary_json(&SmokeOutcome::Failure("embedding model failed to load".into()), &phases);
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["smoke"], "failed");
        assert_eq!(v["detail"], "embedding model failed to load");
        assert_eq!(v["phases"]["embedding"], "failed");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&summary_json(&SmokeOutcome::Success, &phases)).unwrap()["smoke"], "ok");
    }

    #[test]
    fn a_timeout_is_reported_as_a_failure_with_a_reason() {
        let v: serde_json::Value =
            serde_json::from_str(&summary_json(&SmokeOutcome::Waiting, &HashMap::new())).unwrap();
        assert_eq!(v["smoke"], "failed");
        assert!(v["detail"].as_str().unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn waiting_returns_waiting_at_the_deadline_without_hanging() {
        // Uses the process-global phase map: only this test (and others that never record
        // ready+embedding ok together) may run against it, so the outcome stays Waiting.
        let outcome = wait_for_outcome(Duration::from_millis(30), Duration::from_millis(5)).await;
        assert!(matches!(outcome, SmokeOutcome::Waiting | SmokeOutcome::Failure(_)));
    }
}
