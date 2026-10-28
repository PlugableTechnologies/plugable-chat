//! Retries for a chat-model download that a network drop cut short, and the host named in the error.
//!
//! The download itself runs inside Foundry Local Core (the native SDK) or, with the CLI backend,
//! inside the local `foundry` service: this app cannot see the bytes, choose the host or ask for a
//! byte range. What it can do is call the download again after a wait, keep the partial files the
//! engine left in its cache (nothing is removed between attempts), and say which host the failure
//! came from when the engine's error names one. Whether a second call continues from the bytes
//! already on disk is the engine's behaviour, not something proven here.

use std::future::Future;
use std::time::Duration;

/// Waits between attempts; one more than the number of waits is the number of attempts.
pub const BACKOFF: [Duration; 4] = [Duration::from_secs(5), Duration::from_secs(20), Duration::from_secs(60), Duration::from_secs(180)];

/// Errors a retry cannot fix (or must not hide): a full disk, a cancel, a model that is not there.
const PERMANENT: [&str; 12] = [
    "cancel",
    "not enough disk",
    "no space",
    "disk full",
    "not found in catalog",
    "unknown variant",
    "unauthorized",
    "forbidden",
    "http 401",
    "http 403",
    "http 404",
    "access is denied",
];

const TRANSIENT: [&str; 21] = [
    "connection reset",
    "os error 10054",
    "os error 10053",
    "os error 10060",
    "os error 11001",
    "timed out",
    "timeout",
    "dns",
    "failed to lookup",
    "no such host",
    "error sending request",
    "connection closed",
    "connection refused",
    "broken pipe",
    "unexpected eof",
    "network",
    "interrupted",
    "temporarily",
    "http 429",
    "http 5",
    "incomplete",
];

pub fn is_transient(error: &str) -> bool {
    let l = error.to_ascii_lowercase();
    if PERMANENT.iter().any(|p| l.contains(p)) {
        return false;
    }
    TRANSIENT.iter().any(|t| l.contains(t))
}

fn clean_host(raw: &str) -> Option<String> {
    let host = raw
        .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':'))
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let looks_like_name = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty())
        && labels.last().map_or(false, |l| l.len() >= 2 && l.chars().all(|c| c.is_ascii_alphabetic()));
    looks_like_name.then_some(host)
}

/// The host an engine error names: a URL's host, a `host:port` pair, or one of the hosts this app
/// knows model downloads touch. None when the error names none (the engine often does not).
pub fn failing_host(error: &str) -> Option<String> {
    for token in error.split(|c: char| c.is_whitespace() || "\"'()<>,;[]".contains(c)) {
        if let Some(rest) = token.split("://").nth(1) {
            if let Some(h) = clean_host(rest.split('/').next().unwrap_or("")) {
                return Some(h);
            }
        }
    }
    for token in error.split(|c: char| c.is_whitespace() || "\"'()<>,;[]".contains(c)) {
        let t = token.trim_end_matches(|c: char| c == '.' || c == ':');
        if let Some((h, port)) = t.rsplit_once(':') {
            if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
                if let Some(h) = clean_host(h) {
                    return Some(h);
                }
            }
        }
    }
    let lower = error.to_ascii_lowercase();
    crate::commands::diagnostics::reachability_hosts().find(|h| lower.contains(h)).map(str::to_string)
}

