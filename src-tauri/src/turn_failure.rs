//! How a turn ends when the network drops, or the engine reports an error, in the middle of it.
//!
//! rc15 field report: unplugging the network mid-session took the app down in the GPU provider, and the
//! messages and tool results of that turn were not on disk afterwards. The rules here: stop the work
//! that is in flight as soon as the connection is confirmed gone, keep everything the turn produced
//! (the words written so far and every tool call with its result, an interrupted one included), tell
//! the user in plain words, and never start the turn again on its own.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;

use crate::protocol::{ChatMessage, OpenAIToolCall};
use crate::settings::Transport;

/// Consecutive failed probes before a drop is believed: one lost packet is not a dropped network.
pub const DROP_CONFIRMATIONS: u32 = 2;
pub const CHECK_EVERY: Duration = Duration::from_secs(3);

/// What a tool that was cut short reports to the model and the transcript.
pub const INTERRUPTED_TOOL_RESULT: &str =
    "Interrupted: the network connection was lost before this tool finished. Whether it completed is not known.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectivityEvent {
    Dropped,
    Restored,
}

/// Turns raw probe results into drop and restore events. A machine that starts a turn with no
/// network never produces a drop (a local model does not need one), so a restricted office network
/// does not end every turn.
#[derive(Debug, Clone)]
pub struct ConnectivityTracker {
    online: bool,
    failed_in_a_row: u32,
    dropped: bool,
}

impl ConnectivityTracker {
    pub fn new(online_at_start: bool) -> Self {
        Self { online: online_at_start, failed_in_a_row: 0, dropped: false }
    }

    pub fn observe(&mut self, probe_ok: bool) -> Option<ConnectivityEvent> {
        if probe_ok {
            self.failed_in_a_row = 0;
            self.online = true;
            if self.dropped {
                self.dropped = false;
                return Some(ConnectivityEvent::Restored);
            }
            return None;
        }
        if !self.online || self.dropped {
            return None;
        }
        self.failed_in_a_row += 1;
        if self.failed_in_a_row >= DROP_CONFIRMATIONS {
            self.dropped = true;
            return Some(ConnectivityEvent::Dropped);
        }
        None
    }
}

/// Returns when a drop is confirmed. Never returns on a machine that was offline to begin with.
pub async fn wait_for_drop<P, Fut>(mut probe: P, every: Duration)
where
    P: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut tracker = ConnectivityTracker::new(probe().await);
    loop {
        tokio::time::sleep(every).await;
        if tracker.observe(probe().await) == Some(ConnectivityEvent::Dropped) {
            return;
        }
    }
}

/// What a turn watches: a flag that turns true when the network is confirmed gone, and a channel
/// that in-flight work (a stream, a tool call) can wait on. Dropping it stops the watcher.
pub struct DropWatch {
    pub dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub signal: tokio::sync::watch::Receiver<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl DropWatch {
    pub fn start<P, Fut>(probe: P, every: Duration) -> Self
    where
        P: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, signal) = tokio::sync::watch::channel(false);
        let flag = dropped.clone();
        let task = tokio::spawn(async move {
            wait_for_drop(probe, every).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = tx.send(true);
        });
        Self { dropped, signal, task }
    }

    pub fn has_dropped(&self) -> bool {
        self.dropped.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for DropWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Resolves when the channel holds `true` (at once if it already does). Never resolves if the
/// sender goes away without sending it.
pub async fn wait_until_true(mut rx: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Only a server reached over the network can be cut off by a drop; a local process keeps working.
pub fn tool_needs_network(transport: &Transport) -> bool {
    matches!(transport, Transport::Http { .. })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnFailureKind {
    NetworkDropped,
    EngineError,
}

impl TurnFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NetworkDropped => "network_dropped",
            Self::EngineError => "engine_error",
        }
    }

    /// The `chat-error` event: the text for the screen, and which buttons fit.
    pub fn event_payload(self, detail: &str) -> serde_json::Value {
        let (error, offer_repair) = match self {
            Self::NetworkDropped => (
                "The network connection was lost, so this answer was stopped. Everything written so far is saved. Press Retry when the connection is back.".to_string(),
                false,
            ),
            Self::EngineError => (
                format!("The model engine reported an error and this answer was stopped. Everything written so far is saved. {detail}"),
                true,
            ),
        };
        json!({ "error": error.trim(), "kind": self.as_str(), "retryable": true, "offerRepair": offer_repair })
    }
}

// The gateway sets this when the engine returns an error for a stream; the loop reads it when that
// stream's channel closes. One chat turn runs at a time (the engine allows one GPU request), so a
// single slot cannot mix two turns up.
static ENGINE_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn note_engine_error(message: &str) {
    if let Ok(mut slot) = ENGINE_ERROR.lock() {
        *slot = Some(message.to_string());
    }
}

