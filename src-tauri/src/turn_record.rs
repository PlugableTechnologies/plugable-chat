//! Machine-readable record of every chat turn, for automated testing of the app from outside.
//!
//! When `--reply-file` / `PLUGABLE_REPLY_FILE` is set, each finished turn appends one JSON line
//! with the question, the final reply, how the turn ended and a trace of the tool calls the model
//! made (which server and tool, arguments, a short result summary, timing). A harness can then
//! assert on what the app actually did instead of reading screenshots or the chat database.
//!
//! With `--exit-after-reply` the app exits once every expected prompt has a record. Exit codes:
//! 0 every turn ended `ok`, 1 a turn failed, 2 the launch model could not be used, 3 startup
//! failed, 4 the run timed out. Anything that stops the run before a turn can finish (launch
//! model problem, startup failure, timeout) writes a record too, so a harness never has to
//! guess from a missing file.
//!
//! The record and log types are free of Tauri types so every branch is unit-testable on any OS.

use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const REPLY_FILE_ENV_VAR: &str = "PLUGABLE_REPLY_FILE";
pub const REPLY_TIMEOUT_ENV_VAR: &str = "PLUGABLE_REPLY_TIMEOUT_SECS";
/// Thirty minutes: a first run can download models before the first prompt starts.
pub const DEFAULT_REPLY_TIMEOUT_SECS: u64 = 30 * 60;
/// Longest tool-result summary kept in a record (the full text is in the saved chat).
pub const RESULT_SUMMARY_MAX_CHARS: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Ok,
    Error,
    Cancelled,
    MaxIterations,
    ApprovalTimeout,
    LaunchModelError,
    StartupError,
    Timeout,
}

impl TurnOutcome {
    pub fn exit_code(self) -> i32 {
        match self {
            TurnOutcome::Ok => 0,
            TurnOutcome::Error | TurnOutcome::Cancelled | TurnOutcome::MaxIterations | TurnOutcome::ApprovalTimeout => 1,
            TurnOutcome::LaunchModelError => 2,
            TurnOutcome::StartupError => 3,
            TurnOutcome::Timeout => 4,
        }
    }

    /// Outcomes that end the whole run, not just one prompt.
    pub fn ends_run(self) -> bool {
        matches!(self, TurnOutcome::LaunchModelError | TurnOutcome::StartupError | TurnOutcome::Timeout)
    }
}

/// One tool call the model made during a turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolTraceEntry {
    pub server: String,
    pub tool: String,
    pub arguments: Value,
    pub result_summary: String,
    pub result_chars: usize,
    pub is_error: bool,
    pub duration_ms: u64,
}

impl ToolTraceEntry {
    pub fn new(server: &str, tool: &str, arguments: Value, result: &str, is_error: bool, duration: Duration) -> Self {
        Self {
            server: server.to_string(),
            tool: tool.to_string(),
            arguments,
            result_summary: summarize_result(result, RESULT_SUMMARY_MAX_CHARS),
            result_chars: result.chars().count(),
            is_error,
            duration_ms: duration.as_millis() as u64,
        }
    }
}

/// The head of a tool result, cut on a character boundary, with an ellipsis when cut.
pub fn summarize_result(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}…")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnRecord {
    /// Position of this record in the run, assigned by the log (0 for the first prompt).
    pub prompt_index: usize,
    pub question: Option<String>,
    pub reply: String,
    pub outcome: TurnOutcome,
    /// Why the run or turn ended abnormally; empty for `ok`.
    pub detail: String,
    pub model: Option<String>,
    pub chat_id: Option<String>,
    pub duration_ms: u64,
    pub tool_calls: Vec<ToolTraceEntry>,
}

impl TurnRecord {
    /// A record for something that stopped the run before any turn could finish.
    pub fn run_failure(outcome: TurnOutcome, detail: &str) -> Self {
        Self {
            prompt_index: 0,
            question: None,
            reply: String::new(),
            outcome,
            detail: detail.to_string(),
            model: None,
            chat_id: None,
            duration_ms: 0,
            tool_calls: Vec::new(),
        }
    }
}

