//! What to do when the engine declines to register execution providers.
//!
//! Foundry Local Core 1.2.3 answers `download_and_register_eps` in about 10 ms with
//! `Unknown EP bootstrapper name(s): .` (an empty list) when its own provider list repeats a name
//! (`WebGpuExecutionProvider` twice). The same text is in Microsoft's tracker
//! (`microsoft/Foundry-Local` issue 792), where it is fixed in Core 1.2.4. We ship a newer Core;
//! this ladder is the safety net for an engine that still answers that way, and its record of
//! which step worked tells support which hypothesis was right.
//!
//! The ladder is generic over [`RegistrationEngine`] so the tests can stand in a fake engine for
//! each hypothesis about why registration is declined.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::gpu_diagnostics::{make_failure, EpFailure, EpFailureKind};

/// The text the engine puts in front of the names it did not recognise.
const UNKNOWN_BOOTSTRAPPER_MARKER: &str = "Unknown EP bootstrapper name(s):";

/// One answer from the engine to one `download_and_register_eps` call.
#[derive(Debug, Clone, Default)]
pub struct EngineAnswer {
    pub success: bool,
    pub status: String,
    pub registered: Vec<String>,
    pub failed: Vec<String>,
    /// The call itself errored (as opposed to the engine answering `success: false`).
    pub error: Option<String>,
    pub elapsed_ms: u128,
}

/// The engine as the ladder sees it: ask for these names, or for everything with `None`.
#[async_trait]
pub trait RegistrationEngine: Send {
    async fn attempt(&mut self, names: Option<Vec<String>>) -> EngineAnswer;
}

/// One call the ladder made, kept for the support details.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AttemptRecord {
    pub step: String,
    /// `None` means "everything the engine can discover".
    pub requested: Option<Vec<String>>,
    pub success: bool,
    pub status: String,
    pub registered: Vec<String>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct LadderOutcome {
    pub attempts: Vec<AttemptRecord>,
    pub registered: Vec<String>,
    pub failed: Vec<String>,
    pub failures: Vec<EpFailure>,
    pub cancelled: bool,
}

impl LadderOutcome {
    /// The step that registered something, if any.
    pub fn working_step(&self) -> Option<&str> {
        self.attempts.iter().find(|a| !a.registered.is_empty()).map(|a| a.step.as_str())
    }
}

/// Names from the engine's "Available EPs: A, B, A." text, in order, repeats kept.
pub fn available_eps_in_status(status: &str) -> Vec<String> {
    let Some(i) = status.find("Available EPs:") else { return Vec::new() };
    // Provider names hold no '.', so the list ends at the first one, whatever the surrounding log
    // line adds after it (`", registered=[]`, a closing parenthesis).
    status[i + "Available EPs:".len()..]
        .split('.')
        .next()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A name that appears more than once in `names`.
pub fn first_duplicate(names: &[String]) -> Option<&String> {
    names.iter().enumerate().find(|(i, n)| names[..*i].contains(n)).map(|(_, n)| n)
}

/// The engine said "unknown name(s)" and then named none: an answer about the engine's own list,
/// not about anything we asked for.
pub fn is_empty_unknown_decline(status: &str) -> bool {
    let Some(i) = status.find(UNKNOWN_BOOTSTRAPPER_MARKER) else { return false };
    let after = status[i + UNKNOWN_BOOTSTRAPPER_MARKER.len()..].trim_start();
    after.starts_with('.') || after.is_empty()
}

/// What the support details should say about a declined registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclineClass {
    /// Empty unknown-name list and a repeated name in the engine's own list: the Core 1.2.3 bug
    /// (fixed in 1.2.4, `microsoft/Foundry-Local` issue 792).
    DuplicateNameSignature,
    /// Empty unknown-name list without a repeated name: same message, different cause.
    EmptyUnknownNoDuplicate,
    /// Some other decline, or no decline.
    Other,
}

