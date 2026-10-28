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

// ---- out of memory on a long prompt (rc15 on an A10G: "bad allocation" at ~18,000 input tokens) ----

const OOM_TEXT: &str = "command execution error: Error: Microsoft.ML.OnnxRuntimeGenAI.OnnxRuntimeGenAIException: Non-zero status code returned while running SimplifiedLayerNormalization node. Name:'/model/layers.3/attn/k_norm/SimplifiedLayerNormalization' Status Message: bad allocation";

#[test]
fn an_allocation_failure_is_a_client_problem_not_a_bad_model() {
    for m in [OOM_TEXT, "CUDA out of memory", "std::bad_alloc", "Failed to allocate 2147483648 bytes", "E_OUTOFMEMORY"] {
        let e = BackendError::classify_generation(m);
        assert!(matches!(e, BackendError::Client(_)), "{m}");
        assert!(e.is_out_of_memory() && !e.is_server(), "{m}: must not feed the blocklist");
    }
    // a genuine runtime fault is still the model's
    assert!(BackendError::classify_generation("ORT runtime exception: WebGPU validation failed").is_server());
}

#[tokio::test]
async fn a_long_conversation_that_runs_out_of_memory_is_retried_smaller_and_answers() {
    let model = "qwen3.5-oom-retry:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    *b.oom_above_input_tokens.lock().unwrap() = Some(20_000);
    // About 27,000 estimated input tokens of history against a 20,000-token memory limit: the first attempt fails, 60% of it fits.
    let long = vec![msg("system", 400), msg("user", 36_000), msg("assistant", 36_000), msg("user", 24_000), msg("assistant", 40), msg("user", 40)];
    let out = run(&b, req(model, long)).await;
    assert!(out.error.is_none(), "the retry must answer: {:?}", out.error);
    let seen = b.received.lock().unwrap();
    assert_eq!(seen.len(), 2, "exactly one retry");
    assert!(seen[1].1.len() < seen[0].1.len(), "the retry sends less history");
    assert_eq!(seen[1].1.last().unwrap().content.len(), msg("user", 40).content.len(), "the newest message is kept");
}

#[tokio::test]
async fn once_a_memory_limit_is_found_later_turns_stay_under_it_without_a_failed_attempt() {
    let model = "qwen3.5-oom-remembered:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    *b.oom_above_input_tokens.lock().unwrap() = Some(20_000);
    let long = || vec![msg("system", 400), msg("user", 36_000), msg("assistant", 36_000), msg("user", 24_000), msg("assistant", 40), msg("user", 40)];
    assert!(run(&b, req(model, long())).await.error.is_none());
    let before = b.received.lock().unwrap().len();
    assert!(run(&b, req(model, long())).await.error.is_none());
    assert_eq!(b.received.lock().unwrap().len() - before, 1, "the second turn must not fail first");
    assert!(super::memory_ceiling(model).is_some());
}

#[tokio::test]
async fn if_even_the_smaller_request_runs_out_of_memory_the_user_gets_a_plain_message_and_the_model_is_not_blamed() {
    let model = "qwen3.5-oom-hopeless:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    *b.oom_above_input_tokens.lock().unwrap() = Some(50); // nothing realistic fits
    let out = run(&b, req(model, vec![msg("system", 400), msg("user", 12_000 * 3), msg("assistant", 12_000 * 3), msg("user", 4000)])).await;
    let err = out.error.expect("must fail");
    assert!(err.is_out_of_memory() && !err.is_server(), "{err:?}");
    assert!(err.message().contains("Start a new chat"), "{}", err.message());
    assert!(err.message().contains("graphics memory"), "{}", err.message());
    assert_eq!(b.received.lock().unwrap().len(), 2, "one retry only, no loop");
}

#[tokio::test]
async fn a_request_that_fits_in_memory_is_untouched() {
    let model = "qwen3.5-oom-fits:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    *b.oom_above_input_tokens.lock().unwrap() = Some(6_000);
    let out = run(&b, req(model, vec![msg("system", 400), msg("user", 800)])).await;
    assert!(out.error.is_none());
    assert_eq!(b.received.lock().unwrap().len(), 1);
    assert!(super::memory_ceiling(model).is_none());
}

// ---- issue 38: the key-value cache is sized for input + output, and free memory was never asked ----

use crate::actors::foundry::memory_budget::MemoryReading;

const KV_TEXT: &str = "command execution error: Error: Microsoft.ML.OnnxRuntimeGenAI.OnnxRuntimeGenAIException: Could not allocate the key-value cache buffer";

#[test]
fn the_key_value_cache_failure_is_an_out_of_memory_client_problem() {
    let e = BackendError::classify_generation(KV_TEXT);
    assert!(matches!(e, BackendError::Client(_)), "{e:?}");
    assert!(e.is_out_of_memory() && !e.is_server(), "must not blocklist the model");
}

