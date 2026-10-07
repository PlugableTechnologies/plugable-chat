//! Detects "the app died last time it started" without relying on a crash handler.
//!
//! A native crash (access violation) skips the Rust panic hook, so nothing is written and the
//! user just sees the window vanish. Instead a marker file is written at launch and removed once
//! startup finishes or the app exits normally; finding it at the next launch means the previous
//! start never completed.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MARKER_NAME: &str = "launch-in-progress";
static PREVIOUS: OnceLock<Option<String>> = OnceLock::new();

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER_NAME)
}

/// Returns the previous launch's start time if its marker was left behind, then writes a new one.
pub fn begin_in(dir: &Path, now: &str) -> Option<String> {
    let path = marker_path(dir);
    let previous = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string());
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, now);
    previous
}

pub fn clear_in(dir: &Path) {
    let _ = std::fs::remove_file(marker_path(dir));
}

pub fn begin() {
    let now = format!("{} (unix time)", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0));
    let _ = PREVIOUS.set(begin_in(&crate::paths::get_config_dir(), &now));
}

pub fn clear() {
    clear_in(&crate::paths::get_config_dir());
}

pub fn previous_launch_started_at() -> Option<String> {
    PREVIOUS.get().cloned().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-marker-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn first_launch_has_no_previous_marker() {
        let d = temp();
        assert_eq!(begin_in(&d, "t1"), None);
    }

    #[test]
    fn left_behind_marker_is_reported_once() {
        let d = temp();
        begin_in(&d, "t1");
        assert_eq!(begin_in(&d, "t2").as_deref(), Some("t1"));
    }

    #[test]
    fn cleared_marker_means_clean_previous_launch() {
        let d = temp();
        begin_in(&d, "t1");
        clear_in(&d);
        assert_eq!(begin_in(&d, "t2"), None);
    }
}
