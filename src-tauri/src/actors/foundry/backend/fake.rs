//! A scriptable `FoundryBackend` for tests: no engine, no GPU, no network.
//!
//! It models what the real core does that broke in rc14: each model has a context window, and a
//! request asking for more than fits is rejected with the exact `System.ArgumentException` text.
//! It counts tokens more pessimistically than `context_budget` (as the real tokenizer does for
//! code and tables), so the estimator's margin and the overflow retry get exercised.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use super::{
    with_overflow_retry, NativeCancel, BackendError, Budget, ChatStreamOutcome, ChatStreamRequest, ConnectionInfo,
    FoundryBackend, ProgressSink, RawModel,
};
use crate::actors::foundry::context_budget;
use crate::protocol::{CachedModel, CatalogModel, ChatMessage, ModelFamily};

/// What the fake does for the next chat call (consumed in order; once empty, it answers).
#[derive(Debug, Clone)]
pub enum Script {
    /// Fail with this raw message, classified the way the SDK backend classifies it.
    Fail(String),
    /// Fail at load time.
    FailLoad(String),
    /// Answer after this text.
    Say(String),
    /// The reader stops after the first chunk (a complete tool call was seen): the stream ends `cancelled`
    /// and, like the real engine, the native decode behind it keeps running until it is told to stop.
    EarlyStop,
}

#[derive(Default)]
pub struct FakeBackend {
    /// model id -> context length
    pub contexts: Mutex<HashMap<String, u64>>,
    pub script: Mutex<VecDeque<Script>>,
    /// Every attempt the "core" received: (model, messages, max_tokens)
    pub received: Mutex<Vec<(String, Vec<ChatMessage>, u64)>>,
    pub downloads: Mutex<Vec<String>>,
    /// Errors the next downloads fail with, after reporting some progress (a connection cut mid-download).
    pub download_failures: Mutex<VecDeque<String>>,
    pub loads: Mutex<Vec<String>>,
    pub catalog: Mutex<Vec<CatalogModel>>,
    /// Tokens per character the fake core uses (the estimator assumes ~0.29).
    pub strict_tokens_per_char: Mutex<f64>,
    /// When set, the fake engine runs out of memory ("bad allocation", the real A10G text) building any request
    /// whose input is larger than this many core tokens, whatever the model's context window says.
    pub oom_above_input_tokens: Mutex<Option<u64>>,
    /// Free memory the fake machine reports before each request (None = unreadable, no memory limit).
    pub memory: Mutex<crate::actors::foundry::memory_budget::MemoryReading>,
    /// When set, the fake engine fails with the real 'key-value cache' text for any request whose
    /// input + max_tokens (its `max_length`) exceeds this many core tokens.
    pub kv_cache_fails_above_max_length: Mutex<Option<u64>>,
    /// Native generations still decoding (an early-stopped stream leaves one behind until cancelled).
    pub native_active: AtomicUsize,
    /// Whether this fake can stop a native generation (the real SDK 1.2.3 cannot).
    pub native_cancel_supported: AtomicBool,
    pub native_cancel_calls: AtomicUsize,
    /// `native_active` as each request began: must be 0 for every request after an early stop.
    pub native_active_at_request_start: Mutex<Vec<usize>>,
    /// Models the fake engine currently holds in memory.
    pub resident: Mutex<std::collections::HashSet<String>>,
    pub unloads: Mutex<Vec<String>>,
    /// The next this-many `load` calls fail (a reload that cannot get the model back).
    pub fail_next_loads: AtomicUsize,
    pub fail_next_unloads: AtomicUsize,
    /// Weights size the fake reports for every model.
    pub footprint_mb: Mutex<Option<u64>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        let f = Self::default();
        *f.strict_tokens_per_char.lock().unwrap() = 0.31;
        f
    }

    pub fn with_context(self, model: &str, ctx: u64) -> Self {
        self.contexts.lock().unwrap().insert(model.to_string(), ctx);
        self
    }

    pub fn fail_next_downloads(&self, times: usize, error: &str) {
        let mut q = self.download_failures.lock().unwrap();
        for _ in 0..times {
            q.push_back(error.to_string());
        }
    }

    pub fn push(&self, s: Script) {
        self.script.lock().unwrap().push_back(s);
    }

    fn core_tokens(&self, messages: &[ChatMessage]) -> u64 {
        let r = *self.strict_tokens_per_char.lock().unwrap();
        messages
            .iter()
            .map(|m| (m.content.chars().count() as f64 * r).ceil() as u64 + 4)
            .sum()
    }

    async fn attempt(&self, req: ChatStreamRequest, sink: UnboundedSender<String>, budget: Budget) -> ChatStreamOutcome {
        let mut outcome = ChatStreamOutcome::default();
        self.native_active_at_request_start.lock().unwrap().push(self.native_active.load(Ordering::SeqCst));
        let ctx = self.contexts.lock().unwrap().get(&req.model).copied();
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(Script::FailLoad(m)) => {
                outcome.error = Some(BackendError::classify_load(m));
                return outcome;
            }
            Some(Script::Fail(m)) => {
                outcome.error = Some(BackendError::classify_generation(m));
                return outcome;
            }
            Some(Script::EarlyStop) => {
                let _ = sink.send("<tool_call>".to_string());
                self.native_active.fetch_add(1, Ordering::SeqCst);
                outcome.cancelled = true;
                return outcome;
            }
            _ => {}
        }
        let cap = match req.family {
            ModelFamily::Gemma | ModelFamily::Granite => 8192,
            _ => 16384,
        };
        let planned_ctx = crate::actors::foundry::memory_budget::effective_context(ctx, &self.memory.lock().unwrap());
        let plan = context_budget::plan_request(&req.messages, planned_ctx, cap, budget.scale, budget.input_ceiling);
        outcome.input_tokens = plan.input_tokens;
        self.received
            .lock()
            .unwrap()
            .push((req.model.clone(), plan.messages.clone(), plan.max_tokens));
        // The "core" can also run out of memory on a long prompt, independent of the window.
        if let Some(limit) = *self.oom_above_input_tokens.lock().unwrap() {
            if self.core_tokens(&plan.messages) > limit {
                outcome.error = Some(BackendError::classify_generation(
                    "command execution error: Error: Microsoft.ML.OnnxRuntimeGenAI.OnnxRuntimeGenAIException: Non-zero status code returned while running SimplifiedLayerNormalization node. Name:'/model/layers.3/attn/k_norm/SimplifiedLayerNormalization' Status Message: bad allocation",
                ));
                return outcome;
            }
        }
        if let Some(limit) = *self.kv_cache_fails_above_max_length.lock().unwrap() {
            if self.core_tokens(&plan.messages) + plan.max_tokens > limit {
                outcome.error = Some(BackendError::classify_generation(
                    "command execution error: Error: Microsoft.ML.OnnxRuntimeGenAI.OnnxRuntimeGenAIException: Could not allocate the key-value cache buffer",
                ));
                return outcome;
            }
        }
        // The "core": input + max_tokens must fit the true window, counted pessimistically.
        let real_ctx = ctx.unwrap_or(context_budget::DEFAULT_CONTEXT_LENGTH);
        let total = self.core_tokens(&plan.messages) + plan.max_tokens;
        if total > real_ctx {
            outcome.error = Some(BackendError::classify_generation(format!(
                "command execution error: Error: System.ArgumentException: This request requires {total} total tokens, which exceeds the model's maximum context length of {real_ctx} tokens. Please reduce 'max_tokens'or shorten your input... (Foundry runtime sdk-1.2.3+core-1.2.4)"
            )));
            return outcome;
        }
        let _ = sink.send("ok".to_string());
        outcome
    }
}