fn host_clause(host: &Option<String>) -> String {
    match host {
        Some(h) => format!(" from {h}"),
        None => String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryNotice {
    pub model: String,
    /// The attempt that just failed (1-based) and how many there will be.
    pub attempt: u32,
    pub of: u32,
    pub wait: Duration,
    pub host: Option<String>,
    pub error: String,
}

impl RetryNotice {
    pub fn text(&self) -> String {
        format!(
            "The download of {} was interrupted{} (attempt {} of {}). Trying again in {} s.",
            self.model,
            host_clause(&self.host),
            self.attempt,
            self.of,
            self.wait.as_secs()
        )
    }
}

/// What the user reads when every attempt failed: the host when the engine named one, and where to look when it did not.
pub fn final_message(model: &str, attempts: u32, error: &str) -> String {
    let host = failing_host(error);
    let where_to_look = match &host {
        Some(h) => format!("The last error came from {h}. Settings > Network > Test connection shows whether it can be reached."),
        None => "The engine did not say which host it was talking to. Settings > Network > Test connection shows which hosts can be reached.".to_string(),
    };
    format!("The download of {model} failed after {attempts} attempts. {where_to_look} Last error: {error}")
}

async fn wait_unless_cancelled(wait: Duration, is_cancelled: &impl Fn() -> bool) {
    let tick = Duration::from_millis(200).min(wait);
    let mut waited = Duration::ZERO;
    while waited < wait && !is_cancelled() {
        let step = tick.min(wait - waited);
        tokio::time::sleep(step).await;
        waited += step;
    }
}

/// Runs `attempt` until it succeeds, fails for a reason a retry cannot fix, is cancelled, or the
/// waits in `delays` are used up. `on_retry` is told before each wait.
pub async fn run_with_retries<F, Fut>(
    model: &str,
    delays: &[Duration],
    is_cancelled: impl Fn() -> bool,
    mut attempt: F,
    mut on_retry: impl FnMut(&RetryNotice),
) -> Result<(), String>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let of = delays.len() as u32 + 1;
    let mut n = 1u32;
    loop {
        match attempt(n).await {
            Ok(()) => return Ok(()),
            Err(e) if is_cancelled() || !is_transient(&e) => return Err(e),
            Err(e) => {
                let Some(wait) = delays.get(n as usize - 1).copied() else {
                    return Err(final_message(model, n, &e));
                };
                on_retry(&RetryNotice { model: model.to_string(), attempt: n, of, wait, host: failing_host(&e), error: e });
                wait_unless_cancelled(wait, &is_cancelled).await;
                if is_cancelled() {
                    return Err(super::download_cancel::DOWNLOAD_CANCELLED.to_string());
                }
                n += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    const FAST: [Duration; 3] = [Duration::from_millis(1), Duration::from_millis(1), Duration::from_millis(1)];
    const RESET: &str = "Download failed: error sending request for url (https://cas-bridge.xethub.hf.co/xet-bridge-us/abc/def?X-Amz=1): connection reset by peer (os error 10054)";

    #[test]
    fn the_host_is_read_from_a_url_a_host_and_port_or_a_known_host() {
        assert_eq!(failing_host(RESET).as_deref(), Some("cas-bridge.xethub.hf.co"));
        assert_eq!(failing_host("tcp connect error: transfer.xethub.hf.co:443 timed out").as_deref(), Some("transfer.xethub.hf.co"));
        assert_eq!(failing_host("could not reach ai.azure.com during the catalog call").as_deref(), Some("ai.azure.com"));
        assert_eq!(failing_host("failed writing model.onnx: connection reset"), None, "a file name is not a host");
        assert_eq!(failing_host("The operation timed out"), None);
    }

    #[test]
    fn network_trouble_is_retried_and_a_full_disk_a_cancel_or_a_missing_model_is_not() {
        assert!(is_transient(RESET));
        assert!(is_transient("HTTP 503 - service unavailable"));
        assert!(is_transient("dns error: failed to lookup address information"));
        assert!(!is_transient("The operation was canceled."));
        assert!(!is_transient("Not enough disk space for qwen"));
        assert!(!is_transient("Model 'x' not found in catalog (3 models available)"));
        assert!(!is_transient("Download failed: HTTP 403 - forbidden"));
        assert!(!is_transient("something unrelated broke"));
    }

    #[tokio::test]
    async fn a_download_cut_off_twice_completes_on_the_third_try_and_each_wait_names_the_host() {
        let calls = Arc::new(AtomicU32::new(0));
        let notices: Arc<Mutex<Vec<RetryNotice>>> = Arc::default();
        let (c, n) = (calls.clone(), notices.clone());
        let result = run_with_retries(
            "qwen3.5-4b-generic-cpu:3",
            &FAST,
            || false,
            move |_| {
                let k = c.fetch_add(1, Ordering::SeqCst) + 1;
                async move { if k < 3 { Err(RESET.to_string()) } else { Ok(()) } }
            },
            move |notice| n.lock().unwrap().push(notice.clone()),
        )
        .await;
        assert_eq!(result, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let seen = notices.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!((seen[0].attempt, seen[0].of), (1, 4));
        assert!(seen[0].text().contains("cas-bridge.xethub.hf.co") && seen[0].text().contains("attempt 1 of 4"), "{}", seen[0].text());
    }

    #[tokio::test]
    async fn when_every_attempt_fails_the_error_names_the_host_and_the_attempts() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let err = run_with_retries(
            "m",
            &FAST,
            || false,
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                async { Err::<(), _>(RESET.to_string()) }
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert!(err.contains("failed after 4 attempts") && err.contains("cas-bridge.xethub.hf.co") && err.contains("Test connection"), "{err}");
    }

    #[tokio::test]
    async fn an_error_with_no_host_says_so_instead_of_guessing_one() {
        let err = run_with_retries("m", &[], || false, |_| async { Err::<(), _>("The operation timed out".to_string()) }, |_| {})
            .await
            .unwrap_err();
        assert!(err.contains("did not say which host"), "{err}");
    }

    #[tokio::test]
    async fn a_permanent_error_is_returned_at_once_with_no_wait() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let err = run_with_retries(
            "m",
            &BACKOFF,
            || false,
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                async { Err::<(), _>("Not enough disk space for m".to_string()) }
            },
            |_| panic!("no retry notice for a permanent error"),
        )
        .await
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(err, "Not enough disk space for m");
    }

    #[tokio::test]
    async fn a_cancel_during_the_wait_stops_the_retries() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = Arc::new(AtomicU32::new(0));
        let (flag, c) = (cancelled.clone(), calls.clone());
        let flag_for_check = cancelled.clone();
        let err = run_with_retries(
            "m",
            &[Duration::from_secs(30)],
            move || flag_for_check.load(Ordering::SeqCst),
            move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                async { Err::<(), _>(RESET.to_string()) }
            },
            move |_| flag.store(true, Ordering::SeqCst),
        )
        .await
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(err, super::super::download_cancel::DOWNLOAD_CANCELLED);
    }

    #[tokio::test]
    async fn the_fake_engine_cut_off_mid_download_completes_after_the_retry_and_keeps_its_progress_events() {
        use crate::actors::foundry::backend::fake::FakeBackend;
        use crate::actors::foundry::backend::FoundryBackend;
        let backend = FakeBackend::new();
        backend.fail_next_downloads(2, RESET);
        let progress_events = Arc::new(AtomicU32::new(0));
        let pe = progress_events.clone();
        let result = run_with_retries(
            "m",
            &FAST,
            || false,
            |_| {
                let pe = pe.clone();
                let sink: crate::actors::foundry::backend::ProgressSink = Box::new(move |_| {
                    pe.fetch_add(1, Ordering::SeqCst);
                });
                backend.download("m", sink, None)
            },
            |_| {},
        )
        .await;
        assert_eq!(result, Ok(()));
        assert_eq!(backend.downloads.lock().unwrap().as_slice(), ["m"], "only the finished attempt counts as a download");
        assert!(progress_events.load(Ordering::SeqCst) >= 3);
    }
}