/// Classify the engine's status text. The Windows hosts that failed on 2026-10-19 (Intel Arc B60,
/// RTX 4080) are the real inputs this is tested against.
pub fn classify_decline(status: &str) -> DeclineClass {
    if !is_empty_unknown_decline(status) {
        return DeclineClass::Other;
    }
    if first_duplicate(&available_eps_in_status(status)).is_some() {
        DeclineClass::DuplicateNameSignature
    } else {
        DeclineClass::EmptyUnknownNoDuplicate
    }
}

fn dedupe(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    out
}

fn declined(answer: &EngineAnswer) -> bool {
    answer.error.is_none() && answer.registered.is_empty() && is_empty_unknown_decline(&answer.status)
}

/// Ask for the providers; when the engine declines with the empty-unknown answer, try again with
/// the raw list (repeats kept), then with no names at all, then one name at a time. Any other
/// answer, success or failure, ends the ladder: it is the engine's real answer, and asking again
/// the same way would only repeat it.
///
/// `raw_missing` is the engine's own list of unregistered providers in its order, repeats kept.
/// Bootstrapping the Windows App SDK is not a step here: the engine is created once per process,
/// so that hypothesis is tested by `--ep-probe` in a separate process.
pub async fn run_ladder<E: RegistrationEngine>(
    engine: &mut E,
    raw_missing: &[String],
    cancel: &AtomicBool,
) -> LadderOutcome {
    let unique = dedupe(raw_missing);
    let mut outcome = LadderOutcome::default();

    let mut alternatives: Vec<(String, Option<Vec<String>>)> = Vec::new();
    if raw_missing.len() != unique.len() {
        alternatives.push(("raw list with repeats".into(), Some(raw_missing.to_vec())));
    }
    alternatives.push(("all providers (no names)".into(), None));

    let first = ("each name once".to_string(), Some(unique.clone()));
    let mut declined_so_far = true;
    for (i, (label, names)) in std::iter::once(first).chain(alternatives).enumerate() {
        if cancel.load(Ordering::SeqCst) {
            outcome.cancelled = true;
            finish(&mut outcome, &unique);
            return outcome;
        }
        let answer = engine.attempt(names.clone()).await;
        outcome.attempts.push(record(&label, names, &answer));
        if !declined(&answer) {
            // The engine's real answer (success, a download failure, an errored call): report it
            // the way a single call always was, and stop.
            settle(&mut outcome, &unique, &answer, cancel.load(Ordering::SeqCst), i == 0);
            return outcome;
        }
        declined_so_far = true;
    }
    debug_assert!(declined_so_far);

    // Every whole-list ask was declined: one name at a time shows which provider the engine
    // will take.
    for name in &unique {
        if cancel.load(Ordering::SeqCst) {
            outcome.cancelled = true;
            break;
        }
        let names = Some(vec![name.clone()]);
        let answer = engine.attempt(names.clone()).await;
        outcome.attempts.push(record(&format!("only {name}"), names, &answer));
        for ep in &answer.registered {
            if !outcome.registered.contains(ep) {
                outcome.registered.push(ep.clone());
            }
        }
    }
    finish(&mut outcome, &unique);
    outcome
}

fn record(step: &str, requested: Option<Vec<String>>, answer: &EngineAnswer) -> AttemptRecord {
    AttemptRecord {
        step: step.to_string(),
        requested,
        success: answer.success,
        status: answer.status.clone(),
        registered: answer.registered.clone(),
        elapsed_ms: answer.elapsed_ms,
    }
}

