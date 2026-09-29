//! Pure helpers for the "model is incompatible with this Foundry runtime" experience: a short
//! human-readable reason for the blocklist, and the CPU variant of the same alias to offer.
//!
//! Kept free of actor/app state so they can be unit tested.

const MAX_REASON_CHARS: usize = 240;

/// Turn a raw runtime error into a short reason a user can act on. Known failure signatures get
/// an explanation; anything else is reduced to its first meaningful line and truncated.
pub fn friendly_incompatibility_reason(raw_error: &str, runtime_version: &str) -> String {
    let lower = raw_error.to_lowercase();
    if lower.contains("webgpu validation failed") {
        return format!(
            "This GPU build hits a WebGPU validation error in the Foundry runtime ({}). \
             A newer runtime may fix it; the CPU build of the same model still works but is slower.",
            runtime_version
        );
    }
    let first_line = raw_error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("The model failed to run");
    let mut reason: String = first_line.chars().take(MAX_REASON_CHARS).collect();
    if first_line.chars().count() > MAX_REASON_CHARS {
        reason.push('…');
    }
    format!("{} (Foundry runtime {})", reason, runtime_version)
}

/// The cached CPU variant of the same alias as `failed_model_id`, if there is one that is not
/// itself blocklisted. `qwen3.5-4b-generic-gpu:4` -> `qwen3.5-4b-generic-cpu:3`.
/// Returns `None` when the failed model is not a GPU build (nothing to swap to).
pub fn find_cpu_variant_of_same_alias(
    failed_model_id: &str,
    available_models: &[String],
    is_model_incompatible: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let failed_lower = failed_model_id.to_lowercase();
    // Version suffix (":4", or ":5:5" for oddly-formed ids) is dropped; it differs per variant.
    let failed_base = failed_lower.split(':').next().unwrap_or(&failed_lower);
    if !failed_base.contains("-gpu") {
        return None;
    }
    let cpu_base = failed_base.replacen("-gpu", "-cpu", 1);
    available_models
        .iter()
        .filter(|candidate| {
            let candidate_lower = candidate.to_lowercase();
            candidate_lower.split(':').next() == Some(cpu_base.as_str())
        })
        .find(|candidate| !is_model_incompatible(candidate))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none_incompatible(_: &str) -> bool {
        false
    }

    #[test]
    fn webgpu_validation_error_gets_an_explanation() {
        let raw = "Microsoft.ML.OnnxRuntimeGenAI.OnnxRuntimeGenAIException: WebGPU validation failed. [Buffer (unlabeled)] usage ...";
        let reason = friendly_incompatibility_reason(raw, "sdk-1.2.0");
        assert!(reason.contains("WebGPU validation error"));
        assert!(reason.contains("sdk-1.2.0"));
        assert!(reason.contains("CPU build"));
    }

    #[test]
    fn unknown_error_is_first_line_truncated_with_version() {
        let long = "x".repeat(400);
        let raw = format!("\n  {}\nsecond line", long);
        let reason = friendly_incompatibility_reason(&raw, "0.8.119");
        assert!(reason.starts_with(&"x".repeat(MAX_REASON_CHARS)));
        assert!(reason.contains('…'));
        assert!(reason.ends_with("(Foundry runtime 0.8.119)"));
        assert!(!reason.contains("second line"));
    }

    #[test]
    fn empty_error_still_has_a_reason() {
        assert!(friendly_incompatibility_reason("  \n ", "v").starts_with("The model failed to run"));
    }

    #[test]
    fn cpu_variant_of_same_alias_is_found() {
        let available = vec![
            "qwen3.5-4b-generic-cpu:3".to_string(),
            "Phi-4-mini-instruct-generic-gpu:5:5".to_string(),
        ];
        assert_eq!(
            find_cpu_variant_of_same_alias("qwen3.5-4b-generic-gpu:4", &available, &none_incompatible),
            Some("qwen3.5-4b-generic-cpu:3".to_string())
        );
    }

    #[test]
    fn matching_is_case_insensitive_and_exact_on_alias() {
        let available = vec!["QWEN3.5-4B-GENERIC-CPU:3".to_string(), "qwen3.5-4b-x-generic-cpu:1".to_string()];
        assert_eq!(
            find_cpu_variant_of_same_alias("qwen3.5-4b-generic-gpu:4", &available, &none_incompatible),
            Some("QWEN3.5-4B-GENERIC-CPU:3".to_string())
        );
        // Different alias must not match, even with a shared prefix.
        let other = vec!["qwen3.5-4b-x-generic-cpu:1".to_string()];
        assert_eq!(find_cpu_variant_of_same_alias("qwen3.5-4b-generic-gpu:4", &other, &none_incompatible), None);
    }

    #[test]
    fn no_swap_when_cpu_variant_absent_blocklisted_or_failed_model_is_cpu() {
        let gpu_only = vec!["qwen3.5-4b-generic-gpu:4".to_string()];
        assert_eq!(find_cpu_variant_of_same_alias("qwen3.5-4b-generic-gpu:4", &gpu_only, &none_incompatible), None);

        let cpu = vec!["qwen3.5-4b-generic-cpu:3".to_string()];
        assert_eq!(
            find_cpu_variant_of_same_alias("qwen3.5-4b-generic-gpu:4", &cpu, &|m: &str| m.contains("-cpu")),
            None
        );
        assert_eq!(find_cpu_variant_of_same_alias("qwen3.5-4b-generic-cpu:3", &cpu, &none_incompatible), None);
    }
}