#[tokio::test]
async fn free_memory_shrinks_what_is_asked_for_before_the_engine_is_called() {
    let model = "qwen3.5-kv-preflight:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    // A 24 GB card, 19 GB commit limit with about 12 GB free: the box that failed at max_length 29,590.
    *b.memory.lock().unwrap() = MemoryReading { free_gpu_mb: Some(20_000), free_commit_mb: Some(12_000) };
    // The engine refuses a cache for more than 29,000 core tokens in total (the figure it failed at was 29,590).
    *b.kv_cache_fails_above_max_length.lock().unwrap() = Some(29_000);
    // About 8,700 input tokens, like prompt 14 of the soak.
    let history = vec![msg("system", 400), msg("user", 15_000), msg("assistant", 15_000), msg("user", 40)];
    let out = run(&b, req(model, history)).await;
    assert!(out.error.is_none(), "{:?}", out.error);
    let seen = b.received.lock().unwrap();
    assert_eq!(seen.len(), 1, "no failed attempt first: {:?}", seen.iter().map(|s| (s.1.len(), s.2)).collect::<Vec<_>>());
    let max_length = seen[0].2 + super::super::context_budget::input_tokens(&seen[0].1);
    assert!(max_length < 29_590, "{max_length}");
}

#[tokio::test]
async fn a_chat_that_grows_is_trimmed_each_turn_as_free_memory_falls() {
    let model = "qwen3.5-kv-falling:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    let history = || vec![msg("system", 400), msg("user", 20_000), msg("assistant", 20_000), msg("user", 20_000), msg("assistant", 20_000), msg("user", 40)];
    *b.memory.lock().unwrap() = MemoryReading { free_gpu_mb: Some(20_000), free_commit_mb: Some(40_000) };
    run(&b, req(model, history())).await;
    *b.memory.lock().unwrap() = MemoryReading { free_gpu_mb: Some(20_000), free_commit_mb: Some(3_000) };
    run(&b, req(model, history())).await;
    let seen = b.received.lock().unwrap();
    assert!(seen[1].1.len() < seen[0].1.len(), "the second turn keeps less history: {} vs {}", seen[1].1.len(), seen[0].1.len());
}

#[tokio::test]
async fn a_cache_allocation_failure_the_preflight_missed_ends_the_turn_as_a_plain_failure_and_retry_trims_more() {
    let model = "qwen3.5-kv-reactive:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    // Memory is unreadable here (no pre-flight), and the engine refuses anything above 6,000 tokens in total.
    *b.kv_cache_fails_above_max_length.lock().unwrap() = Some(6_000);
    let history = || vec![msg("system", 400), msg("user", 15_000), msg("assistant", 15_000), msg("user", 40)];
    let out = run(&b, req(model, history())).await;
    // 8,700 input + 16,384 output, then 60% of the input: still too much. A normal failed turn, not a panic.
    let err = out.error.expect("both attempts fail");
    assert!(err.is_out_of_memory() && !err.is_server(), "{err:?}");
    assert!(err.message().contains("Retry"), "{}", err.message());
    let after_first_turn = b.received.lock().unwrap().len();
    // Pressing Retry sends the same chat again; it must now be trimmed below what just failed.
    let again = run(&b, req(model, history())).await;
    let seen = b.received.lock().unwrap();
    let first_sizes = super::super::context_budget::input_tokens(&seen[0].1);
    let retry_size = super::super::context_budget::input_tokens(&seen[after_first_turn].1);
    assert!(retry_size < first_sizes, "{retry_size} vs {first_sizes}");
    drop(again);
}

// ---- issue 38: an early-stopped stream must not leave its native generation decoding ----

use std::sync::atomic::Ordering as AtomicOrdering;

async fn run_ending_orphans(b: &FakeBackend, r: ChatStreamRequest) -> super::ChatStreamOutcome {
    let (tx, _rx) = mpsc::unbounded_channel();
    let (_c, crx) = watch::channel(false);
    super::chat_stream_ending_orphans(b, r, tx, crx).await
}

#[tokio::test]
async fn an_early_stopped_request_is_cancelled_before_the_next_request_begins() {
    let model = "qwen3.5-orphan-cancel:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    b.native_cancel_supported.store(true, AtomicOrdering::SeqCst);
    b.push(Script::EarlyStop);
    let first = run_ending_orphans(&b, req(model, vec![msg("user", 200)])).await;
    assert!(first.cancelled);
    let second = run_ending_orphans(&b, req(model, vec![msg("user", 200)])).await;
    assert!(second.error.is_none(), "{:?}", second.error);
    assert_eq!(b.native_cancel_calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(*b.native_active_at_request_start.lock().unwrap(), vec![0, 0], "no orphan was decoding when request 2 began");
}

#[tokio::test]
async fn a_stream_that_ended_normally_is_not_cancelled() {
    let model = "qwen3.5-orphan-none:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    b.native_cancel_supported.store(true, AtomicOrdering::SeqCst);
    run_ending_orphans(&b, req(model, vec![msg("user", 200)])).await;
    assert_eq!(b.native_cancel_calls.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn a_backend_that_cannot_cancel_is_reported_not_silently_assumed_clean() {
    // This is the real SDK 1.2.3: the early stop is attempted, the backend says Unsupported, the orphan stays.
    let model = "qwen3.5-orphan-unsupported:1";
    let b = FakeBackend::new().with_context(model, 262_144);
    b.push(Script::EarlyStop);
    run_ending_orphans(&b, req(model, vec![msg("user", 200)])).await;
    assert_eq!(b.native_cancel_calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(b.native_active.load(AtomicOrdering::SeqCst), 1, "the orphan is still counted, so the gap is visible");
}
