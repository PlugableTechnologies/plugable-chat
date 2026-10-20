//! `plugable-chat --ep-probe <out-dir>`: run the experiments that tell the hypotheses about a
//! declined GPU registration apart, on the machine that shows the problem, and write a verdict for
//! each. Driven by `scripts/support/ep-hypotheses.ps1`, which also gathers the host facts.
//!
//! One process per engine configuration, because the engine is created once per process: a child
//! is started for our configuration, for the Windows App SDK bootstrap, for the SDK's stock
//! configuration and (when given a folder) for another Core build. Everything that decides a
//! verdict is a pure function over the results, so it is tested without an engine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::actors::foundry::backend::ep_ladder::{
    classify_decline, first_duplicate, AttemptRecord, DeclineClass, EngineAnswer, RegistrationEngine,
};

const BOGUS: &str = "NoSuchExecutionProvider";
/// Waits before the retry experiment (H9): is registration declined only because the engine is
/// not ready at start-up?
pub const RETRY_DELAYS_SECS: [u64; 3] = [0, 5, 30];

/// What one engine configuration did.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunReport {
    /// `ours`, `bootstrap`, `stock` or `other-core`.
    pub label: String,
    pub core_dir: Option<String>,
    pub discovered: Vec<(String, bool)>,
    pub attempts: Vec<AttemptRecord>,
    pub error: Option<String>,
}

impl RunReport {
    fn attempt(&self, step: &str) -> Option<&AttemptRecord> {
        self.attempts.iter().find(|a| a.step == step)
    }
    fn registered_anything(&self) -> bool {
        self.attempts.iter().any(|a| a.step != BOGUS_STEP && !a.registered.is_empty())
    }
    fn names(&self) -> Vec<String> {
        self.discovered.iter().map(|(n, _)| n.clone()).collect()
    }
}

const BOGUS_STEP: &str = "control: bogus name";

/// Facts gathered by the PowerShell wrapper; every field is optional because any probe can fail.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostFacts {
    pub os_build: Option<String>,
    /// Files in the app's `foundry-libs` folder.
    #[serde(deserialize_with = "string_or_list")]
    pub foundry_libs_files: Vec<String>,
    /// Distinct onnxruntime.dll files the process could load (path and version).
    #[serde(deserialize_with = "string_or_list")]
    pub onnxruntime_on_disk: Vec<String>,
    /// Windows execution-provider packages (`Get-AppxPackage *.EP.*`) as "name version".
    #[serde(deserialize_with = "string_or_list")]
    pub ep_packages: Vec<String>,
    pub windows_update_blocked: Option<bool>,
    pub pending_reboot: Option<bool>,
    pub foundry_cli_version: Option<String>,
}

