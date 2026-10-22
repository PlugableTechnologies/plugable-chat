//! What the app was doing when it last ran, so a native crash can be explained and answered.
//!
//! A native fault (rc14: 0xc0000409 inside `onnxruntime_providers_webgpu.dll`) kills the process
//! without a Rust panic, so nothing can be written at the moment of the crash. Instead the app
//! keeps `run-state.json` up to date as it works and removes it on a normal exit. A file left
//! behind at the next launch means the last run died, and says in which phase, on which model, and
//! how big the request was. A model that takes the app down twice is then switched off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "run-state.json";

/// Crashes on the same model before it is taken out of use.
pub const CRASHES_BEFORE_BLOCKLIST: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Idle,
    Chatting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub started_at: String,
    pub phase: Phase,
    pub model: Option<String>,
    /// Chat turns started this run.
    pub turn: u32,
    pub input_tokens: Option<u64>,
    pub max_tokens: Option<u64>,
}

impl RunState {
    pub fn new(started_at: &str) -> Self {
        Self { started_at: started_at.to_string(), phase: Phase::Starting, model: None, turn: 0, input_tokens: None, max_tokens: None }
    }
}

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn write_in(dir: &Path, state: &RunState) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_string(state) {
        // Write beside the target and rename, so a crash mid-write never leaves half a file.
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path(dir));
        }
    }
}

pub fn read_in(dir: &Path) -> Option<RunState> {
    serde_json::from_str(&std::fs::read_to_string(path(dir)).ok()?).ok()
}

pub fn clear_in(dir: &Path) {
    let _ = std::fs::remove_file(path(dir));
}

/// How the last run ended, judged from what it left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastRun {
    Clean,
    /// Died while answering on this model.
    DiedWhileChatting { model: String, turn: u32, input_tokens: Option<u64>, max_tokens: Option<u64> },
    DiedDuringStartup,
    /// Died between turns (nothing running); cause unknown.
    DiedIdle,
}

pub fn classify(previous: Option<&RunState>) -> LastRun {
    match previous {
        None => LastRun::Clean,
        Some(s) => match (s.phase, &s.model) {
            (Phase::Chatting, Some(m)) => LastRun::DiedWhileChatting {
                model: m.clone(),
                turn: s.turn,
                input_tokens: s.input_tokens,
                max_tokens: s.max_tokens,
            },
            (Phase::Starting, _) => LastRun::DiedDuringStartup,
            _ => LastRun::DiedIdle,
        },
    }
}

/// What to do about the last run's death.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrashAction {
    Nothing,
    /// Tell the user; the model stays usable.
    Notice { text: String },
    /// Take the model out of use (it has now crashed the app repeatedly) and say so.
    Blocklist { model: String, reason: String, text: String },
}

/// Counts the crash against the model and picks the response. `counts` is persisted by the caller.
pub fn decide(last: &LastRun, counts: &mut HashMap<String, u32>) -> CrashAction {
    let LastRun::DiedWhileChatting { model, turn, input_tokens, max_tokens } = last else {
        return CrashAction::Nothing;
    };
    let n = counts.entry(model.clone()).or_insert(0);
    *n += 1;
    let size = match (input_tokens, max_tokens) {
        (Some(i), Some(m)) => format!(" The request was about {i} tokens in, up to {m} out."),
        _ => String::new(),
    };
    if *n >= CRASHES_BEFORE_BLOCKLIST {
        CrashAction::Blocklist {
            model: model.clone(),
            reason: format!("It closed Plugable Chat unexpectedly {n} times while answering (a crash inside the GPU runtime)."),
            text: format!(
                "Plugable Chat closed unexpectedly again while {model} was answering. {model} has been switched off on this PC so you can keep working; pick another model, or use \"Retry anyway\" in Settings > Models.{size}"
            ),
        }
    } else {
        CrashAction::Notice {
            text: format!(
                "Plugable Chat closed unexpectedly while {model} was answering (question {turn} of that session).{size} If it happens again, {model} will be switched off automatically."
            ),
        }
    }
}

/// A turn on this model finished normally: forget its crash history.
pub fn record_clean_turn(counts: &mut HashMap<String, u32>, model: &str) {
    counts.remove(model);
}

/// Applies the response to the last run's death to the settings. Returns the notice to show.
/// `runtime_key` is the blocklist's version key, so an engine upgrade gives the model a fresh chance.
pub fn apply_to_settings(settings: &mut crate::settings::AppSettings, last: &LastRun, runtime_key: &str) -> Option<String> {
    match decide(last, &mut settings.native_crash_counts) {
        CrashAction::Nothing => None,
        CrashAction::Notice { text } => Some(text),
        CrashAction::Blocklist { model, reason, text } => {
            settings.incompatible_models.insert(model.clone(), runtime_key.to_string());
            settings.incompatible_model_reasons.insert(model.clone(), reason);
            // It is not the selected model any more: the next start must not pick it again.
            if settings.selected_model.as_deref() == Some(model.as_str()) {
                settings.selected_model = None;
            }
            settings.native_crash_counts.remove(&model);
            Some(text)
        }
    }
}

static CRASH_NOTICE: OnceLock<Option<String>> = OnceLock::new();

pub fn set_crash_notice(n: Option<String>) {
    let _ = CRASH_NOTICE.set(n);
}

pub fn crash_notice() -> Option<String> {
    CRASH_NOTICE.get().cloned().flatten()
}

// ---------- process-wide state ----------

static CURRENT: OnceLock<Mutex<RunState>> = OnceLock::new();
static PREVIOUS: OnceLock<Option<RunState>> = OnceLock::new();