/// Render one record line (no trailing newline).
pub fn format_record(record: &TurnRecord) -> String {
    serde_json::to_string(record).unwrap_or_else(|e| format!("{{\"outcome\":\"error\",\"detail\":\"could not serialize record: {e}\"}}"))
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Default::default)
}

/// Append one line to `path`, creating the file and its parent directory if needed.
pub fn append_record_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let _guard = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

/// The records of one launch: where they go, how many prompts are expected, and what ended each.
#[derive(Debug)]
pub struct ReplyLog {
    path: PathBuf,
    expected: usize,
    exit_after_reply: bool,
    outcomes: Vec<TurnOutcome>,
    finished: bool,
}

/// What the caller should do after a record was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    pub prompt_index: usize,
    /// Set when the run is over and the app was asked to exit afterwards.
    pub exit_code: Option<i32>,
}

impl ReplyLog {
    pub fn new(path: PathBuf, expected: usize, exit_after_reply: bool) -> Self {
        Self { path, expected: expected.max(1), exit_after_reply, outcomes: Vec::new(), finished: false }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Exit code for the records so far: the code of the first failing record, else 0.
    pub fn exit_code(&self) -> i32 {
        self.outcomes.iter().map(|o| o.exit_code()).find(|c| *c != 0).unwrap_or(0)
    }

    pub fn record(&mut self, mut record: TurnRecord) -> std::io::Result<Recorded> {
        record.prompt_index = self.outcomes.len();
        append_record_line(&self.path, &format_record(&record))?;
        self.outcomes.push(record.outcome);
        let done = record.outcome.ends_run() || self.outcomes.len() >= self.expected;
        if done {
            self.finished = true;
        }
        Ok(Recorded {
            prompt_index: record.prompt_index,
            exit_code: (done && self.exit_after_reply).then(|| self.exit_code()),
        })
    }
}

fn global_log() -> &'static OnceLock<Mutex<ReplyLog>> {
    static LOG: OnceLock<Mutex<ReplyLog>> = OnceLock::new();
    &LOG
}

/// Turn on turn records for this launch. `expected` is how many prompts the launch will run.
/// Does nothing when no reply file was asked for.
pub fn configure(reply_file: Option<PathBuf>, expected: usize, exit_after_reply: bool) {
    if let Some(path) = reply_file.filter(|p| !p.as_os_str().is_empty()) {
        let _ = global_log().set(Mutex::new(ReplyLog::new(path, expected, exit_after_reply)));
    }
}

pub fn is_enabled() -> bool {
    global_log().get().is_some()
}

/// True once the run is over (every prompt recorded, or something ended it).
pub fn run_finished() -> bool {
    global_log().get().map(|l| l.lock().map(|g| g.is_finished()).unwrap_or(true)).unwrap_or(false)
}

/// Record one turn. Never fails the caller; a write problem is reported on stderr.
/// Returns the exit code to use when the app should exit now.
pub fn record_turn(record: TurnRecord) -> Option<i32> {
    let log = global_log().get()?;
    let mut guard = log.lock().unwrap_or_else(|p| p.into_inner());
    match guard.record(record) {
        Ok(recorded) => recorded.exit_code,
        Err(e) => {
            eprintln!("[TurnRecord] could not write the reply file: {e}");
            None
        }
    }
}

pub fn reply_timeout_from(value: Option<&str>) -> Duration {
    let secs = value.and_then(|v| v.trim().parse::<u64>().ok()).filter(|s| *s > 0).unwrap_or(DEFAULT_REPLY_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Where a failure record should go when the command line could not be parsed: the
/// `--reply-file` value found in the raw arguments (`--reply-file X` or `--reply-file=X`), else the environment's.
pub fn reply_file_hint(args: &[String], env_value: Option<&str>) -> Option<PathBuf> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--reply-file=") {
            return Some(PathBuf::from(value)).filter(|p| !p.as_os_str().is_empty());
        }
        if arg == "--reply-file" {
            return iter.next().map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
        }
    }
    env_value.map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

pub fn reply_timeout() -> Duration {
    reply_timeout_from(std::env::var(REPLY_TIMEOUT_ENV_VAR).ok().as_deref())
}

