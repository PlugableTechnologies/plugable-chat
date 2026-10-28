//! What a native crash leaves behind: a Windows Error Reporting minidump and a small marker that says
//! whether the last run died inside a downloaded execution provider.
//!
//! A fast-fail crash (0xc0000409, what rc14/rc15 hit in `onnxruntime_providers_webgpu.dll`) skips every
//! in-process handler, so no code in this process can write a dump at the moment of death. Windows
//! Error Reporting can: the installer enables `LocalDumps` for `plugable-chat.exe` (see
//! `windows/hooks.nsh`), and the next launch finds the dump and records it in `last-crash.json` next
//! to what `engine_health` already attributes from the Application log.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine_health::{attribute, FaultSite, NativeFault};
use crate::run_state::{LastRun, RunState};

pub const MARKER_FILE: &str = "last-crash.json";
pub const DUMP_DIR_ENV: &str = "PLUGABLE_CHAT_DUMP_DIR";
const DUMP_EXE_PREFIX: &str = "plugable-chat.exe";

/// The previous run's death, as read at the next launch. Kept until the next death replaces it or
/// Repair clears it, so a support person still sees it after one clean session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashMarker {
    pub recorded_at_unix: u64,
    /// `chatting`, `startup` or `idle`: the phase `run-state.json` was in when the process died.
    pub died_in: String,
    pub model: Option<String>,
    pub turn: Option<u32>,
    pub fault_code: Option<String>,
    pub faulting_module: Option<String>,
    /// The faulting module is a downloaded execution provider under `.plugable-chat/ep`.
    pub in_ep: bool,
    pub provider: Option<String>,
    pub dump_path: Option<String>,
}

/// Where WER writes dumps for a program with `LocalDumps` and no `DumpFolder` (the installer sets
/// none: a folder under Program Files would not be writable by the crashing user).
pub fn default_dump_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(DUMP_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::data_local_dir().map(|d| d.join("CrashDumps"))
}

/// The unix time at the front of `RunState::started_at` ("1730000000 (unix time)").
pub fn started_unix(state: &RunState) -> Option<u64> {
    state.started_at.split_whitespace().next()?.parse().ok()
}

/// The newest `plugable-chat.exe.<pid>.dmp` written at or after `not_before_unix`.
pub fn find_dump_in(dir: &Path, not_before_unix: u64) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !(name.starts_with(DUMP_EXE_PREFIX) && name.ends_with(".dmp")) {
            continue;
        }
        let Some(mtime) = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
        else {
            continue;
        };
        if mtime >= not_before_unix && best.as_ref().map_or(true, |(t, _)| mtime > *t) {
            best = Some((mtime, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// None when the last run ended cleanly: there is nothing to record.
pub fn build_marker(last: &LastRun, fault: Option<&NativeFault>, dump: Option<&Path>, now_unix: u64) -> Option<CrashMarker> {
    let (died_in, model, turn) = match last {
        LastRun::Clean => return None,
        LastRun::DiedWhileChatting { model, turn, .. } => ("chatting", Some(model.clone()), Some(*turn)),
        LastRun::DiedDuringStartup => ("startup", None, None),
        LastRun::DiedIdle => ("idle", None, None),
    };
    let site = fault.map(attribute);
    Some(CrashMarker {
        recorded_at_unix: now_unix,
        died_in: died_in.to_string(),
        model,
        turn,
        fault_code: fault.map(|f| f.code.clone()),
        faulting_module: fault.map(|f| f.module_path.clone()),
        in_ep: matches!(site, Some(FaultSite::Ep { .. })),
        provider: match site {
            Some(FaultSite::Ep { provider }) => provider,
            _ => None,
        },
        dump_path: dump.map(|p| p.to_string_lossy().to_string()),
    })
}

pub fn write_marker_in(dir: &Path, marker: &CrashMarker) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_string_pretty(marker) {
        let tmp = dir.join(format!("{MARKER_FILE}.tmp"));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(MARKER_FILE));
        }
    }
}

pub fn read_marker_in(dir: &Path) -> Option<CrashMarker> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(MARKER_FILE)).ok()?).ok()
}

pub fn clear_marker_in(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(MARKER_FILE));
}

/// Records the previous run's death, with its dump when WER wrote one. Returns what was written.
pub fn record_previous_death_in(
    config_dir: &Path,
    dump_dir: Option<&Path>,
    last: &LastRun,
    fault: Option<&NativeFault>,
    previous: Option<&RunState>,
    now_unix: u64,
) -> Option<CrashMarker> {
    // A death with no run-state start time cannot be matched to a dump, so none is claimed.
    let dump = dump_dir.zip(previous.and_then(started_unix)).and_then(|(d, since)| find_dump_in(d, since));
    let marker = build_marker(last, fault, dump.as_deref(), now_unix)?;
    write_marker_in(config_dir, &marker);
    Some(marker)
}

/// The marker for the Settings screen and the support bundle.
pub fn last_crash() -> Option<CrashMarker> {
    read_marker_in(&crate::paths::get_config_dir())
}