#[async_trait::async_trait]
impl FoundryBackend for FakeBackend {
    async fn ensure_service(&self) -> Result<(), String> { Ok(()) }
    async fn stop_service(&self) -> Result<(), String> { Ok(()) }
    async fn restart_service(&self) -> Result<(), String> { Ok(()) }
    async fn connection_info(&self) -> ConnectionInfo { ConnectionInfo::default() }
    async fn list_models(&self) -> Vec<RawModel> { Vec::new() }
    async fn list_catalog(&self) -> Vec<CatalogModel> { self.catalog.lock().unwrap().clone() }
    async fn list_cached(&self) -> Vec<CachedModel> { Vec::new() }
    async fn list_loaded(&self) -> Vec<String> { self.loads.lock().unwrap().clone() }

    async fn load(&self, model: &str) -> Result<(), BackendError> {
        if self.fail_next_loads.load(Ordering::SeqCst) > 0 {
            self.fail_next_loads.fetch_sub(1, Ordering::SeqCst);
            return Err(BackendError::Server("fake: could not load the model".to_string()));
        }
        self.loads.lock().unwrap().push(model.to_string());
        self.resident.lock().unwrap().insert(model.to_string());
        Ok(())
    }
    async fn unload(&self, model: &str) -> Result<(), String> {
        if self.fail_next_unloads.load(Ordering::SeqCst) > 0 {
            self.fail_next_unloads.fetch_sub(1, Ordering::SeqCst);
            return Err("fake: could not unload the model".to_string());
        }
        self.unloads.lock().unwrap().push(model.to_string());
        self.resident.lock().unwrap().remove(model);
        Ok(())
    }
    async fn cancel_native_generation(&self, _model: &str) -> NativeCancel {
        self.native_cancel_calls.fetch_add(1, Ordering::SeqCst);
        if self.native_cancel_supported.load(Ordering::SeqCst) {
            self.native_active.store(0, Ordering::SeqCst);
            NativeCancel::Cancelled
        } else {
            NativeCancel::Unsupported
        }
    }
    async fn model_footprint_mb(&self, _model: &str) -> Option<u64> { *self.footprint_mb.lock().unwrap() }

    async fn download(
        &self,
        model: &str,
        mut progress: ProgressSink,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<(), String> {
        let cut = self.download_failures.lock().unwrap().pop_front();
        for p in [10.0, 50.0, 100.0] {
            if cancel.as_ref().map_or(false, |c| c.load(Ordering::SeqCst)) {
                return Err("The operation was canceled.".to_string());
            }
            progress(p);
            if let (Some(e), true) = (&cut, p >= 50.0) {
                return Err(e.clone());
            }
        }
        self.downloads.lock().unwrap().push(model.to_string());
        Ok(())
    }
    async fn remove_cached(&self, _model: &str) -> Result<(), String> { Ok(()) }

    async fn chat_stream(
        &self,
        req: ChatStreamRequest,
        sink: UnboundedSender<String>,
        _cancel: watch::Receiver<bool>,
    ) -> ChatStreamOutcome {
        let model = req.model.clone();
        with_overflow_retry(&model, |budget| self.attempt(req.clone(), sink.clone(), budget)).await
    }

    fn runtime_version_key(&self) -> String { "fake-1.0".to_string() }
}
