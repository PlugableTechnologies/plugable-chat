//! Keeps a chat request inside the model's context window.
//!
//! rc14 asked every model for 16384 output tokens. A model with an 8192-token context then
//! rejected the request ("requires 17537 total tokens … exceeds … 8192"), and the failure was
//! mistaken for an incompatible model. The budget below clamps `max_tokens` to what is left
//! after the input, and trims old history when the input alone doesn't fit.

use crate::protocol::ChatMessage;

/// Used when the catalog doesn't report a context length.
pub const DEFAULT_CONTEXT_LENGTH: u64 = 4096;
/// Headroom for chat-template tokens and estimator error.
const SAFETY_MARGIN: u64 = 128;
/// Never ask for fewer output tokens than this; below it, trim input instead.
const MIN_OUTPUT_TOKENS: u64 = 256;

/// Rough token count (~3.5 chars per token, rounded up, plus per-message framing).
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as f64 / 3.5).ceil() as u64
}

fn message_tokens(m: &ChatMessage) -> u64 {
    let calls: u64 = m
        .tool_calls
        .as_ref()
        .map(|cs| cs.iter().map(|c| estimate_tokens(&format!("{:?}", c))).sum())
        .unwrap_or(0);
    estimate_tokens(&m.content) + calls + 4
}

pub fn input_tokens(messages: &[ChatMessage]) -> u64 {
    messages.iter().map(message_tokens).sum()
}

/// `min(cap, context - input - margin)`, floored at `MIN_OUTPUT_TOKENS` only when it fits.
/// Returns `None` when even the minimum output doesn't fit (caller must trim input).
pub fn clamp_max_tokens(cap: u64, context_length: Option<u64>, input: u64) -> Option<u64> {
    let ctx = context_length.filter(|c| *c > 0).unwrap_or(DEFAULT_CONTEXT_LENGTH);
    let room = ctx.checked_sub(input + SAFETY_MARGIN)?;
    if room < MIN_OUTPUT_TOKENS.min(cap) {
        return None;
    }
    Some(cap.min(room))
}

/// Drops the oldest non-system turns until the request fits with at least a quarter of the
/// context (or `MIN_OUTPUT_TOKENS`) left for output. Always keeps system messages and the
/// final message. Returns how many messages were dropped.
pub fn trim_to_fit(messages: &mut Vec<ChatMessage>, context_length: Option<u64>) -> usize {
    let ctx = context_length.filter(|c| *c > 0).unwrap_or(DEFAULT_CONTEXT_LENGTH);
    let reserve = (ctx / 4).max(MIN_OUTPUT_TOKENS);
    let budget = ctx.saturating_sub(reserve + SAFETY_MARGIN);
    let mut dropped = 0;
    while input_tokens(messages) > budget {
        // Oldest droppable: not a system message, and not the last message.
        let last = messages.len().saturating_sub(1);
        match messages.iter().enumerate().position(|(i, m)| i < last && m.role != "system") {
            Some(i) => {
                messages.remove(i);
                dropped += 1;
            }
            None => break,
        }
    }
    dropped
}

/// A request fitted to a context window.
#[derive(Debug, Clone)]
pub struct RequestPlan {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u64,
    pub dropped: usize,
    pub input_tokens: u64,
}

/// Trims history and picks `max_tokens` for one request. `budget_scale` < 1 shrinks the window
/// we plan against (the retry after the core still found the request too large).
pub fn plan_request(
    messages: &[ChatMessage],
    context_length: Option<u64>,
    family_cap: u64,
    budget_scale: f64,
) -> RequestPlan {
    let ctx = context_length.map(|c| (c as f64 * budget_scale) as u64);
    let mut messages = messages.to_vec();
    let dropped = trim_to_fit(&mut messages, ctx);
    let input_tokens = input_tokens(&messages);
    // Input alone filled the window: ask for the minimum and let the core report it.
    let max_tokens = clamp_max_tokens(family_cap, ctx, input_tokens).unwrap_or(MIN_OUTPUT_TOKENS);
    RequestPlan { messages, max_tokens, dropped, input_tokens }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, chars: usize) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: "x".repeat(chars),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn rc14_gpt_oss_case_is_clamped_below_the_context() {
        // 1153 input tokens, 16384 requested, 8192 context (the reported failure).
        let m = clamp_max_tokens(16384, Some(8192), 1153).unwrap();
        assert!(m + 1153 <= 8192, "{m}");
        assert!(m >= 256);
    }

    #[test]
    fn large_context_keeps_the_family_cap() {
        assert_eq!(clamp_max_tokens(16384, Some(131072), 2000), Some(16384));
    }

    #[test]
    fn unknown_context_uses_a_safe_default() {
        let m = clamp_max_tokens(16384, None, 500).unwrap();
        assert!(m + 500 <= DEFAULT_CONTEXT_LENGTH);
        assert!(clamp_max_tokens(16384, Some(0), 500).is_some());
    }

    #[test]
    fn input_that_fills_the_context_needs_trimming() {
        assert_eq!(clamp_max_tokens(16384, Some(8192), 8192), None);
        assert_eq!(clamp_max_tokens(16384, Some(8192), 9000), None);
        assert_eq!(clamp_max_tokens(16384, Some(8192), 8000), None);
    }

    #[test]
    fn a_small_cap_is_not_inflated() {
        assert_eq!(clamp_max_tokens(100, Some(8192), 100), Some(100));
    }

    #[test]
    fn trim_keeps_system_and_latest_turn_and_drops_oldest() {
        let mut v = vec![
            msg("system", 400),
            msg("user", 8000),
            msg("assistant", 8000),
            msg("user", 8000),
            msg("assistant", 8000),
            msg("user", 400),
        ];
        let dropped = trim_to_fit(&mut v, Some(8192));
        assert!(dropped >= 2, "dropped {dropped}");
        assert_eq!(v.first().unwrap().role, "system");
        assert_eq!(v.last().unwrap().content.len(), 400);
        assert!(clamp_max_tokens(16384, Some(8192), input_tokens(&v)).is_some());
    }

    #[test]
    fn trim_is_a_no_op_when_it_fits() {
        let mut v = vec![msg("system", 100), msg("user", 100)];
        assert_eq!(trim_to_fit(&mut v, Some(8192)), 0);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn trim_never_drops_a_single_oversized_last_message() {
        let mut v = vec![msg("system", 100), msg("user", 100_000)];
        trim_to_fit(&mut v, Some(8192));
        assert_eq!(v.len(), 2);
    }
}
