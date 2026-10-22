//! Regression tests for the rc14 tester reports, run against `FakeBackend` (no engine needed).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use super::fake::{FakeBackend, Script};
use super::{BackendError, ChatStreamRequest, FoundryBackend};
use crate::actors::foundry::failure_strikes::{should_blocklist, FailureStrikes};
use crate::protocol::{ChatMessage, ModelFamily};

const QWEN: &str = "qwen3.5-9b-generic-gpu:4";
const GPT_OSS: &str = "gpt-oss-20b-generic-gpu:1";
const PHI: &str = "phi-4-openvino-gpu:2";

fn msg(role: &str, chars: usize) -> ChatMessage {
    ChatMessage {
        role: role.into(),
        content: "word ".repeat(chars / 5),
        system_prompt: None,
        tool_calls: None,
        tool_call_id: None,
    }
}

fn req(model: &str, messages: Vec<ChatMessage>) -> ChatStreamRequest {
    ChatStreamRequest {
        model: model.into(),
        family: ModelFamily::GptOss,
        messages,
        tools: None,
        use_native_tools: false,
        supports_reasoning: false,
        supports_reasoning_effort: false,
        reasoning_effort: "medium".into(),
        use_responses_api: false,
    }
}

async fn run(b: &FakeBackend, r: ChatStreamRequest) -> super::ChatStreamOutcome {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (_c, crx) = watch::channel(false);
    b.chat_stream(r, tx, crx).await
}

fn backend() -> FakeBackend {
    FakeBackend::new()
        .with_context(QWEN, 262_144)
        .with_context(GPT_OSS, 8192)
        .with_context(PHI, 16_384)
}

#[tokio::test]
async fn rc14_gpt_oss_8192_context_now_answers() {
    // 1153 input tokens against an 8192 window, family cap 16384: the reported failure.
    let b = backend();
    let out = run(&b, req(GPT_OSS, vec![msg("system", 400), msg("user", 3800)])).await;
    assert!(out.error.is_none(), "{:?}", out.error);
    let (_, _, max_tokens) = b.received.lock().unwrap()[0].clone();
    assert!(max_tokens < 16384 && max_tokens >= 256, "{max_tokens}");
}

#[tokio::test]
async fn model_switch_matrix_never_errors_or_downloads_or_leaks_notices() {
    let b = backend();
    let history = vec![
        msg("system", 400),
        msg("user", 60),
        msg("assistant", 400),
        msg("user", 40),
    ];
    for model in [QWEN, GPT_OSS, PHI, QWEN, GPT_OSS] {
        let out = run(&b, req(model, history.clone())).await;
        assert!(out.error.is_none(), "{model}: {:?}", out.error);
    }
    assert!(b.downloads.lock().unwrap().is_empty(), "switching models must not download");
    for (_, messages, _) in b.received.lock().unwrap().iter() {
        assert!(
            messages.iter().all(|m| !m.content.contains("isn't compatible")),
            "an app notice reached a model"
        );
    }
}

#[tokio::test]
async fn long_tool_heavy_conversation_stays_inside_the_window() {
    // Wash's crash: many follow-ups while querying a dataset. Each turn adds a big SQL result.
    let b = backend();
    let mut history = vec![msg("system", 1200)];
    for turn in 0..14 {
        history.push(msg("user", 120));
        history.push(msg("assistant", 200));
        history.push(msg("tool", 7000)); // a large result set
        let out = run(&b, req(GPT_OSS, history.clone())).await;
        assert!(out.error.is_none(), "turn {turn}: {:?}", out.error);
        let (_, sent, max_tokens) = b.received.lock().unwrap().last().unwrap().clone();
        assert_eq!(sent.first().unwrap().role, "system", "system prompt kept");
        assert_eq!(sent.last().unwrap().content, history.last().unwrap().content, "latest turn kept");
        assert!(max_tokens >= 256);
    }
}

#[tokio::test]
async fn estimator_error_is_absorbed_by_one_tighter_retry() {
    let b = backend();
    // The fake core counts 0.5 tokens/char, far above the estimator's ~0.29: first attempt overflows.
    *b.strict_tokens_per_char.lock().unwrap() = 0.42;
    let out = run(&b, req(GPT_OSS, vec![msg("system", 400), msg("user", 6000)])).await;
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(b.received.lock().unwrap().len(), 2, "exactly one retry");
}

#[tokio::test]
async fn if_the_retry_also_overflows_it_is_a_client_error_not_an_incompatible_model() {
    let b = backend();
    *b.strict_tokens_per_char.lock().unwrap() = 3.0;
    let out = run(&b, req(GPT_OSS, vec![msg("system", 400), msg("user", 3000)])).await;
    let err = out.error.expect("must fail");
    assert!(err.is_context_overflow());
    assert!(!err.is_server(), "must not blocklist the model");
    assert_eq!(b.received.lock().unwrap().len(), 2, "one retry only, no loop");
}

#[tokio::test]
async fn error_classes_match_what_the_actor_does_with_them() {
    let b = backend();
    b.push(Script::Fail("command execution error: Operation was cancelled: The operation was canceled.".into()));
    let e = run(&b, req(QWEN, vec![msg("user", 40)])).await.error.unwrap();
    assert!(e.is_cancelled() && !e.is_server());

    b.push(Script::Fail("ORT runtime exception: WebGPU validation failed".into()));
    let e = run(&b, req(QWEN, vec![msg("user", 40)])).await.error.unwrap();
    assert!(e.is_server());

    b.push(Script::FailLoad("Failed to load model: EP missing".into()));
    let e = run(&b, req(QWEN, vec![msg("user", 40)])).await.error.unwrap();
    assert!(matches!(e, BackendError::Server(_)));

    // and it recovers afterwards
    assert!(run(&b, req(QWEN, vec![msg("user", 40)])).await.error.is_none());
}

#[test]
fn a_single_generation_failure_does_not_blocklist_two_do_and_success_resets() {
    let s = FailureStrikes::default();
    assert!(!should_blocklist(s.record("m")));
    s.clear("m"); // a good turn in between
    assert!(!should_blocklist(s.record("m")));
    assert!(should_blocklist(s.record("m")));
}

#[tokio::test]
async fn download_cancel_stops_it_and_a_queued_cancel_never_starts() {
    let b = backend();
    let flag = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::<f64>::new()));
    let seen2 = seen.clone();
    let f2 = flag.clone();
    let r = b
        .download(
            "phi-4-mini",
            Box::new(move |p| {
                seen2.lock().unwrap().push(p);
                if p >= 50.0 {
                    f2.store(true, Ordering::SeqCst);
                }
            }),
            Some(flag.clone()),
        )
        .await;
    assert!(r.is_err(), "cancelled download must not report success");
    assert!(b.downloads.lock().unwrap().is_empty());
    assert!(crate::actors::foundry::download_cancel::DOWNLOAD_CANCELLED.contains("cancelled"));

    // Cancelled before it ever started: the first check trips immediately.
    let pre = Arc::new(AtomicBool::new(true));
    let r = b.download("phi-4-mini", Box::new(|_| {}), Some(pre)).await;
    assert!(r.is_err());
    assert!(seen.lock().unwrap().len() <= 3);
}
