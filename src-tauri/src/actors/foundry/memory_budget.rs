//! Sizes a request by the memory this computer has free right now.
//!
//! The context budget only knew the model's window (262,144 tokens for Qwen). On an A10G box with about 19 GB of
//! Windows commit memory, a same-chat run asked for `max_length` 29,590 (about 8,700 input tokens plus a 16,384
//! output cap) and the engine failed with "Could not allocate the key-value cache buffer"; at another prompt the
//! process aborted on "memory allocation of 5242880 bytes failed" (issue 38). The key-value cache is sized for
//! input plus output, so the cap here limits that total, from free graphics memory and free commit memory.

use std::process::Command;

/// What the machine has free at this moment. `None` = could not be read (then that limit is not applied).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryReading {
    pub free_gpu_mb: Option<u64>,
    pub free_commit_mb: Option<u64>,
}

/// Never plan for less than this many input + output tokens: below it the failure is not about length.
pub const FLOOR_TOTAL_TOKENS: u64 = 2_048;
/// Never plan for more than this, however much memory looks free. The failing request was 29,590.
pub const CEILING_TOTAL_TOKENS: u64 = 24_576;

/// Key-value cache cost per token, in KB. Measured from qwen3.5-4b's real shape (the default model; the
/// numbers are in its `genai_config.json` and the engine log of the A10G soak, issue 38): 8 full-attention
/// layers x 4 key-value heads x head size 256 x 2 (keys and values) x 2 bytes (FP16) = 32 KB a token.
/// The earlier 160 KB was an estimate for a 36-layer, 8-head model and was 5x too high for this one.
const KV_LAYERS: u64 = 8;
const KV_HEADS: u64 = 4;
const KV_HEAD_SIZE: u64 = 256;
const GPU_KB_PER_TOKEN: u64 = KV_LAYERS * KV_HEADS * KV_HEAD_SIZE * 2 * 2 / 1024;
/// Host-side cost per token: the engine stages the buffer in system memory too. Assumed twice the card cost
/// (an estimate, not a measurement).
const COMMIT_KB_PER_TOKEN: u64 = GPU_KB_PER_TOKEN * 2;
/// Share of the free figure one request may claim. Weights, the next request and other programs need the rest.
const GPU_SHARE_PCT: u64 = 60;
const COMMIT_SHARE_PCT: u64 = 40;
/// Card memory the engine needs besides the cache (activations, workspace). Taken off the free figure first, so
/// a card with under this much free floors the request instead of promising it a cache it cannot have.
const GPU_WORKING_RESERVE_MB: u64 = 1_024;
/// Free commit below this says nothing about what a request may use: in the soak it sat at 500 to 800 MB for
/// many turns while requests kept working (Windows grows the page file, so the limit moves). Commit only caps
/// a request when it is above this headroom and still the tighter of the two.
const MIN_INFORMATIVE_COMMIT_MB: u64 = 1_024;
/// What a request may still ask for while commit is that low. The reading proves nothing about the limit, but
/// the rc19 box run died asking for ~29,600 tokens with commit nearly gone, so the ceiling is not safe either:
/// 8,192 tokens is about 256 MB of cache.
const LOW_COMMIT_TOTAL_TOKENS: u64 = 8_192;

/// The most input + output tokens one request may ask for, or `None` when nothing could be read.
pub fn total_token_cap(r: &MemoryReading) -> Option<u64> {
    let from_gpu = r.free_gpu_mb.map(|mb| {
        mb.saturating_sub(GPU_WORKING_RESERVE_MB) * 1024 * GPU_SHARE_PCT / 100 / GPU_KB_PER_TOKEN
    });
    let from_commit = r
        .free_commit_mb
        .filter(|mb| *mb >= MIN_INFORMATIVE_COMMIT_MB)
        .map(|mb| mb * 1024 * COMMIT_SHARE_PCT / 100 / COMMIT_KB_PER_TOKEN);
    let commit_is_low = r.free_commit_mb.is_some_and(|mb| mb < MIN_INFORMATIVE_COMMIT_MB);
    let tightest = match (from_gpu, from_commit) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        // A commit reading too low to be informative, and nothing else readable: no limit from memory.
        (None, None) => return None,
    };
    let tightest = if commit_is_low { tightest.min(LOW_COMMIT_TOTAL_TOKENS) } else { tightest };
    Some(tightest.clamp(FLOOR_TOTAL_TOKENS, CEILING_TOTAL_TOKENS))
}

/// The window to plan against: the model's own, shrunk to what memory allows.
pub fn effective_context(model_context: Option<u64>, r: &MemoryReading) -> Option<u64> {
    match (model_context.filter(|c| *c > 0), total_token_cap(r)) {
        (Some(c), Some(cap)) => Some(c.min(cap)),
        (None, Some(cap)) => Some(cap.min(super::context_budget::DEFAULT_CONTEXT_LENGTH)),
        (c, None) => c,
    }
}

/// Free memory of the first NVIDIA card with the most free, from `nvidia-smi` (MiB, one line per card).
pub fn parse_nvidia_smi_free_mb(output: &str) -> Option<u64> {
    output.lines().filter_map(|l| l.trim().parse::<u64>().ok()).max()
}