/// Write the record for something that stopped the run, and exit if asked to.
pub fn fail_run(app: &tauri::AppHandle, outcome: TurnOutcome, detail: &str) {
    if run_finished() {
        return;
    }
    println!("[TurnRecord] run ended: {outcome:?}: {detail}");
    if let Some(code) = record_turn(TurnRecord::run_failure(outcome, detail)) {
        exit_app(app, code);
    }
}

/// Leave now, with `code`. The record is already on disk and the chat is saved, so this skips the normal shutdown:
/// tearing down the native model runtime (ONNX Runtime) on the way out segfaulted on macOS, turning the exit code
/// a harness waits for into -11. `app` is kept so callers need no change if a graceful path is ever wanted.
pub fn exit_app(_app: &tauri::AppHandle, code: i32) {
    use std::io::Write;
    crate::launch_marker::clear();
    // This path ends the process directly, so the normal-exit cleanup in `run()` never runs. Without this a
    // harness run left run-state.json behind (rc15 validation); it is a normal exit, not a crash.
    crate::run_state::clear();
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}

/// Watch a reply run: a startup failure or the overall timeout ends it with a record, so a
/// harness waiting on the reply file always gets an answer.
pub fn spawn_watchdog(app: tauri::AppHandle, timeout: Duration) {
    if !is_enabled() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        println!("[TurnRecord] watching the run for up to {}s", timeout.as_secs());
        let started = std::time::Instant::now();
        loop {
            if run_finished() {
                return;
            }
            if let crate::test_state::SmokeOutcome::Failure(why) = crate::test_state::smoke_outcome(&crate::test_state::snapshot()) {
                fail_run(&app, TurnOutcome::StartupError, &why);
                return;
            }
            if started.elapsed() >= timeout {
                fail_run(&app, TurnOutcome::Timeout, &format!("no reply within {}s", timeout.as_secs()));
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plugable-turn-record-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join("replies.jsonl")
    }

    fn ok_record(question: &str, reply: &str) -> TurnRecord {
        TurnRecord {
            prompt_index: 99,
            question: Some(question.to_string()),
            reply: reply.to_string(),
            outcome: TurnOutcome::Ok,
            detail: String::new(),
            model: Some("m".to_string()),
            chat_id: Some("c1".to_string()),
            duration_ms: 12,
            tool_calls: vec![ToolTraceEntry::new("plugableiq", "ask", json!({"question": "q"}), "6,992 orders", false, Duration::from_millis(5))],
        }
    }

    fn lines(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn a_record_is_one_json_line_with_the_reply_and_the_tool_trace() {
        let line = format_record(&ok_record("how many?", "There are 6,992 orders."));
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["reply"], "There are 6,992 orders.");
        assert_eq!(v["tool_calls"][0]["server"], "plugableiq");
        assert_eq!(v["tool_calls"][0]["tool"], "ask");
        assert_eq!(v["tool_calls"][0]["arguments"]["question"], "q");
        assert_eq!(v["tool_calls"][0]["result_chars"], 12);
        assert_eq!(v["tool_calls"][0]["duration_ms"], 5);
    }

    #[test]
    fn a_long_result_is_cut_on_a_character_boundary_and_still_reports_its_full_length() {
        let long = "é".repeat(RESULT_SUMMARY_MAX_CHARS + 50);
        let entry = ToolTraceEntry::new("s", "t", json!({}), &long, false, Duration::ZERO);
        assert_eq!(entry.result_chars, RESULT_SUMMARY_MAX_CHARS + 50);
        assert_eq!(entry.result_summary.chars().count(), RESULT_SUMMARY_MAX_CHARS + 1);
        assert!(entry.result_summary.ends_with('…'));
        assert_eq!(summarize_result("short", 10), "short");
    }

    #[test]
    fn the_log_numbers_prompts_in_order_and_exits_when_every_expected_prompt_has_a_record() {
        let path = tmp_path("order");
        let mut log = ReplyLog::new(path.clone(), 2, true);
        let first = log.record(ok_record("a", "1")).unwrap();
        assert_eq!(first, Recorded { prompt_index: 0, exit_code: None });
        assert!(!log.is_finished());
        let second = log.record(ok_record("b", "2")).unwrap();
        assert_eq!(second, Recorded { prompt_index: 1, exit_code: Some(0) });
        assert!(log.is_finished());
        let v = lines(&path);
        assert_eq!(v.len(), 2);
        assert_eq!([v[0]["prompt_index"].as_u64(), v[1]["prompt_index"].as_u64()], [Some(0), Some(1)]);
    }

    #[test]
    fn without_exit_after_reply_the_app_is_never_asked_to_exit() {
        let mut log = ReplyLog::new(tmp_path("noexit"), 1, false);
        assert_eq!(log.record(ok_record("a", "1")).unwrap().exit_code, None);
        assert!(log.is_finished());
    }

    #[test]
    fn a_failed_turn_gives_a_nonzero_exit_code_once_the_run_is_done() {
        let mut log = ReplyLog::new(tmp_path("failed"), 2, true);
        let mut failed = ok_record("a", "");
        failed.outcome = TurnOutcome::Error;
        assert_eq!(log.record(failed).unwrap().exit_code, None);
        assert_eq!(log.record(ok_record("b", "2")).unwrap().exit_code, Some(1));
    }

    #[test]
    fn a_failure_that_ends_the_run_exits_at_once_with_its_own_code() {
        for (outcome, code) in [(TurnOutcome::LaunchModelError, 2), (TurnOutcome::StartupError, 3), (TurnOutcome::Timeout, 4)] {
            let mut log = ReplyLog::new(tmp_path("ends"), 5, true);
            let recorded = log.record(TurnRecord::run_failure(outcome, "why")).unwrap();
            assert_eq!(recorded.exit_code, Some(code), "{outcome:?}");
            assert!(log.is_finished());
        }
    }

    #[test]
    fn the_exit_code_is_the_first_failing_records() {
        let mut log = ReplyLog::new(tmp_path("first"), 3, true);
        let mut a = ok_record("a", "");
        a.outcome = TurnOutcome::ApprovalTimeout;
        let mut b = ok_record("b", "");
        b.outcome = TurnOutcome::MaxIterations;
        log.record(a).unwrap();
        log.record(b).unwrap();
        assert_eq!(log.exit_code(), 1);
    }

    #[test]
    fn a_run_failure_record_carries_the_reason_and_no_question() {
        let path = tmp_path("failure");
        let mut log = ReplyLog::new(path.clone(), 1, false);
        log.record(TurnRecord::run_failure(TurnOutcome::LaunchModelError, "model x is not cached")).unwrap();
        let v = lines(&path);
        assert_eq!(v[0]["outcome"], "launch_model_error");
        assert_eq!(v[0]["detail"], "model x is not cached");
        assert!(v[0]["question"].is_null());
    }

    #[test]
    fn a_reply_file_is_found_in_the_raw_arguments_even_when_they_do_not_parse() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(reply_file_hint(&args(&["app", "--bogus", "--reply-file", "/t/r.jsonl"]), None), Some(PathBuf::from("/t/r.jsonl")));
        assert_eq!(reply_file_hint(&args(&["app", "--reply-file=/t/r.jsonl"]), None), Some(PathBuf::from("/t/r.jsonl")));
        assert_eq!(reply_file_hint(&args(&["app", "--bogus"]), Some("/e/r.jsonl")), Some(PathBuf::from("/e/r.jsonl")));
        assert_eq!(reply_file_hint(&args(&["app", "--bogus"]), Some("")), None);
        assert_eq!(reply_file_hint(&args(&["app", "--reply-file"]), None), None);
    }

    #[test]
    fn the_timeout_comes_from_the_environment_value_or_the_default() {
        assert_eq!(reply_timeout_from(Some("90")).as_secs(), 90);
        assert_eq!(reply_timeout_from(Some("0")).as_secs(), DEFAULT_REPLY_TIMEOUT_SECS);
        assert_eq!(reply_timeout_from(Some("nope")).as_secs(), DEFAULT_REPLY_TIMEOUT_SECS);
        assert_eq!(reply_timeout_from(None).as_secs(), DEFAULT_REPLY_TIMEOUT_SECS);
    }
}
