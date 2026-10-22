//! A scriptable `FoundryBackend` for tests: no engine, no GPU, no network.
//!
//! It models what the real core does that broke in rc14: each model has a context window, and a
//! request asking for more than fits is rejected with the exact `System.ArgumentException` text.
//! It counts tokens more pessimistically than `context_budget` (as the real tokenizer does for
//! code and tables), so the estimator's margin and the overflow retry get exercised.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use super::{
    with_overflow_retry, BackendError, ChatStreamOutcome, ChatStreamRequest, ConnectionInfo,
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
}

#[derive(Default)]
pub struct FakeBackend {
    /// model id -> context length
    pub contexts: Mutex<HashMap<String, u64>>,
    pub script: Mutex<VecDeque<Script>>,
    /// Every attempt the "core" received: (model, messages, max_tokens)
    pub received: Mutex<Vec<(String, Vec<ChatMessage>, u64)>>,
    pub downloads: Mutex<Vec<String>>,
    pub loads: Mutex<Vec<String>>,
    pub catalog: Mutex<Vec<CatalogModel>>,
    /// Tokens per character the fake core uses (the estimator assumes ~0.29).
    pub strict_tokens_per_char: Mutex<f64>,
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

    async fn attempt(&self, req: ChatStreamRequest, sink: UnboundedSender<String>, scale: f64) -> ChatStreamOutcome {
        let mut outcome = ChatStreamOutcome::default();
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
            _ => {}
        }
        let cap = match req.family {
            ModelFamily::Gemma | ModelFamily::Granite => 8192,
            _ => 16384,
        };
        let plan = context_budget::plan_request(&req.messages, ctx, cap, scale);
        self.received
            .lock()
            .unwrap()
            .push((req.model.clone(), plan.messages.clone(), plan.max_tokens));
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
        self.loads.lock().unwrap().push(model.to_string());
        Ok(())
    }
    async fn unload(&self, _model: &str) -> Result<(), String> { Ok(()) }

    async fn download(
        &self,
        model: &str,
        mut progress: ProgressSink,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<(), String> {
        for p in [10.0, 50.0, 100.0] {
            if cancel.as_ref().map_or(false, |c| c.load(Ordering::SeqCst)) {
                return Err("The operation was canceled.".to_string());
            }
            progress(p);
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
        with_overflow_retry(|scale| self.attempt(req.clone(), sink.clone(), scale)).await
    }

    fn runtime_version_key(&self) -> String { "fake-1.0".to_string() }
}
