//! Pure startup model selection: launch override -> settings -> default -> fallback.
//!
//! Kept free of actor/app state so the ordering can be unit tested. The gateway actor gathers
//! the inputs (settings, blocklist, cached models) and calls [`select_startup_model`].

use super::{DEFAULT_FALLBACK_MODEL, DEFAULT_MODEL};
#[cfg(test)]
use crate::protocol::ModelState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupModelSource {
    LaunchOverride,
    Settings,
    DefaultModel,
    FallbackModel,
}

impl StartupModelSource {
    pub fn label(self) -> &'static str {
        match self {
            StartupModelSource::LaunchOverride => "launch override",
            StartupModelSource::Settings => "settings",
            StartupModelSource::DefaultModel => "default model",
            StartupModelSource::FallbackModel => "fallback - settings and default models not available",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupModelSelection {
    pub selected_model: Option<(String, StartupModelSource)>,
    /// Set when a launch override was requested but could not be honoured. Shown to the user.
    pub launch_override_problem: Option<String>,
}

/// Resolve a requested model name against the cached models: exact (case-insensitive) match,
/// or a unique `"<name>-..."` / `"<name>:..."` prefix match so `qwen3.5-4b` can name one variant.
fn resolve_requested_model<'a>(requested: &str, available_models: &'a [String]) -> Option<&'a String> {
    let requested_lower = requested.trim().to_lowercase();
    if let Some(exact) = available_models.iter().find(|m| m.to_lowercase() == requested_lower) {
        return Some(exact);
    }
    let prefix_matches: Vec<&String> = available_models
        .iter()
        .filter(|m| {
            let lower = m.to_lowercase();
            lower.starts_with(&format!("{}-", requested_lower))
                || lower.starts_with(&format!("{}:", requested_lower))
        })
        .collect();
    if prefix_matches.len() == 1 {
        Some(prefix_matches[0])
    } else {
        None
    }
}

pub fn select_startup_model(
    launch_override_model: Option<&str>,
    persisted_model: Option<&str>,
    available_models: &[String],
    is_model_incompatible: &dyn Fn(&str) -> bool,
) -> StartupModelSelection {
    let mut launch_override_problem = None;

    if let Some(requested) = launch_override_model.map(str::trim).filter(|m| !m.is_empty()) {
        match resolve_requested_model(requested, available_models) {
            Some(model) if !is_model_incompatible(model) => {
                return StartupModelSelection {
                    selected_model: Some((model.clone(), StartupModelSource::LaunchOverride)),
                    launch_override_problem: None,
                };
            }
            Some(model) => {
                launch_override_problem = Some(format!(
                    "Requested model '{}' is marked incompatible with the installed Foundry runtime; using the normal startup model instead.",
                    model
                ));
            }
            None => {
                launch_override_problem = Some(format!(
                    "Requested model '{}' is not downloaded (cached models: {}); using the normal startup model instead.",
                    requested,
                    if available_models.is_empty() { "none".to_string() } else { available_models.join(", ") }
                ));
            }
        }
    }

    let usable = |candidate: &&String| !is_model_incompatible(candidate);
    let persisted = persisted_model
        .and_then(|pm| available_models.iter().find(|m| m.as_str() == pm))
        .filter(usable);
    let default_model = available_models
        .iter()
        .filter(usable)
        .find(|m| m.to_lowercase().contains(DEFAULT_MODEL));
    let fallback_model = available_models
        .iter()
        .filter(usable)
        .find(|m| m.to_lowercase().contains(DEFAULT_FALLBACK_MODEL));

    let selected_model = persisted
        .map(|m| (m.clone(), StartupModelSource::Settings))
        .or_else(|| default_model.map(|m| (m.clone(), StartupModelSource::DefaultModel)))
        .or_else(|| fallback_model.map(|m| (m.clone(), StartupModelSource::FallbackModel)));

    StartupModelSelection { selected_model, launch_override_problem }
}

