//! Cancel flags for model downloads, keyed by the name the download was requested with.
//!
//! The gateway actor runs one download at a time, inline in its message loop, so a "cancel"
//! message to the actor would wait behind the very download it should stop. The command sets the
//! flag directly instead, and the running download (or a queued one, before it starts) sees it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
pub struct DownloadCancels {
    flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DownloadCancels {
    /// Registers a download (a repeat request for the same name starts with a clear flag).
    pub fn register(&self, name: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.flags.lock().unwrap().insert(name.to_string(), flag.clone());
        flag
    }

    /// The flag of a registered download, if any.
    pub fn flag(&self, name: &str) -> Option<Arc<AtomicBool>> {
        self.flags.lock().unwrap().get(name).cloned()
    }

    /// Returns true if a queued or running download was asked to stop.
    pub fn cancel(&self, name: &str) -> bool {
        match self.flag(name) {
            Some(f) => {
                f.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn is_cancelled(&self, name: &str) -> bool {
        self.flag(name).map(|f| f.load(Ordering::SeqCst)).unwrap_or(false)
    }

    pub fn finish(&self, name: &str) {
        self.flags.lock().unwrap().remove(name);
    }
}

pub fn download_cancels() -> &'static DownloadCancels {
    static CANCELS: OnceLock<DownloadCancels> = OnceLock::new();
    CANCELS.get_or_init(Default::default)
}

/// The message the frontend recognises as "the user cancelled", not a failure.
pub const DOWNLOAD_CANCELLED: &str = "Download cancelled";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_reaches_a_registered_download_only() {
        let c = DownloadCancels::default();
        assert!(!c.cancel("a"), "nothing registered");
        let f = c.register("a");
        assert!(c.cancel("a"));
        assert!(f.load(Ordering::SeqCst));
        assert!(c.is_cancelled("a"));
    }

    #[test]
    fn a_queued_download_cancelled_before_it_starts_sees_the_flag() {
        let c = DownloadCancels::default();
        c.register("queued");
        c.cancel("queued");
        // the actor looks the flag up when the download reaches the front of the queue
        assert!(c.flag("queued").unwrap().load(Ordering::SeqCst));
    }

    #[test]
    fn cancelling_one_download_leaves_the_other_running() {
        let c = DownloadCancels::default();
        c.register("a");
        c.register("b");
        c.cancel("a");
        assert!(c.is_cancelled("a"));
        assert!(!c.is_cancelled("b"));
    }

    #[test]
    fn cancel_after_finish_is_a_no_op_and_a_retry_starts_clear() {
        let c = DownloadCancels::default();
        c.register("a");
        c.finish("a");
        assert!(!c.cancel("a"));
        c.register("a");
        assert!(!c.is_cancelled("a"));
        c.cancel("a");
        // re-registering (a retry) replaces the cancelled flag
        let fresh = c.register("a");
        assert!(!fresh.load(Ordering::SeqCst));
    }
}
