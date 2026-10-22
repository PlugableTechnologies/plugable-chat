//! How many times a model has failed while generating, so one bad turn doesn't blocklist it.
//!
//! A load failure is deterministic per (model, runtime) and blocklists at once. A failure while
//! generating can be a one-off (memory pressure, a driver hiccup), so the model gets a second
//! chance; a success in between clears the count.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Generation failures in a row before the model is blocklisted and a fallback offered.
pub const BLOCKLIST_AFTER: u32 = 2;

#[derive(Default)]
pub struct FailureStrikes {
    counts: Mutex<HashMap<String, u32>>,
}

impl FailureStrikes {
    /// Records a failure and returns the number in a row.
    pub fn record(&self, model: &str) -> u32 {
        let mut m = self.counts.lock().unwrap();
        let n = m.entry(model.to_string()).or_insert(0);
        *n += 1;
        *n
    }

    pub fn clear(&self, model: &str) {
        self.counts.lock().unwrap().remove(model);
    }
}

pub fn should_blocklist(strikes: u32) -> bool {
    strikes >= BLOCKLIST_AFTER
}

pub fn failure_strikes() -> &'static FailureStrikes {
    static S: OnceLock<FailureStrikes> = OnceLock::new();
    S.get_or_init(Default::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failure_does_not_blocklist_two_in_a_row_do() {
        let s = FailureStrikes::default();
        assert!(!should_blocklist(s.record("m")));
        assert!(should_blocklist(s.record("m")));
    }

    #[test]
    fn a_success_in_between_resets_the_count() {
        let s = FailureStrikes::default();
        s.record("m");
        s.clear("m");
        assert!(!should_blocklist(s.record("m")));
    }

    #[test]
    fn models_are_counted_separately() {
        let s = FailureStrikes::default();
        s.record("a");
        assert!(!should_blocklist(s.record("b")));
    }
}
