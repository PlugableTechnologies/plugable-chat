//! A small rotating log file for the diagnostics that support needs after the fact.
//!
//! Until now the app only printed to a console nobody sees and wrote `crash.log` on a panic, so
//! "what happened on first start?" had no answer. Lines written here also reach the console, and
//! the last few are attached to the "Copy details for support" text. Never fails the caller.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LOG_NAME: &str = "plugable-chat.log";
const MAX_BYTES: u64 = 1024 * 1024;

fn write_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}

pub fn log_dir() -> PathBuf {
    crate::paths::get_config_dir().join("logs")
}

/// Where Foundry Core writes its own log: inside our logs folder so the support bundle can read it.
pub fn engine_log_dir() -> PathBuf {
    log_dir().join("foundry")
}

pub fn log_path() -> PathBuf {
    log_dir().join(LOG_NAME)
}

/// Keep one previous file (`.1`) so a long session cannot grow the log without bound.
fn rotate_if_needed(path: &Path, max_bytes: u64) {
    if std::fs::metadata(path).map(|m| m.len() > max_bytes).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
}

pub fn append_in(dir: &Path, max_bytes: u64, area: &str, message: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(LOG_NAME);
    let line = format!("{} [{}] {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), area, message.replace('\n', "\n    "));
    let _guard = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    rotate_if_needed(&path, max_bytes);
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// Write to the console and the log file.
pub fn log(area: &str, message: &str) {
    println!("[{area}] {message}");
    // Unit tests exercise code that logs; they must not write into the developer's real log folder.
    if cfg!(test) {
        return;
    }
    if let Err(e) = append_in(&log_dir(), MAX_BYTES, area, message) {
        eprintln!("[Log] could not write the log file: {e}");
    }
}

/// The last `n` lines, oldest first; empty when there is no log yet.
pub fn tail_in(dir: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join(LOG_NAME)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect()
}

pub fn tail(n: usize) -> Vec<String> {
    tail_in(&log_dir(), n)
}

/// Words that mark the engine's provider and hardware lines; the rest of its log is model traffic.
const ENGINE_KEYWORDS: [&str; 12] = [
    "executionprovider", "execution provider", "ep ", "winml", "windows ml", "cuda", "webgpu", "tensorrt",
    "openvino", "bootstrap", "device", "gpu",
];

/// The newest file Foundry Core wrote in `dir`, reduced to its last `n` provider-related lines
/// (warnings and errors always count). `None` when the engine has written nothing there.
pub fn engine_log_excerpt_in(dir: &Path, n: usize) -> Option<(PathBuf, Vec<String>)> {
    let newest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?
        .path();
    let text = String::from_utf8_lossy(&std::fs::read(&newest).ok()?).to_string();
    let wanted: Vec<&str> = text
        .lines()
        .filter(|l| {
            let lower = l.to_lowercase();
            ENGINE_KEYWORDS.iter().any(|k| lower.contains(k))
                || lower.contains("warn")
                || lower.contains("error")
                || lower.contains("fail")
        })
        .collect();
    let lines = wanted[wanted.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect();
    Some((newest, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn lines_are_appended_and_tailed_in_order() {
        let d = temp("tail");
        for i in 0..5 {
            append_in(&d, MAX_BYTES, "T", &format!("line {i}")).unwrap();
        }
        let t = tail_in(&d, 2);
        assert_eq!(t.len(), 2);
        assert!(t[0].ends_with("[T] line 3") && t[1].ends_with("[T] line 4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_log_rotates_instead_of_growing_forever() {
        let d = temp("rotate");
        for i in 0..40 {
            append_in(&d, 200, "T", &format!("a fairly long line number {i}")).unwrap();
        }
        assert!(d.join("plugable-chat.log.1").exists());
        assert!(std::fs::metadata(d.join(LOG_NAME)).unwrap().len() < 1000);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn engine_excerpt_keeps_provider_lines_from_the_newest_file() {
        let d = temp("engine");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("old.log"), "CUDAExecutionProvider from an old run\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            d.join("new.log"),
            "chat request 1\nWinML catalog returned 0 providers\nchat request 2\nWARN bootstrap skipped\n",
        )
        .unwrap();
        let (path, lines) = engine_log_excerpt_in(&d, 10).unwrap();
        assert!(path.ends_with("new.log"));
        assert_eq!(lines, vec!["WinML catalog returned 0 providers", "WARN bootstrap skipped"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn no_engine_log_is_none() {
        assert!(engine_log_excerpt_in(&temp("engine-none"), 10).is_none());
    }

    #[test]
    fn a_missing_log_tails_to_nothing() {
        assert!(tail_in(&temp("none"), 10).is_empty());
    }
}