/// Dies the way the rc14/rc15 crash did, for the fault harness (`PLUGABLE_CHAT_TEST_FAULT=crash-mid-turn`).
/// On Windows `abort` is a fast-fail (0xc0000409): no handler runs, WER takes a dump, and the
/// run-state file stays behind, which is the whole path under test.
pub fn die_like_a_native_fault() -> ! {
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_health::test_ep_fault;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-crashcap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn chatting_state(started: u64) -> RunState {
        RunState {
            started_at: format!("{started} (unix time)"),
            phase: crate::run_state::Phase::Chatting,
            model: Some("qwen3.5-4b-generic-gpu:1".into()),
            turn: 3,
            input_tokens: None,
            max_tokens: None,
        }
    }

    fn set_mtime(path: &Path, unix: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn a_death_the_ep_fault_seam_names_leaves_a_last_run_crashed_in_ep_marker() {
        let cfg = temp("ep");
        let state = chatting_state(1_000);
        let last = crate::run_state::classify(Some(&state));
        let fault = test_ep_fault();
        let m = record_previous_death_in(&cfg, None, &last, Some(&fault), Some(&state), 2_000).unwrap();
        assert!(m.in_ep);
        assert_eq!(m.provider.as_deref(), Some("webgpu-ep"));
        assert_eq!(m.died_in, "chatting");
        assert_eq!(m.model.as_deref(), Some("qwen3.5-4b-generic-gpu:1"));
        assert_eq!(m.fault_code.as_deref(), Some("0xc0000409"));
        assert_eq!(read_marker_in(&cfg), Some(m));
    }

    #[test]
    fn a_death_outside_the_providers_is_recorded_but_not_as_an_ep_crash() {
        let cfg = temp("other");
        let fault = NativeFault { module_path: "C:\\Program Files\\plugable-chat\\foundry-libs\\onnxruntime.dll".into(), ..test_ep_fault() };
        let m = record_previous_death_in(&cfg, None, &LastRun::DiedIdle, Some(&fault), None, 5).unwrap();
        assert!(!m.in_ep && m.provider.is_none());
        let unknown = record_previous_death_in(&cfg, None, &LastRun::DiedDuringStartup, None, None, 6).unwrap();
        assert!(!unknown.in_ep && unknown.fault_code.is_none() && unknown.died_in == "startup");
    }

    #[test]
    fn a_clean_exit_records_nothing_and_leaves_an_earlier_marker_alone() {
        let cfg = temp("clean");
        assert_eq!(record_previous_death_in(&cfg, None, &LastRun::Clean, None, None, 1), None);
        assert_eq!(read_marker_in(&cfg), None);
        let state = chatting_state(1);
        let last = crate::run_state::classify(Some(&state));
        record_previous_death_in(&cfg, None, &last, Some(&test_ep_fault()), Some(&state), 2);
        assert_eq!(record_previous_death_in(&cfg, None, &LastRun::Clean, None, None, 3), None);
        assert!(read_marker_in(&cfg).unwrap().in_ep, "one clean launch must not erase the evidence");
        clear_marker_in(&cfg);
        assert_eq!(read_marker_in(&cfg), None);
    }

    #[test]
    fn the_marker_carries_the_dump_that_belongs_to_the_dead_run_not_an_older_one() {
        let cfg = temp("dump-cfg");
        let dumps = temp("dump-dir");
        let old = dumps.join("plugable-chat.exe.111.dmp");
        let new = dumps.join("plugable-chat.exe.222.dmp");
        let other_app = dumps.join("notepad.exe.333.dmp");
        for p in [&old, &new, &other_app] {
            std::fs::write(p, "MDMP").unwrap();
        }
        set_mtime(&old, 500);
        set_mtime(&new, 1_500);
        set_mtime(&other_app, 1_900);
        let state = chatting_state(1_000);
        let last = crate::run_state::classify(Some(&state));
        let m = record_previous_death_in(&cfg, Some(&dumps), &last, Some(&test_ep_fault()), Some(&state), 2_000).unwrap();
        assert_eq!(m.dump_path.as_deref(), Some(new.to_string_lossy().as_ref()));
        // nothing newer than the run's start: no dump is claimed
        set_mtime(&new, 900);
        let none = record_previous_death_in(&cfg, Some(&dumps), &last, None, Some(&state), 2_000).unwrap();
        assert_eq!(none.dump_path, None);
    }

    #[test]
    fn the_run_start_time_is_read_from_the_run_state_text() {
        assert_eq!(started_unix(&chatting_state(1730000000)), Some(1730000000));
        let mut s = chatting_state(1);
        s.started_at = "garbage".into();
        assert_eq!(started_unix(&s), None);
    }

    #[test]
    fn a_marker_file_that_is_garbage_reads_as_none() {
        let cfg = temp("garbage");
        std::fs::write(cfg.join(MARKER_FILE), "{nope").unwrap();
        assert_eq!(read_marker_in(&cfg), None);
    }
}