pub fn clear_engine_error() {
    if let Ok(mut slot) = ENGINE_ERROR.lock() {
        *slot = None;
    }
}

pub fn take_engine_error() -> Option<String> {
    ENGINE_ERROR.lock().ok().and_then(|mut slot| slot.take())
}

/// What is worth keeping of a reply that was cut off: the words, without a half-written tool call
/// (which the model would otherwise be shown next turn as if it had finished).
pub fn salvage_partial_reply(text: &str) -> String {
    let open = text.rfind("<tool_call>");
    let closed = text.rfind("</tool_call>");
    let kept = match (open, closed) {
        (Some(o), Some(c)) if c > o => text,
        (Some(o), _) => &text[..o],
        _ => text,
    };
    kept.trim().to_string()
}

/// Native tool calls need a tool message per call id or the next request is rejected. Adds the
/// missing ones, saying the call was interrupted, so a turn cut short still leaves a valid transcript.
pub fn close_dangling_tool_calls(history: &mut Vec<ChatMessage>) {
    let Some(idx) = history.iter().rposition(|m| m.role == "assistant" && m.tool_calls.is_some()) else {
        return;
    };
    let calls: Vec<OpenAIToolCall> = history[idx].tool_calls.clone().unwrap_or_default();
    let answered: Vec<String> = history[idx + 1..]
        .iter()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.tool_call_id.clone())
        .collect();
    for call in calls.iter().filter(|c| !answered.contains(&c.id)) {
        history.push(crate::message_builders::create_native_tool_result_message(&call.id, INTERRUPTED_TOOL_RESULT));
    }
}