/// True only when the model state is `Ready` on exactly `model_id`. The launch prompt waits on
/// this (mirrored by the frontend) so it never runs while another model is selected or switching in.
#[allow(dead_code)]
pub fn is_ready_with_model(model_state: &crate::protocol::ModelState, model_id: &str) -> bool {
    matches!(model_state, crate::protocol::ModelState::Ready { model_id: ready } if ready == model_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_with_model_requires_ready_and_matching_id() {
        let ready = ModelState::Ready { model_id: "qwen3.5-4b-generic-gpu:4".into() };
        assert!(is_ready_with_model(&ready, "qwen3.5-4b-generic-gpu:4"));
        assert!(!is_ready_with_model(&ready, "qwen3-4b-generic-gpu:2"));
    }

    #[test]
    fn switching_and_loading_states_are_not_ready_for_the_target() {
        let target = "qwen3.5-4b-generic-gpu:4";
        let switching = ModelState::SwitchingModel { from: Some("qwen3-4b-generic-gpu:2".into()), to: target.into() };
        let loading = ModelState::LoadingModel { model_id: target.into() };
        assert!(!is_ready_with_model(&switching, target));
        assert!(!is_ready_with_model(&loading, target));
        assert!(!is_ready_with_model(&ModelState::Initializing, target));
        assert!(switching.blocks_prompts() && loading.blocks_prompts());
    }

    fn cached(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    fn none_incompatible(_: &str) -> bool {
        false
    }

    #[test]
    fn launch_override_wins_over_settings() {
        let available = cached(&["qwen3-4b-generic-gpu:2", "qwen3.5-4b-generic-gpu:4"]);
        let selection = select_startup_model(
            Some("qwen3.5-4b-generic-gpu:4"),
            Some("qwen3-4b-generic-gpu:2"),
            &available,
            &none_incompatible,
        );
        assert_eq!(
            selection.selected_model,
            Some(("qwen3.5-4b-generic-gpu:4".to_string(), StartupModelSource::LaunchOverride))
        );
        assert!(selection.launch_override_problem.is_none());
    }

    #[test]
    fn without_override_settings_then_default_then_fallback() {
        let available = cached(&["qwen3-4b-generic-gpu:2", "qwen3.5-4b-generic-gpu:4", "phi-4-mini-instruct-generic-gpu:1"]);
        let from_settings = select_startup_model(None, Some("qwen3-4b-generic-gpu:2"), &available, &none_incompatible);
        assert_eq!(from_settings.selected_model.unwrap().1, StartupModelSource::Settings);

        let from_default = select_startup_model(None, None, &available, &none_incompatible);
        assert_eq!(from_default.selected_model.unwrap().1, StartupModelSource::DefaultModel);

        let only_phi = cached(&["phi-4-mini-instruct-generic-gpu:1"]);
        let from_fallback = select_startup_model(None, None, &only_phi, &none_incompatible);
        assert_eq!(from_fallback.selected_model.unwrap().1, StartupModelSource::FallbackModel);
    }

    #[test]
    fn unavailable_override_reports_problem_and_uses_normal_order() {
        let available = cached(&["qwen3-4b-generic-gpu:2"]);
        let selection = select_startup_model(
            Some("qwen3.5-4b-generic-gpu:4"),
            Some("qwen3-4b-generic-gpu:2"),
            &available,
            &none_incompatible,
        );
        assert_eq!(selection.selected_model.unwrap().1, StartupModelSource::Settings);
        assert!(selection.launch_override_problem.unwrap().contains("not downloaded"));
    }

    #[test]
    fn incompatible_override_reports_problem() {
        let available = cached(&["qwen3-4b-generic-gpu:2", "qwen3.5-4b-generic-gpu:4"]);
        let is_incompatible = |m: &str| m == "qwen3.5-4b-generic-gpu:4";
        let selection = select_startup_model(
            Some("qwen3.5-4b-generic-gpu:4"),
            Some("qwen3-4b-generic-gpu:2"),
            &available,
            &is_incompatible,
        );
        assert_eq!(selection.selected_model.unwrap().0, "qwen3-4b-generic-gpu:2");
        assert!(selection.launch_override_problem.unwrap().contains("incompatible"));
    }

    #[test]
    fn override_matches_case_insensitively_and_by_unique_prefix() {
        let available = cached(&["qwen3-4b-generic-gpu:2", "qwen3.5-4b-generic-gpu:4"]);
        let by_case = select_startup_model(Some("QWEN3.5-4B-GENERIC-GPU:4"), None, &available, &none_incompatible);
        assert_eq!(by_case.selected_model.unwrap().0, "qwen3.5-4b-generic-gpu:4");
        let by_prefix = select_startup_model(Some("qwen3.5-4b"), None, &available, &none_incompatible);
        assert_eq!(by_prefix.selected_model.unwrap().0, "qwen3.5-4b-generic-gpu:4");

        let ambiguous = cached(&["qwen3.5-4b-generic-gpu:4", "qwen3.5-4b-generic-cpu:2"]);
        let by_ambiguous_prefix = select_startup_model(Some("qwen3.5-4b"), None, &ambiguous, &none_incompatible);
        assert!(by_ambiguous_prefix.launch_override_problem.is_some());
    }

    #[test]
    fn nothing_usable_selects_nothing() {
        let selection = select_startup_model(None, None, &cached(&["some-other-model:1"]), &none_incompatible);
        assert!(selection.selected_model.is_none());
    }
}
