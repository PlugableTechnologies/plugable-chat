//! Gives the engine's memory back by reloading the model between turns (issue 38).
//!
//! On the A10G soak, free graphics memory and free commit memory were lost in whole 1 to 2 GiB steps as the
//! chat went on and never came back while the app lived; a restart returned all of it. The engine runs inside
//! this process, so the likely cause is its allocator keeping what it grew to. Unloading and loading the model
//! again is the cheapest thing that might hand that back without a restart. Whether it does is only provable on
//! a GPU box (see docs/gpu-validation.md, Issue 38), so this is guarded: it runs only at the start of a turn
//! (never between the steps of one), only when free graphics memory is below a threshold, only when the last
//! reload actually helped, and a failed reload leaves the model loaded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::FoundryBackend;

/// Set to `1`, `true`, `on` or `yes` to reload (off by default: it crashed the app in the rc19 box run).
pub const RELOAD_ENABLE_ENV: &str = "PLUGABLE_RELOAD_ON_LOW_MEMORY";
/// Reload when free graphics memory is below this many MB (overrides the derived threshold).
pub const RELOAD_FREE_GPU_MB_ENV: &str = "PLUGABLE_RELOAD_FREE_GPU_MB";

/// Reload when less than this many times the model's own size is free: after an unload the card has the
/// weights' space back, and the next request needs room for the cache and its working memory on top of them.
pub const RELOAD_FOOTPRINT_MULTIPLE: u64 = 2;
/// Never reload because of a threshold below this (the budget's own floor handles a nearly full card).
pub const RELOAD_MIN_THRESHOLD_MB: u64 = 2_048;
/// Threshold when the model's size is unknown.
pub const RELOAD_DEFAULT_THRESHOLD_MB: u64 = 6_144;
/// A reload that freed less than this did not help; do not repeat it until memory has recovered on its own.
pub const RELOAD_MIN_USEFUL_GAIN_MB: u64 = 512;
/// What the user sees while it runs, through the existing `chat-stream-status` event.
pub const RELOAD_STATUS_TEXT: &str = "Refreshing the model to free memory";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadConfig {
    pub enabled: bool,
    pub threshold_override_mb: Option<u64>,
}

impl ReloadConfig {
    pub fn from_values(enable: Option<&str>, threshold_mb: Option<&str>) -> Self {
        // Opt-in: on the rc19 box run a reload after an early-stopped request crashed the app in
        // onnxruntime-genai.dll (twice) and gave back only part of the memory.
        let enabled = matches!(
            enable.map(|v| v.trim().to_lowercase()).as_deref(),
            Some("1") | Some("true") | Some("on") | Some("yes")
        );
        let threshold_override_mb = threshold_mb.and_then(|v| v.trim().parse::<u64>().ok()).filter(|v| *v > 0);
        Self { enabled, threshold_override_mb }
    }

    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var(RELOAD_ENABLE_ENV).ok().as_deref(),
            std::env::var(RELOAD_FREE_GPU_MB_ENV).ok().as_deref(),
        )
    }
}