fn probe_free_gpu_mb() -> Option<u64> {
    use crate::process_utils::HideConsoleWindow;
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
        .hide_console_window()
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nvidia_smi_free_mb(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn probe_free_commit_mb() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut s = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `s` is a correctly sized, initialised MEMORYSTATUSEX that lives for the call.
    if unsafe { GlobalMemoryStatusEx(&mut s) } == 0 {
        return None;
    }
    // "Page file" here is the system commit limit minus what is committed: the headroom before allocations fail.
    Some(s.avail_page_file / (1024 * 1024))
}

#[cfg(not(windows))]
fn probe_free_commit_mb() -> Option<u64> {
    None
}

/// Blocking: spawns `nvidia-smi` (about 50 ms) and calls one Windows API.
pub fn probe() -> MemoryReading {
    MemoryReading { free_gpu_mb: probe_free_gpu_mb(), free_commit_mb: probe_free_commit_mb() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(gpu: Option<u64>, commit: Option<u64>) -> MemoryReading {
        MemoryReading { free_gpu_mb: gpu, free_commit_mb: commit }
    }

    #[test]
    fn a_24gb_card_with_19gb_of_commit_cannot_ask_for_the_29590_tokens_that_failed() {
        // 19 GB commit limit, about 12 GB of it free once the model is loaded; 20 GB of the card free.
        let cap = total_token_cap(&r(Some(20_000), Some(12_000))).unwrap();
        assert!(cap < 29_590, "{cap}");
        assert!(cap >= 16_000, "a healthy box keeps a long window: {cap}");
    }

    #[test]
    fn the_tighter_of_the_two_memories_decides() {
        let gpu_bound = total_token_cap(&r(Some(1_500), Some(60_000))).unwrap();
        let commit_bound = total_token_cap(&r(Some(40_000), Some(1_500))).unwrap();
        assert!(gpu_bound < total_token_cap(&r(Some(40_000), Some(60_000))).unwrap());
        assert!(commit_bound < total_token_cap(&r(Some(40_000), Some(60_000))).unwrap());
    }

    #[test]
    fn a_6gb_machine_gets_a_small_but_usable_window() {
        // At 32 KB a token, 4 GB of card and 3 GB of commit really do hold a long window.
        let cap = total_token_cap(&r(Some(4_000), Some(3_000))).unwrap();
        assert!((4_096..=CEILING_TOTAL_TOKENS).contains(&cap), "{cap}");
    }

    #[test]
    fn the_floor_and_ceiling_hold() {
        assert_eq!(total_token_cap(&r(Some(10), Some(10))), Some(FLOOR_TOTAL_TOKENS));
        assert_eq!(total_token_cap(&r(Some(900_000), Some(900_000))), Some(CEILING_TOTAL_TOKENS));
    }

    #[test]
    fn nothing_readable_applies_no_limit() {
        assert_eq!(total_token_cap(&r(None, None)), None);
        assert_eq!(effective_context(Some(262_144), &r(None, None)), Some(262_144));
        assert_eq!(effective_context(None, &r(None, None)), None);
    }

    #[test]
    fn one_readable_memory_is_enough_to_limit() {
        assert!(total_token_cap(&r(Some(20_000), None)).is_some());
        assert!(total_token_cap(&r(None, Some(8_000))).is_some());
    }

    #[test]
    fn the_window_is_the_models_own_unless_memory_is_tighter() {
        assert_eq!(effective_context(Some(8_192), &r(Some(40_000), Some(40_000))), Some(8_192));
        assert!(effective_context(Some(262_144), &r(Some(20_000), Some(12_000))).unwrap() <= CEILING_TOTAL_TOKENS);
    }

    #[test]
    fn the_soak_evidence_case_no_longer_pins_the_request_to_a_tiny_window() {
        // A10G soak, prompt ~14: 11,866 MB of the card free, only 803 MB of commit free, input ~2,129 tokens.
        // The old per-token costs gave a total of 2,876 (max_tokens 747); that commit reading had stopped
        // being informative (the commit limit grows), and the cache really costs 32 KB a token.
        let cap = total_token_cap(&r(Some(11_866), Some(803))).unwrap();
        assert!(cap >= 4_096, "{cap}");
        let max_tokens = cap.saturating_sub(2_129);
        assert!(max_tokens >= 2_048, "max_tokens would be {max_tokens}");
    }

    #[test]
    fn a_truly_tiny_free_card_still_floors_whatever_the_commit_says() {
        assert_eq!(total_token_cap(&r(Some(800), Some(60_000))), Some(FLOOR_TOTAL_TOKENS));
        assert_eq!(total_token_cap(&r(Some(1_024), None)), Some(FLOOR_TOTAL_TOKENS));
    }

    #[test]
    fn low_commit_is_ignored_below_the_minimum_headroom_and_binds_above_it() {
        let ignored = total_token_cap(&r(Some(40_000), Some(MIN_INFORMATIVE_COMMIT_MB - 1))).unwrap();
        assert_eq!(ignored, LOW_COMMIT_TOTAL_TOKENS, "below the headroom commit no longer sizes the request, but it still holds it to a modest one");
        let binding = total_token_cap(&r(Some(40_000), Some(MIN_INFORMATIVE_COMMIT_MB + 200))).unwrap();
        assert!(binding < CEILING_TOTAL_TOKENS && binding >= FLOOR_TOTAL_TOKENS, "{binding}");
    }

    #[test]
    fn the_per_token_cost_is_the_real_cache_shape() {
        // 4 KV heads x head size 256 x 8 full-attention layers x K and V x 2 bytes.
        assert_eq!(GPU_KB_PER_TOKEN, 4 * 256 * 8 * 2 * 2 / 1024);
        assert_eq!(GPU_KB_PER_TOKEN, 32);
    }

    #[test]
    fn nvidia_smi_output_is_read_as_mib_and_the_largest_card_wins() {
        assert_eq!(parse_nvidia_smi_free_mb("22912\n"), Some(22_912));
        assert_eq!(parse_nvidia_smi_free_mb("1024\r\n15000\r\n"), Some(15_000));
        assert_eq!(parse_nvidia_smi_free_mb("NVIDIA-SMI has failed because it couldn't communicate"), None);
    }
}