/// Turn a non-decline answer into the outcome, the way the old single call did.
fn settle(outcome: &mut LadderOutcome, unique: &[String], answer: &EngineAnswer, cancelled: bool, first_step: bool) {
    outcome.cancelled = cancelled;
    if let Some(error) = &answer.error {
        outcome.failures = unique.iter().map(|ep| ep_failure(ep, error, cancelled)).collect();
        outcome.failed = unique.to_vec();
        return;
    }
    let (registered, failed, failures) = super::sdk::interpret_registration(
        unique,
        answer.success,
        &answer.status,
        answer.registered.clone(),
        answer.failed.clone(),
        cancelled,
        answer.elapsed_ms,
    );
    outcome.registered = registered;
    outcome.failed = failed;
    outcome.failures = failures;
    // Later steps asked for a different list than `unique`, so providers they did not return are
    // not failures of this answer.
    let _ = first_step;
}

fn ep_failure(ep: &str, message: &str, cancelled: bool) -> EpFailure {
    let mut failure = make_failure(ep, message);
    if cancelled {
        failure.kind = EpFailureKind::Cancelled;
    }
    failure
}

/// Account for every provider we asked about after the whole ladder, so a decline never reads as
/// "nothing to report".
fn finish(outcome: &mut LadderOutcome, unique: &[String]) {
    let tried = outcome
        .attempts
        .iter()
        .map(|a| format!("{} ({} ms): {}", a.step, a.elapsed_ms, if a.registered.is_empty() { "declined" } else { "registered" }))
        .collect::<Vec<_>>()
        .join("; ");
    // The last answer that declined, which is the one worth quoting even when a later single-name
    // step succeeded for another provider.
    let last_status = outcome
        .attempts
        .iter()
        .rev()
        .find(|a| a.registered.is_empty() && !a.status.trim().is_empty())
        .map(|a| a.status.clone())
        .unwrap_or_default();
    let known = match classify_decline(&last_status) {
        DeclineClass::DuplicateNameSignature => {
            " This is the known Foundry Local 1.2.3 bug (a provider listed twice), fixed in Core 1.2.4."
        }
        _ => "",
    };
    for ep in unique {
        if outcome.registered.contains(ep) || outcome.failed.contains(ep) {
            continue;
        }
        let (kind, message) = if outcome.cancelled {
            (EpFailureKind::Cancelled, "cancelled".to_string())
        } else {
            (
                EpFailureKind::EngineDeclined,
                format!(
                    "the engine answered without registering it. Tried: {tried}. Last answer: {}.{known}",
                    last_status.trim()
                ),
            )
        };
        outcome.failures.push(EpFailure { ep: ep.clone(), kind, message });
        outcome.failed.push(ep.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The text the engine returned on both failing hosts (Windows, 2026-10-19).
    const B60_STATUS: &str = "Unknown EP bootstrapper name(s): . Call DiscoverEps() and retry with supported names only. Available EPs: WebGpuExecutionProvider, OpenVINOExecutionProvider, WebGpuExecutionProvider.";
    const RTX_STATUS: &str = "Unknown EP bootstrapper name(s): . Call DiscoverEps() and retry with supported names only. Available EPs: CUDAExecutionProvider, WebGpuExecutionProvider, OpenVINOExecutionProvider, NvTensorRTRTXExecutionProvider, WebGpuExecutionProvider.";

    fn decline(status: &str) -> EngineAnswer {
        EngineAnswer { success: false, status: status.into(), elapsed_ms: 12, ..Default::default() }
    }
    fn ok(registered: &[&str]) -> EngineAnswer {
        EngineAnswer { success: true, status: String::new(), registered: s(registered), elapsed_ms: 400, ..Default::default() }
    }

    /// A fake engine whose behaviour models one hypothesis. `calls` records what was asked.
    struct Fake {
        calls: Vec<Option<Vec<String>>>,
        rule: Box<dyn FnMut(&Option<Vec<String>>, usize) -> EngineAnswer + Send>,
    }
    #[async_trait]
    impl RegistrationEngine for Fake {
        async fn attempt(&mut self, names: Option<Vec<String>>) -> EngineAnswer {
            self.calls.push(names.clone());
            let n = self.calls.len();
            (self.rule)(&names, n)
        }
    }
    fn fake(rule: impl FnMut(&Option<Vec<String>>, usize) -> EngineAnswer + Send + 'static) -> Fake {
        Fake { calls: Vec::new(), rule: Box::new(rule) }
    }

    const RAW: [&str; 3] = ["WebGpuExecutionProvider", "OpenVINOExecutionProvider", "WebGpuExecutionProvider"];

    // ---- H1: Core 1.2.3 rejects any multi-name request when its list repeats a name ----------

    #[tokio::test]
    async fn h1_duplicate_name_engine_recovers_through_all_providers() {
        let mut e = fake(|names, _| match names {
            Some(v) if v.len() > 1 => decline(B60_STATUS),
            Some(v) if v.len() == 1 && v[0] == "WebGpuExecutionProvider" => decline(B60_STATUS),
            None => ok(&["WebGpuExecutionProvider", "OpenVINOExecutionProvider"]),
            _ => decline(B60_STATUS),
        });
        let out = run_ladder(&mut e, &s(&RAW), &AtomicBool::new(false)).await;
        assert_eq!(out.registered, s(&["WebGpuExecutionProvider", "OpenVINOExecutionProvider"]));
        assert!(out.failed.is_empty());
        assert_eq!(out.working_step(), Some("all providers (no names)"));
        // each name once, then the raw list, then everything; it stops at the first success
        assert_eq!(e.calls.len(), 3);
        assert_eq!(e.calls[0], Some(s(&["WebGpuExecutionProvider", "OpenVINOExecutionProvider"])));
        assert_eq!(e.calls[1], Some(s(&RAW)));
        assert_eq!(e.calls[2], None);
    }

    #[tokio::test]
    async fn h1_duplicate_name_engine_recovers_through_single_names_when_all_providers_also_declines() {
        let mut e = fake(|names, _| match names {
            Some(v) if v.len() == 1 && v[0] == "OpenVINOExecutionProvider" => ok(&["OpenVINOExecutionProvider"]),
            _ => decline(B60_STATUS),
        });
        let out = run_ladder(&mut e, &s(&RAW), &AtomicBool::new(false)).await;
        assert_eq!(out.registered, s(&["OpenVINOExecutionProvider"]));
        assert_eq!(out.failed, s(&["WebGpuExecutionProvider"]));
        assert_eq!(out.failures.len(), 1);
        assert_eq!(out.failures[0].kind, EpFailureKind::EngineDeclined);
        assert!(out.failures[0].message.contains("known Foundry Local 1.2.3 bug"));
    }

    // ---- H3: an old Core declines everything in-process, whatever we send ---------------------

    #[tokio::test]
    async fn h3_old_core_declines_every_step_and_reports_each() {
        let mut e = fake(|_, _| decline(B60_STATUS));
        let out = run_ladder(&mut e, &s(&RAW), &AtomicBool::new(false)).await;
        assert!(out.registered.is_empty());
        assert_eq!(out.failed, s(&["WebGpuExecutionProvider", "OpenVINOExecutionProvider"]));
        // deduped, raw, all, one name x2
        assert_eq!(e.calls.len(), 5);
        assert!(out.failures.iter().all(|f| f.kind == EpFailureKind::EngineDeclined));
        assert!(out.failures[0].message.contains("each name once"));
        assert!(out.failures[0].message.contains("all providers (no names)"));
    }

    // ---- H9: engine not ready yet; a later step succeeds ---------------------------------------

    #[tokio::test]
    async fn h9_not_ready_engine_succeeds_on_a_later_step() {
        let mut e = fake(|_, n| if n < 2 { decline(B60_STATUS) } else { ok(&["WebGpuExecutionProvider", "OpenVINOExecutionProvider"]) });
        let out = run_ladder(&mut e, &s(&RAW), &AtomicBool::new(false)).await;
        assert_eq!(out.working_step(), Some("raw list with repeats"));
        assert_eq!(e.calls.len(), 2);
        assert!(out.failed.is_empty());
    }

    // ---- control: one adapter, no repeat, first call works -------------------------------------

    #[tokio::test]
    async fn control_single_adapter_engine_makes_exactly_one_call() {
        let mut e = fake(|_, _| ok(&["CUDAExecutionProvider"]));
        let out = run_ladder(&mut e, &s(&["CUDAExecutionProvider", "WebGpuExecutionProvider"]), &AtomicBool::new(false)).await;
        assert_eq!(e.calls.len(), 1);
        assert_eq!(out.registered, s(&["CUDAExecutionProvider"]));
        assert!(out.failed.is_empty(), "a success keeps the old behaviour: providers not returned are not failures");
    }

    // ---- an answer that is not the empty-unknown decline is the engine's real answer ----------

    #[tokio::test]
    async fn another_failure_is_not_retried() {
        let mut e = fake(|_, _| EngineAnswer {
            success: false,
            status: "download failed: 503".into(),
            failed: s(&["CUDAExecutionProvider"]),
            elapsed_ms: 3000,
            ..Default::default()
        });
        let out = run_ladder(&mut e, &s(&["CUDAExecutionProvider"]), &AtomicBool::new(false)).await;
        assert_eq!(e.calls.len(), 1);
        assert_eq!(out.failed, s(&["CUDAExecutionProvider"]));
        assert_eq!(out.failures[0].kind, EpFailureKind::Network);
    }

    #[tokio::test]
    async fn an_errored_call_is_recorded_against_every_provider_and_not_retried() {
        let mut e = fake(|_, _| EngineAnswer { error: Some("engine crashed".into()), ..Default::default() });
        let out = run_ladder(&mut e, &s(&["A", "B"]), &AtomicBool::new(false)).await;
        assert_eq!(e.calls.len(), 1);
        assert_eq!(out.failed, s(&["A", "B"]));
    }

    // ---- cancel stops the ladder ---------------------------------------------------------------

    #[tokio::test]
    async fn cancel_before_the_first_call_makes_none() {
        let mut e = fake(|_, _| ok(&["A"]));
        let out = run_ladder(&mut e, &s(&["A"]), &AtomicBool::new(true)).await;
        assert!(e.calls.is_empty());
        assert!(out.cancelled);
        assert_eq!(out.failures[0].kind, EpFailureKind::Cancelled);
    }

    #[tokio::test]
    async fn cancel_between_steps_stops_the_ladder() {
        use std::sync::Arc;
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let mut e = fake(move |_, n| {
            if n == 1 {
                flag.store(true, Ordering::SeqCst);
            }
            decline(B60_STATUS)
        });
        let out = run_ladder(&mut e, &s(&RAW), &cancel).await;
        assert_eq!(e.calls.len(), 1);
        assert!(out.cancelled);
    }

    #[tokio::test]
    async fn without_a_repeat_the_raw_step_is_skipped() {
        let mut e = fake(|names, _| if names.is_none() { ok(&["A"]) } else { decline(B60_STATUS) });
        let out = run_ladder(&mut e, &s(&["A", "B"]), &AtomicBool::new(false)).await;
        assert_eq!(e.calls.len(), 2, "each name once, then all providers; no raw step when nothing repeats");
        assert_eq!(out.working_step(), Some("all providers (no names)"));
    }

    // ---- classifier on the real status texts --------------------------------------------------

    #[test]
    fn real_failing_hosts_have_the_duplicate_name_signature() {
        assert_eq!(classify_decline(B60_STATUS), DeclineClass::DuplicateNameSignature);
        assert_eq!(classify_decline(RTX_STATUS), DeclineClass::DuplicateNameSignature);
    }

    #[test]
    fn upstream_snapdragon_report_has_the_same_signature() {
        let s = "Unknown EP bootstrapper name(s): . Available EPs: WebGpuExecutionProvider, WebGpuExecutionProvider, QNNExecutionProvider.";
        assert_eq!(classify_decline(s), DeclineClass::DuplicateNameSignature);
    }

    #[test]
    fn empty_unknown_list_without_a_repeat_is_a_different_cause() {
        let s = "Unknown EP bootstrapper name(s): . Available EPs: CUDAExecutionProvider, WebGpuExecutionProvider.";
        assert_eq!(classify_decline(s), DeclineClass::EmptyUnknownNoDuplicate);
    }

    #[test]
    fn a_normal_unknown_name_error_names_the_name_and_is_not_the_signature() {
        // The control: when the engine does validate names it says which one it did not know.
        let s = "Unknown EP bootstrapper name(s): NoSuchExecutionProvider. Available EPs: WebGpuExecutionProvider.";
        assert!(!is_empty_unknown_decline(s));
        assert_eq!(classify_decline(s), DeclineClass::Other);
    }

    #[test]
    fn success_and_other_text_are_not_declines() {
        assert_eq!(classify_decline(""), DeclineClass::Other);
        assert_eq!(classify_decline("ok"), DeclineClass::Other);
    }

    #[test]
    fn available_list_parsing_keeps_order_and_repeats() {
        assert_eq!(available_eps_in_status(B60_STATUS), s(&RAW));
        assert_eq!(first_duplicate(&s(&RAW)), Some(&"WebGpuExecutionProvider".to_string()));
        assert_eq!(first_duplicate(&s(&["A", "B"])), None);
    }

    // ---- the four real support files from 2026-10-19 ---------------------------------------------

    const FIXTURES: [(&str, &str); 4] = [
        ("Intel Arc B60 (David), attempt 1", include_str!("fixtures/ep-declined/david-arc-b60-rc13-1.txt")),
        ("Intel Arc B60 (David), attempt 2", include_str!("fixtures/ep-declined/david-arc-b60-rc13-2.txt")),
        ("RTX 4080 (Bob), attempt 1", include_str!("fixtures/ep-declined/bob-rtx4080-rc13-1.txt")),
        ("RTX 4080 (Bob), attempt 2", include_str!("fixtures/ep-declined/bob-rtx4080-rc13-2.txt")),
    ];

    /// Every engine status in a support file: the text after "status: " or `status="` up to the
    /// end of the line's sentence, the way both the app log and the card print it.
    fn statuses_in(file: &str) -> Vec<String> {
        file.lines()
            .filter_map(|line| {
                let i = line.find("Unknown EP bootstrapper name(s):")?;
                Some(line[i..].trim_end_matches(['"', ')', ',']).trim_end_matches(")").to_string())
            })
            .collect()
    }

    #[test]
    fn every_engine_status_in_the_real_support_files_has_the_duplicate_name_signature() {
        for (host, file) in FIXTURES {
            let statuses = statuses_in(file);
            assert!(!statuses.is_empty(), "{host}: no engine status found in the file");
            for status in statuses {
                assert_eq!(
                    classify_decline(&status),
                    DeclineClass::DuplicateNameSignature,
                    "{host}: {status}"
                );
            }
        }
    }

    #[test]
    fn real_files_show_the_engine_listing_webgpu_twice_and_no_unknown_name() {
        for (host, file) in FIXTURES {
            let status = statuses_in(file).into_iter().next().unwrap();
            let available = available_eps_in_status(&status);
            assert_eq!(first_duplicate(&available), Some(&"WebGpuExecutionProvider".to_string()), "{host}");
            assert!(is_empty_unknown_decline(&status), "{host}: the unknown-name list is empty");
        }
    }

    #[test]
    fn the_a10g_style_success_log_is_not_classified_as_a_decline() {
        let success = "2026-10-17 [gpu] engine answered after 61000 ms: success=true, status=\"\", registered=[CUDAExecutionProvider], failed=[]";
        assert!(statuses_in(success).is_empty());
        assert_eq!(classify_decline(""), DeclineClass::Other);
    }
}