fn with_current(f: impl FnOnce(&mut RunState)) {
    if let Some(m) = CURRENT.get() {
        if let Ok(mut s) = m.lock() {
            f(&mut s);
            write_in(&crate::paths::get_config_dir(), &s);
        }
    }
}

/// Reads what the last run left behind, then starts this run's record.
pub fn begin() {
    let dir = crate::paths::get_config_dir();
    let previous = read_in(&dir);
    let now = format!(
        "{} (unix time)",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    );
    let state = RunState::new(&now);
    write_in(&dir, &state);
    let _ = PREVIOUS.set(previous);
    let _ = CURRENT.set(Mutex::new(state));
}

pub fn previous() -> Option<RunState> {
    PREVIOUS.get().cloned().flatten()
}

pub fn clear() {
    clear_in(&crate::paths::get_config_dir());
}

pub fn mark_idle() {
    with_current(|s| s.phase = Phase::Idle);
}

pub fn mark_chatting(model: &str) {
    with_current(|s| {
        s.phase = Phase::Chatting;
        s.model = Some(model.to_string());
        s.turn += 1;
        s.input_tokens = None;
        s.max_tokens = None;
    });
}

/// The size of the request that is about to run (set just before the engine is called).
pub fn note_request(input_tokens: u64, max_tokens: u64) {
    with_current(|s| {
        s.input_tokens = Some(input_tokens);
        s.max_tokens = Some(max_tokens);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-runstate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn chatting(model: &str) -> RunState {
        RunState { started_at: "t".into(), phase: Phase::Chatting, model: Some(model.into()), turn: 4, input_tokens: Some(6200), max_tokens: Some(1900) }
    }

    #[test]
    fn a_file_left_behind_is_read_back_and_a_cleared_one_is_clean() {
        let d = temp("rw");
        write_in(&d, &chatting("m"));
        assert_eq!(read_in(&d), Some(chatting("m")));
        clear_in(&d);
        assert_eq!(read_in(&d), None);
        assert_eq!(classify(read_in(&d).as_ref()), LastRun::Clean);
    }

    #[test]
    fn a_torn_or_garbage_file_is_treated_as_no_information() {
        let d = temp("garbage");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FILE_NAME), "{not json").unwrap();
        assert_eq!(read_in(&d), None);
    }

    #[test]
    fn classify_says_where_the_last_run_died() {
        assert!(matches!(classify(Some(&chatting("m"))), LastRun::DiedWhileChatting { turn: 4, .. }));
        assert_eq!(classify(Some(&RunState::new("t"))), LastRun::DiedDuringStartup);
        let mut idle = chatting("m");
        idle.phase = Phase::Idle;
        assert_eq!(classify(Some(&idle)), LastRun::DiedIdle);
    }

    #[test]
    fn first_crash_warns_second_switches_the_model_off() {
        let mut counts = HashMap::new();
        let last = classify(Some(&chatting("gpt-oss-20b-generic-gpu:1")));
        match decide(&last, &mut counts) {
            CrashAction::Notice { text } => assert!(text.contains("gpt-oss-20b") && text.contains("6200"), "{text}"),
            other => panic!("{other:?}"),
        }
        match decide(&last, &mut counts) {
            CrashAction::Blocklist { model, text, .. } => {
                assert_eq!(model, "gpt-oss-20b-generic-gpu:1");
                assert!(text.contains("Retry anyway"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn crashes_are_counted_per_model_and_a_clean_turn_resets() {
        let mut counts = HashMap::new();
        decide(&classify(Some(&chatting("a"))), &mut counts);
        assert!(matches!(decide(&classify(Some(&chatting("b"))), &mut counts), CrashAction::Notice { .. }));
        record_clean_turn(&mut counts, "a");
        assert!(matches!(decide(&classify(Some(&chatting("a"))), &mut counts), CrashAction::Notice { .. }));
    }

    #[test]
    fn only_a_mid_chat_death_counts_against_a_model() {
        let mut counts = HashMap::new();
        assert_eq!(decide(&LastRun::DiedDuringStartup, &mut counts), CrashAction::Nothing);
        assert_eq!(decide(&LastRun::DiedIdle, &mut counts), CrashAction::Nothing);
        assert_eq!(decide(&LastRun::Clean, &mut counts), CrashAction::Nothing);
        assert!(counts.is_empty());
    }

    #[test]
    fn the_second_crash_blocklists_in_settings_and_deselects_the_model() {
        let mut st = crate::settings::AppSettings::default();
        st.selected_model = Some("m".into());
        let last = classify(Some(&chatting("m")));
        let first = apply_to_settings(&mut st, &last, "sdk-1");
        assert!(first.unwrap().contains("will be switched off"));
        assert!(st.incompatible_models.is_empty());
        assert_eq!(st.selected_model.as_deref(), Some("m"));
        let second = apply_to_settings(&mut st, &last, "sdk-1");
        assert!(second.unwrap().contains("switched off on this PC"));
        assert_eq!(st.incompatible_models.get("m").map(String::as_str), Some("sdk-1"));
        assert!(st.incompatible_model_reasons["m"].contains("crash"));
        assert_eq!(st.selected_model, None);
        assert!(!st.native_crash_counts.contains_key("m"));
    }

    #[test]
    fn a_clean_previous_run_changes_nothing() {
        let mut st = crate::settings::AppSettings::default();
        assert_eq!(apply_to_settings(&mut st, &LastRun::Clean, "k"), None);
    }
}