/// PowerShell's `ConvertTo-Json` writes a one-item list as a bare string and an empty one as null.
fn string_or_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape {
        One(String),
        Many(Vec<String>),
        Nothing,
    }
    Ok(match Shape::deserialize(d)? {
        Shape::One(s) => vec![s],
        Shape::Many(v) => v,
        Shape::Nothing => Vec::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictState {
    Confirmed,
    Refuted,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub id: String,
    pub name: String,
    pub state: VerdictState,
    pub reason: String,
}

fn verdict(id: &str, name: &str, state: VerdictState, reason: impl Into<String>) -> Verdict {
    Verdict { id: id.into(), name: name.into(), state, reason: reason.into() }
}

// ---------------------------------------------------------------------------------------------
// The experiment matrix (one engine configuration)
// ---------------------------------------------------------------------------------------------

fn dedupe(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    out
}

fn record(step: &str, requested: Option<Vec<String>>, a: &EngineAnswer) -> AttemptRecord {
    AttemptRecord {
        step: step.into(),
        requested,
        success: a.success,
        status: if let Some(e) = &a.error { format!("error: {e}") } else { a.status.clone() },
        registered: a.registered.clone(),
        elapsed_ms: a.elapsed_ms,
    }
}

/// Run the experiments against one engine. The control (a name that does not exist) always runs
/// and has no side effects. The others stop at the first one that registers something, because
/// later asks would only see providers that are already registered. `sleep` is a parameter so the
/// tests do not wait.
pub async fn run_matrix<E, S, F>(engine: &mut E, discovered: &[(String, bool)], delays: &[u64], mut sleep: S) -> Vec<AttemptRecord>
where
    E: RegistrationEngine,
    S: FnMut(Duration) -> F,
    F: std::future::Future<Output = ()>,
{
    let raw: Vec<String> = discovered.iter().filter(|(_, reg)| !reg).map(|(n, _)| n.clone()).collect();
    let unique = dedupe(&raw);
    let mut out = Vec::new();

    let bogus = Some(vec![BOGUS.to_string()]);
    let a = engine.attempt(bogus.clone()).await;
    out.push(record(BOGUS_STEP, bogus, &a));

    let mut steps: Vec<(String, Option<Vec<String>>)> = vec![("each name once".into(), Some(unique.clone()))];
    if raw.len() != unique.len() {
        steps.push(("raw list with repeats".into(), Some(raw.clone())));
    }
    steps.push(("all providers (no names)".into(), None));
    for n in &unique {
        steps.push((format!("only {n}"), Some(vec![n.clone()])));
    }
    if unique.is_empty() {
        return out;
    }
    for (label, names) in steps {
        let a = engine.attempt(names.clone()).await;
        let got = !a.registered.is_empty();
        out.push(record(&label, names, &a));
        if got {
            return out;
        }
    }
    for secs in delays {
        sleep(Duration::from_secs(*secs)).await;
        let names = Some(unique.clone());
        let a = engine.attempt(names.clone()).await;
        let got = !a.registered.is_empty();
        out.push(record(&format!("retry after {secs}s"), names, &a));
        if got {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Verdicts
// ---------------------------------------------------------------------------------------------

const H1: &str = "Core 1.2.3 rejects a request when the engine lists a provider twice";
const H2: &str = "The Windows App SDK is not bootstrapped for an unpackaged app";
const H3: &str = "It is a Core version problem";
const H4: &str = "Windows execution-provider packages are stale or missing";
const H5: &str = "Windows Insider build 26300";
const H6: &str = "Our manager configuration differs from the stock one";
const H7: &str = "A second onnxruntime in the process conflicts with Core's";
const H8: &str = "The installed bundle is incomplete or mismatched";
const H9: &str = "Registration is called before the engine is ready";

/// Files the app's `foundry-libs` folder must hold on Windows for Core with Windows ML.
pub const EXPECTED_LIBS: [&str; 5] = [
    "Microsoft.AI.Foundry.Local.Core.dll",
    "Microsoft.Windows.AI.MachineLearning.dll",
    "onnxruntime.dll",
    "onnxruntime-genai.dll",
    "onnxruntime_providers_shared.dll",
];

pub fn verdicts(runs: &[RunReport], facts: &HostFacts) -> Vec<Verdict> {
    let ours = runs.iter().find(|r| r.label == "ours");
    vec![
        h1(ours),
        h2(ours, runs.iter().find(|r| r.label == "bootstrap")),
        h3(ours, runs.iter().find(|r| r.label == "other-core")),
        h4(ours, facts),
        h5(ours, facts),
        h6(ours, runs.iter().find(|r| r.label == "stock")),
        h7(ours, facts),
        h8(facts),
        h9(ours),
    ]
}

fn no_run(id: &str, name: &str, what: &str) -> Verdict {
    verdict(id, name, VerdictState::Inconclusive, format!("{what} did not run or produced no result"))
}

fn h1(ours: Option<&RunReport>) -> Verdict {
    let Some(r) = ours else { return no_run("H1", H1, "the run with our configuration") };
    if let Some(e) = &r.error {
        return verdict("H1", H1, VerdictState::Inconclusive, format!("the engine could not be created: {e}"));
    }
    let Some(dup) = first_duplicate(&r.names()).cloned() else {
        return verdict("H1", H1, VerdictState::Inconclusive, "the engine lists no provider twice on this host");
    };
    let Some(once) = r.attempt("each name once") else {
        return verdict("H1", H1, VerdictState::Inconclusive, "nothing was left to register");
    };
    if !once.registered.is_empty() {
        return verdict("H1", H1, VerdictState::Refuted, format!("{dup} is listed twice, but asking for each name once registered {}", once.registered.join(", ")));
    }
    if classify_decline(&once.status) != DeclineClass::DuplicateNameSignature {
        return verdict("H1", H1, VerdictState::Refuted, format!("{dup} is listed twice, but the answer is not the empty unknown-name decline: {}", once.status.trim()));
    }
    let alt = r
        .attempts
        .iter()
        .find(|a| a.step != BOGUS_STEP && a.step != "each name once" && !a.step.starts_with("retry") && !a.registered.is_empty());
    match alt {
        Some(a) => verdict("H1", H1, VerdictState::Confirmed, format!("{dup} is listed twice; each name once was declined with an empty unknown-name list, and \"{}\" registered {}", a.step, a.registered.join(", "))),
        None => verdict("H1", H1, VerdictState::Inconclusive, format!("{dup} is listed twice and each name once was declined the same way, but no other way of asking registered anything (see H3, H4)")),
    }
}

fn h2(ours: Option<&RunReport>, boot: Option<&RunReport>) -> Verdict {
    let (Some(a), Some(b)) = (ours, boot) else { return no_run("H2", H2, "the run with and without the bootstrap") };
    if let Some(e) = a.error.as_ref().or(b.error.as_ref()) {
        return verdict("H2", H2, VerdictState::Inconclusive, format!("the engine could not be created: {e}"));
    }
    match (a.registered_anything(), b.registered_anything()) {
        (false, true) => verdict("H2", H2, VerdictState::Confirmed, "nothing registered without the bootstrap; with it, something did"),
        (false, false) => verdict("H2", H2, VerdictState::Refuted, "nothing registered with or without the bootstrap"),
        (true, _) => verdict("H2", H2, VerdictState::Refuted, "registration works without the bootstrap"),
    }
}

fn h3(ours: Option<&RunReport>, other: Option<&RunReport>) -> Verdict {
    let (Some(a), Some(b)) = (ours, other) else {
        return verdict("H3", H3, VerdictState::Inconclusive, "run with --other-core <folder with another Core build> to compare versions");
    };
    if let Some(e) = a.error.as_ref().or(b.error.as_ref()) {
        return verdict("H3", H3, VerdictState::Inconclusive, format!("the engine could not be created: {e}"));
    }
    match (a.registered_anything(), b.registered_anything()) {
        (false, true) => verdict("H3", H3, VerdictState::Confirmed, format!("the other Core ({}) registered what the bundled one refused", b.core_dir.as_deref().unwrap_or("?"))),
        (false, false) => verdict("H3", H3, VerdictState::Refuted, "both Core builds were declined; look at the Windows side (H4, H5)"),
        (true, _) => verdict("H3", H3, VerdictState::Refuted, "the bundled Core registers providers here"),
    }
}

fn h4(ours: Option<&RunReport>, facts: &HostFacts) -> Verdict {
    let Some(r) = ours else { return no_run("H4", H4, "the run with our configuration") };
    if r.registered_anything() {
        return verdict("H4", H4, VerdictState::Refuted, "registration worked, so the installed packages are enough");
    }
    if facts.windows_update_blocked == Some(true) || facts.pending_reboot == Some(true) {
        return verdict("H4", H4, VerdictState::Confirmed, format!("registration failed and Windows Update is paused or a restart is pending (blocked: {:?}, restart pending: {:?})", facts.windows_update_blocked, facts.pending_reboot));
    }
    verdict(
        "H4",
        H4,
        VerdictState::Inconclusive,
        format!("registration failed; installed provider packages: {}", if facts.ep_packages.is_empty() { "none listed".to_string() } else { facts.ep_packages.join("; ") }),
    )
}

fn h5(ours: Option<&RunReport>, facts: &HostFacts) -> Verdict {
    let Some(build) = &facts.os_build else { return no_run("H5", H5, "the Windows build lookup") };
    let insider = build.contains("26300");
    let worked = ours.map(|r| r.registered_anything());
    match (insider, worked) {
        (true, Some(true)) => verdict("H5", H5, VerdictState::Refuted, format!("build {build} registers providers")),
        (true, Some(false)) => verdict("H5", H5, VerdictState::Inconclusive, format!("build {build} fails here; compare with a host on another build (Pat, David and Bob are all on 26300)")),
        (false, Some(false)) => verdict("H5", H5, VerdictState::Refuted, format!("build {build} fails the same way, so it is not specific to 26300")),
        (false, _) => verdict("H5", H5, VerdictState::Inconclusive, format!("build {build}")),
        (true, None) => no_run("H5", H5, "the run with our configuration"),
    }
}

fn h6(ours: Option<&RunReport>, stock: Option<&RunReport>) -> Verdict {
    let (Some(a), Some(b)) = (ours, stock) else { return no_run("H6", H6, "the run with the stock configuration") };
    if let Some(e) = a.error.as_ref().or(b.error.as_ref()) {
        return verdict("H6", H6, VerdictState::Inconclusive, format!("the engine could not be created: {e}"));
    }
    match (a.registered_anything(), b.registered_anything()) {
        (false, true) => verdict("H6", H6, VerdictState::Confirmed, "the stock configuration registers providers and ours does not"),
        (a_ok, b_ok) if a_ok == b_ok => verdict("H6", H6, VerdictState::Refuted, "ours and the stock configuration behave the same"),
        _ => verdict("H6", H6, VerdictState::Inconclusive, "ours registers and the stock configuration does not"),
    }
}

fn h7(ours: Option<&RunReport>, facts: &HostFacts) -> Verdict {
    // The probe does not load the embedding model, so this is about what could be loaded.
    if facts.onnxruntime_on_disk.is_empty() {
        return no_run("H7", H7, "the onnxruntime search");
    }
    let paths: std::collections::BTreeSet<String> = facts
        .onnxruntime_on_disk
        .iter()
        .map(|p| p.rsplit_once(' ').map(|(path, _)| path.to_string()).unwrap_or_else(|| p.to_string()))
        .collect();
    let versions: std::collections::BTreeSet<String> =
        facts.onnxruntime_on_disk.iter().filter_map(|p| p.rsplit_once(' ').map(|(_, v)| v.to_string())).collect();
    let listed = facts.onnxruntime_on_disk.join("; ");
    let two_versions = paths.len() > 1 && versions.len() > 1;
    // Windows ships its own onnxruntime.dll in System32, so two versions on disk are normal and
    // were wrongly "confirmed" on a healthy A10G box. A second runtime can only be the cause when
    // registration actually failed.
    match (two_versions, ours.map(|r| r.registered_anything())) {
        (_, Some(true)) => verdict("H7", H7, VerdictState::Refuted, format!("registration works here, whatever onnxruntime files exist: {listed}")),
        (true, Some(false)) => verdict("H7", H7, VerdictState::Confirmed, format!("registration failed and {} onnxruntime.dll files with different versions could be loaded: {listed}", paths.len())),
        (true, None) => verdict("H7", H7, VerdictState::Inconclusive, format!("two onnxruntime versions are on disk ({listed}), but the registration run gave no result to compare with")),
        (false, _) => verdict("H7", H7, VerdictState::Refuted, format!("one onnxruntime version on disk: {listed}")),
    }
}

fn h8(facts: &HostFacts) -> Verdict {
    if facts.foundry_libs_files.is_empty() {
        return no_run("H8", H8, "the foundry-libs listing");
    }
    let missing: Vec<&str> = EXPECTED_LIBS
        .iter()
        .copied()
        .filter(|f| !facts.foundry_libs_files.iter().any(|have| have.eq_ignore_ascii_case(f)))
        .collect();
    if missing.is_empty() {
        verdict("H8", H8, VerdictState::Refuted, "every expected file is in foundry-libs")
    } else {
        verdict("H8", H8, VerdictState::Confirmed, format!("missing from foundry-libs: {}", missing.join(", ")))
    }
}

fn h9(ours: Option<&RunReport>) -> Verdict {
    let Some(r) = ours else { return no_run("H9", H9, "the run with our configuration") };
    let retries: Vec<&AttemptRecord> = r.attempts.iter().filter(|a| a.step.starts_with("retry")).collect();
    if retries.is_empty() {
        return verdict("H9", H9, VerdictState::Inconclusive, "no retry ran, because an earlier ask registered something or there was nothing to register");
    }
    match retries.iter().find(|a| !a.registered.is_empty()) {
        Some(a) => verdict("H9", H9, VerdictState::Confirmed, format!("\"{}\" registered after earlier asks were declined", a.step)),
        None => verdict("H9", H9, VerdictState::Refuted, format!("{} retries over {}s were all declined", retries.len(), RETRY_DELAYS_SECS.iter().sum::<u64>())),
    }
}

// ---------------------------------------------------------------------------------------------
// Running it
// ---------------------------------------------------------------------------------------------

/// Marker on the single stdout line a child prints, so noise from the engine cannot be mistaken
/// for the result.
const RESULT_PREFIX: &str = "EP_PROBE_RESULT ";

fn default_library_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let core = if cfg!(target_os = "windows") {
        "Microsoft.AI.Foundry.Local.Core.dll"
    } else if cfg!(target_os = "macos") {
        "Microsoft.AI.Foundry.Local.Core.dylib"
    } else {
        "Microsoft.AI.Foundry.Local.Core.so"
    };
    [dir.join("foundry-libs"), dir.to_path_buf()].into_iter().find(|d| d.join(core).exists())
}

/// Child process: one engine configuration, result on stdout.
pub async fn run_case(label: &str, other_core: Option<&Path>) -> RunReport {
    use crate::actors::foundry::backend::sdk;
    let mut report = RunReport { label: label.to_string(), ..Default::default() };
    let library_dir = if label == "other-core" { other_core.map(Path::to_path_buf) } else { default_library_dir() };
    report.core_dir = library_dir.as_ref().map(|d| d.display().to_string());
    let manager = match sdk::probe_manager(library_dir.as_deref(), label == "stock") {
        Ok(m) => m,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    report.discovered = match sdk::probe_discover(manager) {
        Ok(d) => d,
        Err(e) => {
            report.error = Some(format!("discover_eps failed: {e}"));
            return report;
        }
    };
    let mut engine = sdk::probe_engine(manager);
    let discovered = report.discovered.clone();
    report.attempts = run_matrix(&mut engine, &discovered, &RETRY_DELAYS_SECS, |d| tokio::time::sleep(d)).await;
    report
}

pub fn print_case(report: &RunReport) {
    println!("{RESULT_PREFIX}{}", serde_json::to_string(report).unwrap_or_default());
}

/// Parent process: start one child per configuration, collect the results, write the report.
/// Returns the path of the JSON report.
pub fn run_probe(out_dir: &Path, facts_file: Option<&Path>, other_core: Option<&Path>) -> Result<PathBuf, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut labels = vec!["ours", "bootstrap", "stock"];
    if other_core.is_some() {
        labels.push("other-core");
    }
    let mut runs = Vec::new();
    for label in labels {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--ep-probe-case").arg(label);
        if let Some(core) = other_core {
            cmd.arg("--ep-probe-other-core").arg(core);
        }
        if label == "bootstrap" {
            cmd.env("PLUGABLE_FOUNDRY_WINML_BOOTSTRAP", "1");
        }
        let output = crate::process_utils::output_with_timeout(&mut cmd, Duration::from_secs(30 * 60));
        let report = match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.strip_prefix(RESULT_PREFIX))
                .and_then(|json| serde_json::from_str::<RunReport>(json).ok())
                .unwrap_or_else(|| RunReport {
                    label: label.to_string(),
                    error: Some(format!("no result from the child process (exit {:?}): {}", o.status.code(), String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>())),
                    ..Default::default()
                }),
            Err(e) => RunReport { label: label.to_string(), error: Some(format!("child process failed: {e}")), ..Default::default() },
        };
        runs.push(report);
    }
    let facts: HostFacts = facts_file
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim_start_matches('\u{feff}').to_string())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let verdicts = verdicts(&runs, &facts);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = out_dir.join(format!("ep-probe-{stamp}.json"));
    let doc = serde_json::json!({
        "version": crate::version_info::VersionInfo::current().display(),
        "runs": runs,
        "facts": facts,
        "verdicts": verdicts,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    const SIG: &str = "Unknown EP bootstrapper name(s): . Call DiscoverEps() and retry with supported names only. Available EPs: WebGpuExecutionProvider, OpenVINOExecutionProvider, WebGpuExecutionProvider.";
    fn discovered_dup() -> Vec<(String, bool)> {
        vec![("WebGpuExecutionProvider".into(), false), ("OpenVINOExecutionProvider".into(), false), ("WebGpuExecutionProvider".into(), false)]
    }

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
    fn decline() -> EngineAnswer {
        EngineAnswer { success: false, status: SIG.into(), elapsed_ms: 12, ..Default::default() }
    }
    fn ok(r: &[&str]) -> EngineAnswer {
        EngineAnswer { success: true, registered: s(r), elapsed_ms: 300, ..Default::default() }
    }
    async fn matrix(rule: impl FnMut(&Option<Vec<String>>, usize) -> EngineAnswer + Send + 'static, discovered: &[(String, bool)]) -> (Vec<AttemptRecord>, Vec<Option<Vec<String>>>) {
        let mut e = Fake { calls: vec![], rule: Box::new(rule) };
        let out = run_matrix(&mut e, discovered, &[0, 5, 30], |_| async {}).await;
        (out, e.calls)
    }
    fn run(label: &str, discovered: Vec<(String, bool)>, attempts: Vec<AttemptRecord>) -> RunReport {
        RunReport { label: label.into(), discovered, attempts, ..Default::default() }
    }
    fn rec(step: &str, status: &str, registered: &[&str]) -> AttemptRecord {
        AttemptRecord { step: step.into(), requested: None, success: !registered.is_empty(), status: status.into(), registered: s(registered), elapsed_ms: 1 }
    }
    fn state(v: &[Verdict], id: &str) -> VerdictState {
        v.iter().find(|x| x.id == id).unwrap().state
    }

    // ---- the matrix against fake engines ----------------------------------------------------

    #[tokio::test]
    async fn matrix_asks_the_control_first_and_stops_at_the_first_success() {
        let (out, calls) = matrix(
            |names, _| match names {
                None => ok(&["WebGpuExecutionProvider", "OpenVINOExecutionProvider"]),
                _ => decline(),
            },
            &discovered_dup(),
        )
        .await;
        let steps: Vec<&str> = out.iter().map(|a| a.step.as_str()).collect();
        assert_eq!(steps, ["control: bogus name", "each name once", "raw list with repeats", "all providers (no names)"]);
        assert_eq!(calls[0], Some(s(&[BOGUS])));
        assert_eq!(calls.len(), 4);
    }

    #[tokio::test]
    async fn matrix_retries_after_delays_only_when_everything_was_declined() {
        let (out, _) = matrix(|_, n| if n >= 8 { ok(&["WebGpuExecutionProvider"]) } else { decline() }, &discovered_dup()).await;
        let steps: Vec<&str> = out.iter().map(|a| a.step.as_str()).collect();
        assert!(steps.contains(&"retry after 0s"), "{steps:?}");
        assert!(out.iter().any(|a| a.step.starts_with("retry") && !a.registered.is_empty()));
    }

    #[tokio::test]
    async fn matrix_with_nothing_to_register_only_runs_the_control() {
        let (out, _) = matrix(|_, _| ok(&[]), &[("CPUExecutionProvider".into(), true)]).await;
        assert_eq!(out.len(), 1);
    }

    // ---- H1 ------------------------------------------------------------------------------------

    #[test]
    fn h1_confirmed_by_a_duplicate_a_signature_decline_and_a_working_alternative() {
        let r = run("ours", discovered_dup(), vec![rec(BOGUS_STEP, "Unknown EP bootstrapper name(s): NoSuchExecutionProvider.", &[]), rec("each name once", SIG, &[]), rec("all providers (no names)", "", &["OpenVINOExecutionProvider"])]);
        assert_eq!(state(&verdicts(&[r], &HostFacts::default()), "H1"), VerdictState::Confirmed);
    }

    #[test]
    fn h1_refuted_when_each_name_once_registers() {
        let r = run("ours", discovered_dup(), vec![rec("each name once", "", &["WebGpuExecutionProvider"])]);
        assert_eq!(state(&verdicts(&[r], &HostFacts::default()), "H1"), VerdictState::Refuted);
    }

    #[test]
    fn h1_refuted_when_the_decline_is_a_different_message() {
        let r = run("ours", discovered_dup(), vec![rec("each name once", "download failed: 503", &[])]);
        assert_eq!(state(&verdicts(&[r], &HostFacts::default()), "H1"), VerdictState::Refuted);
    }

    #[test]
    fn h1_inconclusive_without_a_duplicate_and_when_no_alternative_works() {
        let single = run("ours", vec![("CUDAExecutionProvider".into(), false)], vec![rec("each name once", SIG, &[])]);
        assert_eq!(state(&verdicts(&[single], &HostFacts::default()), "H1"), VerdictState::Inconclusive);
        let none = run("ours", discovered_dup(), vec![rec("each name once", SIG, &[]), rec("all providers (no names)", SIG, &[])]);
        assert_eq!(state(&verdicts(&[none], &HostFacts::default()), "H1"), VerdictState::Inconclusive);
    }

    #[test]
    fn h1_inconclusive_when_the_engine_could_not_be_created() {
        let r = RunReport { label: "ours".into(), error: Some("SDK create failed".into()), ..Default::default() };
        assert_eq!(state(&verdicts(&[r], &HostFacts::default()), "H1"), VerdictState::Inconclusive);
    }

    // ---- H2 / H3 / H6: two configurations compared -----------------------------------------

    fn working(label: &str) -> RunReport {
        run(label, discovered_dup(), vec![rec("each name once", "", &["OpenVINOExecutionProvider"])])
    }
    fn declined(label: &str) -> RunReport {
        run(label, discovered_dup(), vec![rec("each name once", SIG, &[])])
    }

    #[test]
    fn h2_h3_h6_confirm_when_only_the_other_configuration_registers() {
        for (id, other) in [("H2", "bootstrap"), ("H3", "other-core"), ("H6", "stock")] {
            let v = verdicts(&[declined("ours"), working(other)], &HostFacts::default());
            assert_eq!(state(&v, id), VerdictState::Confirmed, "{id}");
        }
    }

    #[test]
    fn h2_h3_h6_refute_when_both_decline() {
        for (id, other) in [("H2", "bootstrap"), ("H3", "other-core"), ("H6", "stock")] {
            let v = verdicts(&[declined("ours"), declined(other)], &HostFacts::default());
            assert_eq!(state(&v, id), VerdictState::Refuted, "{id}");
        }
    }

    #[test]
    fn h2_h3_refute_when_ours_already_works_and_all_are_inconclusive_without_the_other_run() {
        let v = verdicts(&[working("ours"), working("bootstrap"), working("other-core")], &HostFacts::default());
        assert_eq!(state(&v, "H2"), VerdictState::Refuted);
        assert_eq!(state(&v, "H3"), VerdictState::Refuted);
        let alone = verdicts(&[declined("ours")], &HostFacts::default());
        assert_eq!(state(&alone, "H2"), VerdictState::Inconclusive);
        assert_eq!(state(&alone, "H3"), VerdictState::Inconclusive);
        assert_eq!(state(&alone, "H6"), VerdictState::Inconclusive);
    }

    // ---- H4 / H5 / H7 / H8 / H9: facts and retries -------------------------------------------

    #[test]
    fn h4_confirmed_only_when_windows_update_is_blocked_and_registration_failed() {
        let blocked = HostFacts { windows_update_blocked: Some(true), ..Default::default() };
        assert_eq!(state(&verdicts(&[declined("ours")], &blocked), "H4"), VerdictState::Confirmed);
        assert_eq!(state(&verdicts(&[declined("ours")], &HostFacts::default()), "H4"), VerdictState::Inconclusive);
        assert_eq!(state(&verdicts(&[working("ours")], &blocked), "H4"), VerdictState::Refuted);
    }

    #[test]
    fn h4_lists_the_installed_packages_in_the_reason() {
        let facts = HostFacts { ep_packages: s(&["MicrosoftCorporationII.WinML.Intel.OpenVINO.EP.1.8 1.8.80.0"]), ..Default::default() };
        let v = verdicts(&[declined("ours")], &facts);
        assert!(v.iter().find(|x| x.id == "H4").unwrap().reason.contains("OpenVINO.EP.1.8"));
    }

    #[test]
    fn h5_refuted_by_a_failure_on_another_build_and_never_confirmed_by_one_host() {
        let other = HostFacts { os_build: Some("26100".into()), ..Default::default() };
        assert_eq!(state(&verdicts(&[declined("ours")], &other), "H5"), VerdictState::Refuted);
        let insider = HostFacts { os_build: Some("Microsoft Windows 11 Home build 26300".into()), ..Default::default() };
        assert_eq!(state(&verdicts(&[declined("ours")], &insider), "H5"), VerdictState::Inconclusive);
        assert_eq!(state(&verdicts(&[working("ours")], &insider), "H5"), VerdictState::Refuted);
    }

    fn two_runtimes() -> HostFacts {
        HostFacts { onnxruntime_on_disk: s(&["C:\\app\\foundry-libs\\onnxruntime.dll 1.26.0", "C:\\Windows\\System32\\onnxruntime.dll 1.17.1"]), ..Default::default() }
    }

    #[test]
    fn h7_confirmed_only_when_registration_failed_with_two_onnxruntime_versions() {
        assert_eq!(state(&verdicts(&[declined("ours")], &two_runtimes()), "H7"), VerdictState::Confirmed);
    }

    #[test]
    fn h7_refuted_when_registration_works_despite_two_onnxruntime_versions() {
        // The A10G box (2026-10-20): Windows' System32 copy is 1.17, ours 1.26, and CUDA registers.
        let v = verdicts(&[working("ours")], &two_runtimes());
        assert_eq!(state(&v, "H7"), VerdictState::Refuted);
        assert!(v.iter().find(|x| x.id == "H7").unwrap().reason.contains("registration works"));
    }

    #[test]
    fn h7_is_not_confirmed_without_a_registration_result_and_handles_missing_facts() {
        assert_eq!(state(&verdicts(&[], &two_runtimes()), "H7"), VerdictState::Inconclusive);
        let one = HostFacts { onnxruntime_on_disk: s(&["C:\\app\\foundry-libs\\onnxruntime.dll 1.26.0"]), ..Default::default() };
        assert_eq!(state(&verdicts(&[declined("ours")], &one), "H7"), VerdictState::Refuted);
        assert_eq!(state(&verdicts(&[], &HostFacts::default()), "H7"), VerdictState::Inconclusive);
    }

    #[test]
    fn h8_confirmed_by_a_missing_file_and_names_it() {
        // The rc13 support details listed only these three files in foundry-libs.
        let three = HostFacts { foundry_libs_files: s(&["onnxruntime-genai.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll"]), ..Default::default() };
        let v = verdicts(&[], &three);
        assert_eq!(state(&v, "H8"), VerdictState::Confirmed);
        let reason = &v.iter().find(|x| x.id == "H8").unwrap().reason;
        assert!(reason.contains("Microsoft.AI.Foundry.Local.Core.dll") && reason.contains("Microsoft.Windows.AI.MachineLearning.dll"), "{reason}");
        let all = HostFacts { foundry_libs_files: EXPECTED_LIBS.iter().map(|f| f.to_string()).collect(), ..Default::default() };
        assert_eq!(state(&verdicts(&[], &all), "H8"), VerdictState::Refuted);
    }

    #[test]
    fn h9_confirmed_by_a_late_success_and_refuted_by_all_declines() {
        let late = run("ours", discovered_dup(), vec![rec("each name once", SIG, &[]), rec("retry after 0s", SIG, &[]), rec("retry after 5s", "", &["WebGpuExecutionProvider"])]);
        assert_eq!(state(&verdicts(&[late], &HostFacts::default()), "H9"), VerdictState::Confirmed);
        let never = run("ours", discovered_dup(), vec![rec("retry after 0s", SIG, &[]), rec("retry after 5s", SIG, &[]), rec("retry after 30s", SIG, &[])]);
        assert_eq!(state(&verdicts(&[never], &HostFacts::default()), "H9"), VerdictState::Refuted);
        assert_eq!(state(&verdicts(&[working("ours")], &HostFacts::default()), "H9"), VerdictState::Inconclusive);
    }

    // ---- the report round-trips -------------------------------------------------------------

    #[test]
    fn run_report_survives_the_json_a_child_prints() {
        let r = run("ours", discovered_dup(), vec![rec("each name once", SIG, &[])]);
        let line = format!("{RESULT_PREFIX}{}", serde_json::to_string(&r).unwrap());
        let back: RunReport = serde_json::from_str(line.strip_prefix(RESULT_PREFIX).unwrap()).unwrap();
        assert_eq!(back.attempts, r.attempts);
        assert_eq!(back.discovered, r.discovered);
    }

    #[test]
    fn host_facts_accept_powershells_bare_string_and_null_for_lists() {
        let facts: HostFacts = serde_json::from_str("{\"ep_packages\":\"A 1.0\",\"foundry_libs_files\":null,\"onnxruntime_on_disk\":[\"x 1\",\"y 2\"]}").unwrap();
        assert_eq!(facts.ep_packages, vec!["A 1.0".to_string()]);
        assert!(facts.foundry_libs_files.is_empty());
        assert_eq!(facts.onnxruntime_on_disk.len(), 2);
    }

    #[test]
    fn host_facts_parse_the_powershell_json_with_a_byte_order_mark() {
        let text = "\u{feff}{\"os_build\":\"26300\",\"ep_packages\":[\"A 1.0\"],\"windows_update_blocked\":false}";
        let facts: HostFacts = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        assert_eq!(facts.os_build.as_deref(), Some("26300"));
        assert_eq!(facts.windows_update_blocked, Some(false));
        assert!(facts.foundry_libs_files.is_empty());
    }
}