/// Free graphics memory (MB) below which a turn start reloads the model.
pub fn threshold_mb(footprint_mb: Option<u64>, cfg: &ReloadConfig) -> u64 {
    if let Some(v) = cfg.threshold_override_mb {
        return v;
    }
    match footprint_mb {
        Some(f) => (f * RELOAD_FOOTPRINT_MULTIPLE).max(RELOAD_MIN_THRESHOLD_MB),
        None => RELOAD_DEFAULT_THRESHOLD_MB,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    /// Not the first request of a turn: the user is mid-answer.
    MidTurn,
    /// Free memory could not be read.
    Unreadable,
    Plentiful,
    /// The previous reload freed almost nothing and memory has not recovered since.
    Futile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    Skipped(SkipReason),
    Reloaded { free_before_mb: u64, free_after_mb: Option<u64> },
    /// The reload did not complete. `model_usable` says whether the model is loaded afterwards.
    Failed { reason: String, model_usable: bool },
}

/// Remembers whether the last reload helped, so a card that is full for another reason is not reloaded every turn.
#[derive(Debug, Default)]
pub struct ReloadGovernor {
    last_reload_was_futile: AtomicBool,
}

/// Reload `model` when free graphics memory is low. Call it with the GPU lock held, so nothing else is in
/// flight; `turn_start` is false for every request after the first of a turn.
pub async fn reload_if_memory_low<B, F>(
    backend: &B,
    governor: &ReloadGovernor,
    model: &str,
    turn_start: bool,
    cfg: &ReloadConfig,
    read_free_gpu_mb: F,
    on_status: &(dyn Fn(&str) + Sync),
) -> ReloadOutcome
where
    B: FoundryBackend + ?Sized,
    F: Fn() -> Option<u64> + Clone + Send + 'static,
{
    if !cfg.enabled {
        return ReloadOutcome::Skipped(SkipReason::Disabled);
    }
    if !turn_start {
        return ReloadOutcome::Skipped(SkipReason::MidTurn);
    }
    let read = read_free_gpu_mb.clone();
    let Some(free_before) = tokio::task::spawn_blocking(read).await.ok().flatten() else {
        return ReloadOutcome::Skipped(SkipReason::Unreadable);
    };
    let threshold = threshold_mb(backend.model_footprint_mb(model).await, cfg);
    if free_before >= threshold {
        governor.last_reload_was_futile.store(false, Ordering::SeqCst);
        return ReloadOutcome::Skipped(SkipReason::Plentiful);
    }
    if governor.last_reload_was_futile.load(Ordering::SeqCst) {
        return ReloadOutcome::Skipped(SkipReason::Futile);
    }

    println!("[SdkBackend] free gpu {free_before} MB is below {threshold} MB; reloading the model between turns");
    on_status(RELOAD_STATUS_TEXT);
    if let Err(e) = backend.unload(model).await {
        // Nothing changed: the model is still loaded.
        println!("[SdkBackend] reload skipped, the model could not be unloaded: {e}");
        return ReloadOutcome::Failed { reason: e, model_usable: true };
    }
    let mut loaded = backend.load(model).await;
    if loaded.is_err() {
        // Get the old model back before giving up; the next request would otherwise find nothing loaded.
        loaded = backend.load(model).await;
    }
    if let Err(e) = loaded {
        println!("[SdkBackend] reload failed, the model is not loaded; the next request will load it again: {}", e.message());
        return ReloadOutcome::Failed { reason: e.message().to_string(), model_usable: false };
    }
    let read = read_free_gpu_mb;
    let free_after = tokio::task::spawn_blocking(read).await.ok().flatten();
    let futile = free_after.map_or(false, |a| a < free_before + RELOAD_MIN_USEFUL_GAIN_MB);
    governor.last_reload_was_futile.store(futile, Ordering::SeqCst);
    println!(
        "[SdkBackend] reloaded the model to release engine memory (free gpu {free_before} -> {} MB)",
        free_after.map_or("unreadable".to_string(), |a| a.to_string())
    );
    ReloadOutcome::Reloaded { free_before_mb: free_before, free_after_mb: free_after }
}

/// Convenience for the gateway: one shared governor for the process.
pub fn process_governor() -> Arc<ReloadGovernor> {
    static GOV: std::sync::OnceLock<Arc<ReloadGovernor>> = std::sync::OnceLock::new();
    GOV.get_or_init(Default::default).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actors::foundry::backend::fake::FakeBackend;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const M: &str = "qwen3.5-4b-reload:1";

    /// Free-memory readings handed out in order; the last one repeats.
    fn readings(vals: &[Option<u64>]) -> impl Fn() -> Option<u64> + Clone + Send + 'static {
        let q = Arc::new(Mutex::new(VecDeque::from(vals.to_vec())));
        move || {
            let mut q = q.lock().unwrap();
            if q.len() > 1 { q.pop_front().unwrap() } else { q.front().copied().unwrap() }
        }
    }

    fn cfg() -> ReloadConfig {
        ReloadConfig::from_values(Some("1"), None)
    }

    fn loaded_backend() -> FakeBackend {
        let b = FakeBackend::new();
        b.resident.lock().unwrap().insert(M.to_string());
        *b.footprint_mb.lock().unwrap() = Some(4_500); // threshold 9,000 MB
        b
    }

    async fn go(b: &FakeBackend, g: &ReloadGovernor, turn_start: bool, c: &ReloadConfig, r: impl Fn() -> Option<u64> + Clone + Send + 'static, statuses: &Mutex<Vec<String>>) -> ReloadOutcome {
        reload_if_memory_low(b, g, M, turn_start, c, r, &|s| statuses.lock().unwrap().push(s.to_string())).await
    }

    #[tokio::test]
    async fn reloads_when_free_gpu_is_below_the_threshold_and_says_so() {
        let b = loaded_backend();
        let statuses = Mutex::new(vec![]);
        let out = go(&b, &ReloadGovernor::default(), true, &cfg(), readings(&[Some(5_000), Some(14_000)]), &statuses).await;
        assert_eq!(out, ReloadOutcome::Reloaded { free_before_mb: 5_000, free_after_mb: Some(14_000) });
        assert_eq!(*b.unloads.lock().unwrap(), vec![M.to_string()]);
        assert!(b.resident.lock().unwrap().contains(M), "loaded again");
        assert_eq!(*statuses.lock().unwrap(), vec!["Refreshing the model to free memory".to_string()]);
    }

    #[tokio::test]
    async fn does_not_reload_when_memory_is_plentiful() {
        let b = loaded_backend();
        let statuses = Mutex::new(vec![]);
        let out = go(&b, &ReloadGovernor::default(), true, &cfg(), readings(&[Some(20_000)]), &statuses).await;
        assert_eq!(out, ReloadOutcome::Skipped(SkipReason::Plentiful));
        assert!(b.unloads.lock().unwrap().is_empty() && statuses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn never_reloads_in_the_middle_of_a_turn_however_low_memory_is() {
        let b = loaded_backend();
        let statuses = Mutex::new(vec![]);
        let out = go(&b, &ReloadGovernor::default(), false, &cfg(), readings(&[Some(100)]), &statuses).await;
        assert_eq!(out, ReloadOutcome::Skipped(SkipReason::MidTurn));
        assert!(b.unloads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_reload_that_cannot_load_again_retries_and_a_dead_unload_leaves_the_old_model() {
        // Unload fails: the old model is untouched and usable.
        let b = loaded_backend();
        b.fail_next_unloads.store(1, Ordering::SeqCst);
        let st = Mutex::new(vec![]);
        let out = go(&b, &ReloadGovernor::default(), true, &cfg(), readings(&[Some(5_000)]), &st).await;
        assert!(matches!(out, ReloadOutcome::Failed { model_usable: true, .. }), "{out:?}");
        assert!(b.resident.lock().unwrap().contains(M));

        // First load after the unload fails, the second brings the model back: usable again, reported as reloaded.
        let b = loaded_backend();
        b.fail_next_loads.store(1, Ordering::SeqCst);
        let out = go(&b, &ReloadGovernor::default(), true, &cfg(), readings(&[Some(5_000), Some(15_000)]), &st).await;
        assert!(matches!(out, ReloadOutcome::Reloaded { .. }), "{out:?}");
        assert!(b.resident.lock().unwrap().contains(M));

        // Both loads fail: reported as failed and not usable; the next chat request loads it again by itself.
        let b = loaded_backend();
        b.fail_next_loads.store(2, Ordering::SeqCst);
        let out = go(&b, &ReloadGovernor::default(), true, &cfg(), readings(&[Some(5_000)]), &st).await;
        assert!(matches!(out, ReloadOutcome::Failed { model_usable: false, .. }), "{out:?}");
    }

    #[tokio::test]
    async fn a_reload_that_freed_nothing_is_not_repeated_every_turn() {
        let b = loaded_backend();
        let g = ReloadGovernor::default();
        let st = Mutex::new(vec![]);
        let first = go(&b, &g, true, &cfg(), readings(&[Some(5_000), Some(5_100)]), &st).await;
        assert!(matches!(first, ReloadOutcome::Reloaded { .. }));
        let second = go(&b, &g, true, &cfg(), readings(&[Some(5_100)]), &st).await;
        assert_eq!(second, ReloadOutcome::Skipped(SkipReason::Futile));
        assert_eq!(b.unloads.lock().unwrap().len(), 1);
        // Memory recovers on its own: the governor forgets.
        go(&b, &g, true, &cfg(), readings(&[Some(30_000)]), &st).await;
        let third = go(&b, &g, true, &cfg(), readings(&[Some(5_000), Some(15_000)]), &st).await;
        assert!(matches!(third, ReloadOutcome::Reloaded { .. }));
    }

    #[tokio::test]
    async fn the_off_switch_and_an_unreadable_card_both_skip() {
        let b = loaded_backend();
        let st = Mutex::new(vec![]);
        let off = ReloadConfig::from_values(Some("0"), None);
        assert!(!ReloadConfig::from_values(None, None).enabled);
        assert_eq!(go(&b, &ReloadGovernor::default(), true, &off, readings(&[Some(100)]), &st).await, ReloadOutcome::Skipped(SkipReason::Disabled));
        assert_eq!(go(&b, &ReloadGovernor::default(), true, &cfg(), readings(&[None]), &st).await, ReloadOutcome::Skipped(SkipReason::Unreadable));
        assert!(b.unloads.lock().unwrap().is_empty());
    }

    #[test]
    fn the_threshold_comes_from_the_footprint_with_an_env_override() {
        assert_eq!(threshold_mb(Some(4_500), &cfg()), 9_000);
        assert_eq!(threshold_mb(Some(300), &cfg()), RELOAD_MIN_THRESHOLD_MB);
        assert_eq!(threshold_mb(None, &cfg()), RELOAD_DEFAULT_THRESHOLD_MB);
        let c = ReloadConfig::from_values(Some("1"), Some(" 12000 "));
        assert_eq!(threshold_mb(Some(4_500), &c), 12_000);
        assert_eq!(ReloadConfig::from_values(Some("OFF"), Some("junk")), ReloadConfig { enabled: false, threshold_override_mb: None });
        assert!(ReloadConfig::from_values(Some("1"), None).enabled && !ReloadConfig::from_values(None, None).enabled);
        assert_eq!(RELOAD_FREE_GPU_MB_ENV, "PLUGABLE_RELOAD_FREE_GPU_MB");
    }
}