/// `history` as it should be saved after a turn failed: a valid tool pairing, and the partial
/// reply (if any) as the last assistant message. A failed turn never gets a made-up reply.
pub fn history_after_failed_turn(history: &[ChatMessage], partial_reply: &str) -> Vec<ChatMessage> {
    let mut out = history.to_vec();
    close_dangling_tool_calls(&mut out);
    let kept = salvage_partial_reply(partial_reply);
    // A call the model finished writing but the drop kept from running would sit in the saved chat
    // with nothing after it (and a reload shows it as raw text); answer it like any interrupted call.
    let ends_in_unanswered_call = kept.ends_with("</tool_call>");
    if !kept.is_empty() {
        out.push(ChatMessage { role: "assistant".to_string(), content: kept, system_prompt: None, tool_calls: None, tool_call_id: None });
    }
    if ends_in_unanswered_call {
        out.push(ChatMessage {
            role: "user".to_string(),
            content: format!("<tool_response error=\"true\">\n{INTERRUPTED_TOOL_RESULT}\n</tool_response>"),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::OpenAIToolCallFunction;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage { role: role.into(), content: content.into(), system_prompt: None, tool_calls: None, tool_call_id: None }
    }

    fn call(id: &str) -> OpenAIToolCall {
        OpenAIToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: OpenAIToolCallFunction { name: "srv___lookup".into(), arguments: "{}".into() },
        }
    }

    #[test]
    fn one_failed_probe_is_not_a_drop_two_in_a_row_are() {
        let mut t = ConnectivityTracker::new(true);
        assert_eq!(t.observe(false), None);
        assert_eq!(t.observe(true), None, "a good probe resets the count");
        assert_eq!(t.observe(false), None);
        assert_eq!(t.observe(false), Some(ConnectivityEvent::Dropped));
        assert_eq!(t.observe(false), None, "a drop is reported once");
        assert_eq!(t.observe(true), Some(ConnectivityEvent::Restored));
        assert_eq!(t.observe(true), None);
    }

    #[test]
    fn a_machine_with_no_network_at_the_start_never_reports_a_drop() {
        let mut t = ConnectivityTracker::new(false);
        for _ in 0..10 {
            assert_eq!(t.observe(false), None);
        }
        assert_eq!(t.observe(true), None, "coming online is not a restore of anything");
        assert_eq!(t.observe(false), None);
        assert_eq!(t.observe(false), Some(ConnectivityEvent::Dropped), "but a drop after that is");
    }

    #[tokio::test]
    async fn the_watch_returns_only_after_the_network_goes_away() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        // online for the first three probes, then gone
        let probe = move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            async move { n < 3 }
        };
        tokio::time::timeout(Duration::from_secs(5), wait_for_drop(probe, Duration::from_millis(1)))
            .await
            .expect("the drop must be noticed");
        assert_eq!(calls.load(Ordering::SeqCst), 3 + DROP_CONFIRMATIONS);
    }

    #[tokio::test]
    async fn the_watch_stays_quiet_on_a_network_that_was_never_there() {
        let probe = || async { false };
        let r = tokio::time::timeout(Duration::from_millis(60), wait_for_drop(probe, Duration::from_millis(1))).await;
        assert!(r.is_err(), "no drop to report when the turn began offline");
    }

    #[tokio::test]
    async fn a_started_watch_flags_the_drop_and_wakes_what_waits_on_it() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let watch = DropWatch::start(
            move || {
                let n = c.fetch_add(1, Ordering::SeqCst);
                async move { n < 2 }
            },
            Duration::from_millis(1),
        );
        assert!(!watch.has_dropped() || calls.load(Ordering::SeqCst) > 2);
        tokio::time::timeout(Duration::from_secs(5), wait_until_true(watch.signal.clone()))
            .await
            .expect("a waiting tool call must be woken by the drop");
        assert!(watch.has_dropped());
    }

    #[tokio::test]
    async fn waiting_on_a_watch_that_is_stopped_never_fires() {
        let watch = DropWatch::start(|| async { true }, Duration::from_millis(1));
        let rx = watch.signal.clone();
        drop(watch);
        let r = tokio::time::timeout(Duration::from_millis(60), wait_until_true(rx)).await;
        assert!(r.is_err());
    }

    #[test]
    fn only_a_server_over_the_network_is_cut_off_by_a_drop() {
        assert!(tool_needs_network(&Transport::Http { url: "http://box:8787/mcp".into() }));
        assert!(!tool_needs_network(&Transport::Stdio));
    }

    #[test]
    fn a_half_written_tool_call_is_dropped_from_the_saved_partial_reply_but_the_words_stay() {
        assert_eq!(salvage_partial_reply("Let me look.\n<tool_call>{\"name\": \"sql_sel"), "Let me look.");
        assert_eq!(salvage_partial_reply("Done: <tool_call>{}</tool_call> ok"), "Done: <tool_call>{}</tool_call> ok");
        assert_eq!(salvage_partial_reply("  The answer is 4"), "The answer is 4");
        assert_eq!(salvage_partial_reply("<tool_call>{\"na"), "");
    }

    #[test]
    fn a_native_call_with_no_result_gets_an_interrupted_result_and_answered_ones_are_untouched() {
        let mut assistant = msg("assistant", "");
        assistant.tool_calls = Some(vec![call("a"), call("b")]);
        let mut done = msg("tool", "42 rows");
        done.tool_call_id = Some("a".into());
        let mut h = vec![msg("user", "q"), assistant, done];
        close_dangling_tool_calls(&mut h);
        assert_eq!(h.len(), 4);
        assert_eq!(h[3].role, "tool");
        assert_eq!(h[3].tool_call_id.as_deref(), Some("b"));
        assert!(h[3].content.starts_with("Interrupted"));
        assert_eq!(h[2].content, "42 rows");
        let before = h.len();
        close_dangling_tool_calls(&mut h);
        assert_eq!(h.len(), before, "closing twice adds nothing");
    }

    #[test]
    fn a_failed_turn_keeps_the_partial_words_and_every_tool_result_and_invents_no_reply() {
        let mut assistant = msg("assistant", "");
        assistant.tool_calls = Some(vec![call("a")]);
        let mut result = msg("tool", "{\"rows\":3}");
        result.tool_call_id = Some("a".into());
        let history = vec![msg("user", "how many?"), assistant, result];
        let saved = history_after_failed_turn(&history, "There are 3 rows, and I was about to <tool_call>{\"na");
        assert_eq!(saved.len(), 4);
        assert_eq!(saved[2].content, "{\"rows\":3}");
        assert_eq!(saved[3].role, "assistant");
        assert_eq!(saved[3].content, "There are 3 rows, and I was about to");
        let nothing = history_after_failed_turn(&history, "");
        assert_eq!(nothing.len(), 3, "no partial words, no assistant message");
        assert!(nothing.iter().all(|m| !m.content.contains("network")), "no notice text is saved into the transcript");
    }

    #[test]
    fn the_failure_event_says_what_happened_and_which_buttons_fit() {
        let drop = TurnFailureKind::NetworkDropped.event_payload("");
        assert_eq!(drop["kind"], "network_dropped");
        assert_eq!(drop["retryable"], true);
        assert_eq!(drop["offerRepair"], false);
        assert!(drop["error"].as_str().unwrap().contains("Retry"));
        let engine = TurnFailureKind::EngineError.event_payload("It closed unexpectedly.");
        assert_eq!(engine["offerRepair"], true);
        assert!(engine["error"].as_str().unwrap().contains("It closed unexpectedly."));
    }

    #[test]
    fn the_engine_error_slot_is_read_once() {
        clear_engine_error();
        assert_eq!(take_engine_error(), None);
        note_engine_error("boom");
        assert_eq!(take_engine_error().as_deref(), Some("boom"));
        assert_eq!(take_engine_error(), None);
    }
}
