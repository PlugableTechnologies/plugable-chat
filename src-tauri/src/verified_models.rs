//! Models Plugable has tested end to end, and how the rest are treated.
//!
//! A model is "verified" when the release gate (the Chicago questions on a real GPU box, see
//! `docs/gpu-validation.md` and `infra/aws-gpu/chicago-questions.json`) has run it through the
//! installed app. Everything else still works if the runtime can load it, but the picker shows it
//! under "Experimental (not tested by Plugable)" with a warning, and the automatic fallback
//! (`startup_model_selection::any_compatible`) prefers verified models.
//!
//! The same list is mirrored in `src/lib/verified-models.ts`; a test on each side checks that
//! the two agree. The blocklist (`incompatible_models`) is unrelated and still applies first.

/// A verified model family and the catalog build(s) the gate actually ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedModel {
    /// Catalog alias / name prefix, matched case-insensitively against `<id>`, `<id>-...`, `<id>:...`.
    pub id: &'static str,
    /// Catalog version numbers (the `:N` suffix) of the variants that were tested.
    pub tested_versions: &'static [&'static str],
    /// Where the evidence is recorded; checked against `docs/gpu-validation.md` by a test.
    pub gate_evidence: &'static str,
}

pub const VERIFIED_MODELS: &[VerifiedModel] = &[
    VerifiedModel {
        id: "qwen3.5-4b",
        tested_versions: &["4"],
        gate_evidence: "Chicago questions 7 of 7 on an A10G through the installed app (qwen3.5-4b-cuda-gpu:4). Other builds (Mac CPU/WebGPU) load but have not run the Chicago gate",
    },
    VerifiedModel {
        id: "phi-4-mini-instruct",
        tested_versions: &["5"],
        gate_evidence: "Chicago questions 7 of 7 on a T4 through the installed app (Phi-4-mini-instruct-cuda-gpu:5)",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTrust {
    /// A verified family at a version the gate ran (or with no version in the id).
    Verified,
    /// A verified family at a catalog version the gate did not run.
    VerifiedNewBuild,
    Experimental,
}

impl ModelTrust {
    /// Lower sorts first.
    pub fn rank(self) -> u8 {
        match self {
            ModelTrust::Verified => 0,
            ModelTrust::VerifiedNewBuild => 1,
            ModelTrust::Experimental => 2,
        }
    }
}

fn family_matches(model_lower: &str, family: &str) -> bool {
    match model_lower.strip_prefix(family) {
        Some("") => true,
        Some(rest) => rest.starts_with('-') || rest.starts_with(':'),
        None => false,
    }
}

/// The `:N` version suffix of a catalog id such as `qwen3.5-4b-cuda-gpu:4`.
fn version_of(model_lower: &str) -> Option<&str> {
    model_lower.rsplit_once(':').map(|(_, v)| v).filter(|v| !v.is_empty())
}

pub fn model_trust(model_id: &str) -> ModelTrust {
    let lower = model_id.trim().to_lowercase();
    // Variant ids may start with a publisher or display prefix; try the name as given first.
    match VERIFIED_MODELS.iter().find(|v| family_matches(&lower, v.id)) {
        None => ModelTrust::Experimental,
        Some(v) => match version_of(&lower) {
            Some(ver) if !v.tested_versions.contains(&ver) => ModelTrust::VerifiedNewBuild,
            _ => ModelTrust::Verified,
        },
    }
}

pub fn is_verified(model_id: &str) -> bool {
    model_trust(model_id) != ModelTrust::Experimental
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATE_DOC: &str = include_str!("../../docs/gpu-validation.md");
    const GATE_QUESTIONS: &str = include_str!("../../infra/aws-gpu/chicago-questions.json");
    const FRONTEND_MIRROR: &str = include_str!("../../src/lib/verified-models.ts");

    #[test]
    fn gate_question_list_is_not_empty() {
        let questions: Vec<serde_json::Value> = serde_json::from_str(GATE_QUESTIONS).expect("questions json");
        assert!(questions.len() >= 7, "the release gate asks the 7 Chicago questions");
    }

    /// The question list is model-independent, so a verified model is "gate-covered" when the gate
    /// record names it: the id and every tested version must appear in `docs/gpu-validation.md`.
    #[test]
    fn every_verified_model_is_documented_as_gate_covered() {
        let doc = GATE_DOC.to_lowercase();
        for v in VERIFIED_MODELS {
            assert!(!v.gate_evidence.trim().is_empty(), "{} needs gate evidence", v.id);
            assert!(doc.contains(v.id), "{} is verified but docs/gpu-validation.md never mentions it", v.id);
            for ver in v.tested_versions {
                assert!(
                    doc.contains(&format!(":{ver}")),
                    "{}: tested version :{ver} is not recorded in docs/gpu-validation.md",
                    v.id
                );
            }
        }
    }

    #[test]
    fn default_and_fallback_models_are_verified() {
        assert!(is_verified(super::super::actors::foundry::DEFAULT_MODEL));
        assert!(is_verified(super::super::actors::foundry::DEFAULT_FALLBACK_MODEL));
    }

    #[test]
    fn frontend_mirror_lists_the_same_models_and_versions() {
        for v in VERIFIED_MODELS {
            assert!(FRONTEND_MIRROR.contains(&format!("id: '{}'", v.id)), "src/lib/verified-models.ts is missing {}", v.id);
            for ver in v.tested_versions {
                assert!(FRONTEND_MIRROR.contains(&format!("'{ver}'")), "mirror is missing version {ver} of {}", v.id);
            }
        }
        let mirror_ids = FRONTEND_MIRROR.matches("id: '").count();
        assert_eq!(mirror_ids, VERIFIED_MODELS.len(), "mirror has extra or missing models");
    }

    #[test]
    fn trust_levels() {
        assert_eq!(model_trust("qwen3.5-4b"), ModelTrust::Verified);
        assert_eq!(model_trust("qwen3.5-4b-cuda-gpu:4"), ModelTrust::Verified);
        assert_eq!(model_trust("Phi-4-mini-instruct-cuda-gpu:5"), ModelTrust::Verified);
        assert_eq!(model_trust("phi-4-mini-instruct-cuda-gpu:6"), ModelTrust::VerifiedNewBuild);
        assert_eq!(model_trust("phi-4-mini-reasoning-cuda-gpu:3"), ModelTrust::Experimental);
        assert_eq!(model_trust("gpt-oss-20b-cuda-gpu:1"), ModelTrust::Experimental);
        assert_eq!(model_trust("qwen3.5-4bx"), ModelTrust::Experimental);
        assert!(ModelTrust::Verified.rank() < ModelTrust::VerifiedNewBuild.rank());
        assert!(ModelTrust::VerifiedNewBuild.rank() < ModelTrust::Experimental.rank());
    }
}
