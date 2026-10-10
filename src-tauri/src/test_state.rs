//! Machine-readable startup progress for automated clean-host testing.
//!
//! When `PLUGABLE_CHAT_TEST_STATE=<file>` is set, every startup phase appends one JSON line
//! `{"ts", "phase", "status", "detail"}` to that file, so a script can wait on and assert
//! against what the app actually did instead of reading screenshots. The latest status per
//! phase is also kept in memory, which is what `--smoke` waits on.
//!
//! Free of Tauri types so every branch is unit-testable on any OS.

use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const ENV_VAR: &str = "PLUGABLE_CHAT_TEST_STATE";
pub const SMOKE_TIMEOUT_ENV_VAR: &str = "PLUGABLE_CHAT_SMOKE_TIMEOUT_SECS";
/// Twenty minutes: a first run downloads about 1.5 GB of GPU components plus two models.
pub const DEFAULT_SMOKE_TIMEOUT_SECS: u64 = 20 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    EpRegistration,
    Embedding,
    ModelDownload,
    Toolbox,
    Ready,
    Error,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::EpRegistration => "ep-registration",
            Phase::Embedding => "embedding",
            Phase::ModelDownload => "model-download",
            Phase::Toolbox => "toolbox",
            Phase::Ready => "ready",
            Phase::Error => "error",
        }
    }
}

/// Statuses used by callers; kept as strings in the file so scripts need no enum.
pub mod status {
    pub const STARTED: &str = "started";
    pub const PROGRESS: &str = "progress";
    pub const OK: &str = "ok";
    pub const FAILED: &str = "failed";
    pub const SKIPPED: &str = "skipped";
}

fn latest() -> &'static Mutex<HashMap<String, String>> {
    static LATEST: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LATEST.get_or_init(Default::default)
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Default::default)
}

/// Render one state line (no trailing newline).
pub fn format_line(ts: &str, phase: &str, status: &str, detail: &str) -> String {
    json!({ "ts": ts, "phase": phase, "status": status, "detail": detail }).to_string()
}

/// Append one line to `path`, creating the file and its parent directory if needed.
pub fn append_line(path: &Path, phase: &str, status: &str, detail: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let line = format_line(&chrono::Utc::now().to_rfc3339(), phase, status, detail);
    let _guard = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

/// Record a phase transition. Always updates the in-memory view; writes the file only when
/// `PLUGABLE_CHAT_TEST_STATE` is set. Never fails the caller.
pub fn record(phase: Phase, status: &str, detail: &str) {
    if let Ok(mut map) = latest().lock() {
        map.insert(phase.as_str().to_string(), status.to_string());
    }
    if let Some(path) = std::env::var_os(ENV_VAR).filter(|p| !p.is_empty()) {
        if let Err(e) = append_line(Path::new(&path), phase.as_str(), status, detail) {
            eprintln!("[TestState] could not write {}: {e}", Path::new(&path).display());
        }
    }
}

/// Snapshot of the latest status per phase name.
pub fn snapshot() -> HashMap<String, String> {
    latest().lock().map(|m| m.clone()).unwrap_or_default()
}

/// What `--smoke` should do given the latest status of each phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmokeOutcome {
    Waiting,
    Success,
    Failure(String),
}

/// Decide whether the smoke run is finished.
///
/// A failed embedding model or a service that cannot start ends the run at once. A failed GPU
/// provider does not: CUDA is expected to be unavailable on a CPU-only host, and the state file
/// carries the detail for scripts that care. Success needs a ready model and a loaded embedder.
pub fn smoke_outcome(latest_by_phase: &HashMap<String, String>) -> SmokeOutcome {
    let get = |p: Phase| latest_by_phase.get(p.as_str()).map(String::as_str);
    if get(Phase::Embedding) == Some(status::FAILED) {
        return SmokeOutcome::Failure("embedding model failed to load".to_string());
    }
    if get(Phase::Error) == Some(status::FAILED) {
        return SmokeOutcome::Failure("service reported an unrecoverable error".to_string());
    }
    if get(Phase::Ready) == Some(status::OK) && get(Phase::Embedding) == Some(status::OK) {
        return SmokeOutcome::Success;
    }
    SmokeOutcome::Waiting
}

pub fn smoke_timeout_from(value: Option<&str>) -> std::time::Duration {
    let secs = value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_SMOKE_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

pub fn smoke_timeout() -> std::time::Duration {
    smoke_timeout_from(std::env::var(SMOKE_TIMEOUT_ENV_VAR).ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_line_writes_one_json_object_per_line_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.jsonl");
        append_line(&path, "embedding", "started", "downloading").unwrap();
        append_line(&path, "ready", "ok", "").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> =
            text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["phase"], "embedding");
        assert_eq!(lines[0]["status"], "started");
        assert_eq!(lines[0]["detail"], "downloading");
        assert!(lines[0]["ts"].as_str().unwrap().contains('T'));
        assert_eq!(lines[1]["phase"], "ready");
    }

    #[test]
    fn detail_with_quotes_and_newlines_stays_on_one_line() {
        let line = format_line("t", "error", "failed", "bad \"x\"\nsecond line");
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["detail"], "bad \"x\"\nsecond line");
    }

    #[test]
    fn phase_names_match_the_documented_contract() {
        let names: Vec<_> = [
            Phase::EpRegistration,
            Phase::Embedding,
            Phase::ModelDownload,
            Phase::Toolbox,
            Phase::Ready,
            Phase::Error,
        ]
        .iter()
        .map(|p| p.as_str())
        .collect();
        assert_eq!(
            names,
            ["ep-registration", "embedding", "model-download", "toolbox", "ready", "error"]
        );
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn smoke_waits_until_both_ready_and_embedding_are_ok() {
        assert_eq!(smoke_outcome(&map(&[])), SmokeOutcome::Waiting);
        assert_eq!(smoke_outcome(&map(&[("ready", "ok")])), SmokeOutcome::Waiting);
        assert_eq!(
            smoke_outcome(&map(&[("ready", "ok"), ("embedding", "ok")])),
            SmokeOutcome::Success
        );
    }

    #[test]
    fn smoke_fails_fast_on_embedding_or_service_error() {
        assert!(matches!(
            smoke_outcome(&map(&[("embedding", "failed")])),
            SmokeOutcome::Failure(_)
        ));
        assert!(matches!(
            smoke_outcome(&map(&[("error", "failed"), ("ready", "ok"), ("embedding", "ok")])),
            SmokeOutcome::Failure(_)
        ));
    }

    #[test]
    fn smoke_ignores_a_failed_gpu_provider() {
        assert_eq!(
            smoke_outcome(&map(&[("ep-registration", "failed"), ("ready", "ok"), ("embedding", "ok")])),
            SmokeOutcome::Success
        );
    }

    #[test]
    fn smoke_timeout_defaults_and_accepts_override() {
        assert_eq!(smoke_timeout_from(None).as_secs(), 1200);
        assert_eq!(smoke_timeout_from(Some("30")).as_secs(), 30);
        assert_eq!(smoke_timeout_from(Some("0")).as_secs(), 1200);
        assert_eq!(smoke_timeout_from(Some("junk")).as_secs(), 1200);
    }
}
