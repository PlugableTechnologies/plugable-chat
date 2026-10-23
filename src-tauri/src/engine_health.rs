//! Keeps the GPU engine's downloaded parts (the execution-provider dlls Foundry Local Core puts in
//! `~/.plugable-chat/ep`) from trapping the app in a crash loop, without the user reinstalling.
//!
//! rc15 field report: a network drop mid-session crashed the app inside
//! `onnxruntime_providers_webgpu.dll`; afterwards every prompt crashed again, because the damaged
//! dll survived the reinstall, and the per-model crash blocklist then switched off every model
//! ("No compatible model available"). This module holds the pure decisions and the small file
//! operations; `lib.rs` applies them at launch, before the engine loads anything.
//!
//! The ladder, in order:
//! 1. A *verified marker* (file sizes + a head/tail checksum of each library) is written only
//!    after a chat turn finished on the current files. Files that no longer match it are
//!    quarantined (renamed, never deleted) so Core downloads them again.
//! 2. A death attributed to a provider dll, or any death on files that were never verified,
//!    quarantines the provider; Core re-downloads it (offline: the app starts on CPU models).
//! 3. A second unclean death in a row, with no good session between, turns GPU acceleration off
//!    until the user presses "Try GPU again" (or Repair).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::run_state::LastRun;

const HEALTH_FILE: &str = "engine-health.json";
const MARKER_FILE: &str = ".plugable-verified.json";
/// Unclean deaths in a row (no good session between) before GPU acceleration is switched off.
pub const DEATHS_BEFORE_GPU_OFF: u32 = 2;
/// Only library-like files are fingerprinted: Core may keep bookkeeping files here that change.
const FINGERPRINTED: [&str; 5] = ["dll", "so", "dylib", "onnx", "bin"];
const SAMPLE: u64 = 64 * 1024;

pub const TEST_FAULT_ENV: &str = "PLUGABLE_CHAT_TEST_FAULT";
pub const EP_DIR_ENV: &str = "PLUGABLE_CHAT_EP_DIR";

// ---------- test-only fault hooks (off unless the variable is set) ----------

/// Faults the harness can inject through `PLUGABLE_CHAT_TEST_FAULT=<a>,<b>`:
/// `offline` (the connectivity probe says no network), `ep-fault` (the previous death is
/// attributed to a provider dll), `registration-hang` (provider registration never returns),
/// `empty-catalog` (the model list comes back empty).
pub fn test_fault(name: &str) -> bool {
    std::env::var(TEST_FAULT_ENV).map(|v| v.split(',').any(|p| p.trim() == name)).unwrap_or(false)
}

// ---------- persisted state ----------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineHealth {
    /// Unclean deaths since the last good session.
    pub unclean_deaths: u32,
    /// GPU acceleration is off (the engine registers no GPU providers; only CPU models are offered).
    pub gpu_off: bool,
    pub gpu_off_reason: Option<String>,
    /// Application Error reports already handled, so one crash is never counted twice.
    pub seen_report_ids: Vec<String>,
    pub last_quarantine: Option<String>,
}

pub fn health_in(dir: &Path) -> EngineHealth {
    std::fs::read_to_string(dir.join(HEALTH_FILE)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_health_in(dir: &Path, health: &EngineHealth) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_string(health) {
        let tmp = dir.join(format!("{HEALTH_FILE}.tmp"));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(HEALTH_FILE));
        }
    }
}

static GPU_OFF: AtomicBool = AtomicBool::new(false);

/// True while GPU acceleration is switched off for this run.
pub fn gpu_off() -> bool {
    GPU_OFF.load(Ordering::SeqCst)
}

pub fn set_gpu_off(v: bool) {
    GPU_OFF.store(v, Ordering::SeqCst);
}

// ---------- where the files live ----------

/// Foundry Local Core's provider folder. Core defaults its app-data dir to `~/.plugable-chat`
/// (the rc15 field report shows `C:\Users\<u>\.plugable-chat\ep\webgpu-ep`).
pub fn ep_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(EP_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(".plugable-chat").join("ep"))
}

// ---------- crash attribution ----------

/// A native fault read from the Windows Application log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFault {
    pub code: String,
    pub module_path: String,
    pub report_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultSite {
    /// A downloaded execution-provider dll; the named provider folder is the one to quarantine.
    Ep { provider: Option<String> },
    /// The app itself or the engine it ships.
    Other,
}

pub fn attribute(fault: &NativeFault) -> FaultSite {
    let norm = fault.module_path.replace('\\', "/").to_lowercase();
    let parts: Vec<&str> = norm.split('/').collect();
    match parts.iter().position(|p| *p == "ep") {
        Some(i) if i > 0 && parts[i - 1] == ".plugable-chat" => {
            // keep the original case of the provider folder name
            let orig: Vec<&str> = fault.module_path.split(|c| c == '\\' || c == '/').collect();
            FaultSite::Ep { provider: orig.get(i + 1).filter(|_| parts.len() > i + 2).map(|s| s.to_string()) }
        }
        _ => FaultSite::Other,
    }
}

/// Pulls plugable-chat crash reports out of `wevtutil qe Application /f:text` output, newest first.
/// (A fast-fail crash, 0xc0000409, skips every in-process exception filter, so the Application
/// log is the one place that names the module.)
pub fn parse_application_errors(text: &str) -> Vec<NativeFault> {
    let mut out = Vec::new();
    for block in text.split("Event[") {
        let get = |key: &str| -> Option<String> {
            block.lines().find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().trim_end_matches(',').to_string()))
        };
        let Some(app) = get("Faulting application name:") else { continue };
        // "Faulting application name: plugable-chat.exe, version: ..." keeps the first field only
        let app = app.split(',').next().unwrap_or("").trim().to_lowercase();
        if app != "plugable-chat.exe" {
            continue;
        }
        let (Some(code), Some(path)) = (get("Exception code:"), get("Faulting module path:")) else { continue };
        out.push(NativeFault { code, module_path: path, report_id: get("Report Id:").unwrap_or_default() });
    }
    out
}

/// The newest crash report not handled before. Windows only; other platforms have no such log.
pub fn newest_unseen_fault(seen: &[String]) -> Option<NativeFault> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("wevtutil")
            .args(["qe", "Application", "/q:*[System[Provider[@Name='Application Error']]]", "/c:10", "/rd:true", "/f:text"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        parse_application_errors(&text).into_iter().find(|f| !seen.contains(&f.report_id))
    }
    #[cfg(not(windows))]
    {
        let _ = seen;
        None
    }
}

// ---------- the verified marker ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerCheck {
    /// No provider files at all: nothing to protect.
    NoEp,
    /// Files exist but were never verified by a finished chat turn.
    Unverified,
    Valid,
    /// Files differ from what was verified: truncated, replaced or corrupt.
    Mismatch,
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn sample_hash(path: &Path, len: u64) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = 0xcbf29ce484222325u64;
    let mut buf = vec![0u8; SAMPLE.min(len) as usize];
    f.read_exact(&mut buf).ok()?;
    h = fnv(h, &buf);
    if len > SAMPLE {
        f.seek(SeekFrom::Start(len.saturating_sub(SAMPLE))).ok()?;
        let mut tail = vec![0u8; SAMPLE.min(len) as usize];
        f.read_exact(&mut tail).ok()?;
        h = fnv(h, &tail);
    }
    Some(h)
}

/// relative path -> (size, head/tail checksum) of every library-like file under `dir`.
pub fn fingerprint(dir: &Path) -> BTreeMap<String, (u64, u64)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, u64)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| FINGERPRINTED.contains(&x.to_lowercase().as_str()))
                .unwrap_or(false)
            {
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                // an unreadable file gets checksum 0 so it can never match a good marker
                out.insert(rel, (len, sample_hash(&p, len).unwrap_or(0)));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

pub fn write_marker(dir: &Path) {
    let fp = fingerprint(dir);
    if fp.is_empty() {
        return;
    }
    if let Ok(json) = serde_json::to_string(&fp) {
        let _ = std::fs::write(dir.join(MARKER_FILE), json);
    }
}

pub fn check_marker(dir: &Path) -> MarkerCheck {
    let current = fingerprint(dir);
    if current.is_empty() {
        return MarkerCheck::NoEp;
    }
    let Some(saved) = std::fs::read_to_string(dir.join(MARKER_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<BTreeMap<String, (u64, u64)>>(&s).ok())
    else {
        return MarkerCheck::Unverified;
    };
    // New files are fine (a provider added later); a verified file that changed or vanished is not.
    if saved.iter().all(|(k, v)| current.get(k) == Some(v)) {
        MarkerCheck::Valid
    } else {
        MarkerCheck::Mismatch
    }
}

// ---------- quarantine ----------

/// Renames `target` to a sibling `<name>.quarantine-<stamp>` so Core downloads it again. Nothing
/// is deleted: a support person can still look at the damaged files. Returns the new path.
pub fn quarantine(target: &Path, stamp: &str) -> std::io::Result<Option<PathBuf>> {
    if !target.exists() {
        return Ok(None);
    }
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "ep".into());
    let dest = target.with_file_name(format!("{name}.quarantine-{stamp}"));
    std::fs::rename(target, &dest)?;
    Ok(Some(dest))
}

// ---------- the launch decision ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineScope {
    /// One provider folder (the one the crash named).
    Provider(String),
    /// Everything under `ep/` (cause unknown, or a marker mismatch).
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchPlan {
    pub quarantine: Option<QuarantineScope>,
    pub gpu_off: bool,
    /// The death was the engine's fault, not a model's: do not count it against the model.
    pub spare_the_model: bool,
    pub notice: Option<String>,
}

/// Decides what to do about the last run's death. Mutates `health` (death count, GPU-off).
pub fn decide_launch(last: &LastRun, fault: Option<&NativeFault>, marker: &MarkerCheck, health: &mut EngineHealth) -> LaunchPlan {
    let died = !matches!(last, LastRun::Clean);
    if died {
        health.unclean_deaths += 1;
    }
    let site = fault.map(attribute);
    let ep_fault = matches!(site, Some(FaultSite::Ep { .. }));
    let mut plan = LaunchPlan::default();

    if *marker == MarkerCheck::Mismatch {
        plan.quarantine = Some(QuarantineScope::All);
        plan.notice = Some("Plugable Chat found that its GPU components were damaged (they changed since they last worked). They were set aside and will be downloaded again.".into());
    } else if died && (ep_fault || (fault.is_none() && *marker == MarkerCheck::Unverified)) {
        plan.quarantine = Some(match &site {
            Some(FaultSite::Ep { provider: Some(p) }) => QuarantineScope::Provider(p.clone()),
            _ => QuarantineScope::All,
        });
        plan.notice = Some("Plugable Chat closed unexpectedly while using its GPU components. They were set aside and will be downloaded again (a CPU model is used until then if you are offline).".into());
    }
    plan.spare_the_model = ep_fault || plan.quarantine.is_some();

    if health.unclean_deaths >= DEATHS_BEFORE_GPU_OFF && !health.gpu_off {
        health.gpu_off = true;
        health.gpu_off_reason = Some("Plugable Chat closed unexpectedly twice in a row.".into());
        plan.spare_the_model = true;
        plan.notice = Some("Plugable Chat closed unexpectedly twice in a row, so GPU acceleration is switched off and a CPU model is used. Use \"Try GPU again\" in Settings > Models when you want to retry.".into());
    }
    plan.gpu_off = health.gpu_off;
    plan
}

/// A chat turn finished: the engine files on disk work. Forgets earlier deaths and records the files.
pub fn note_good_session_in(health_dir: &Path, ep: Option<&Path>) {
    let mut h = health_in(health_dir);
    if h.unclean_deaths != 0 {
        h.unclean_deaths = 0;
        save_health_in(health_dir, &h);
    }
    if let Some(ep) = ep {
        write_marker(ep);
    }
}

pub fn note_good_session() {
    note_good_session_in(&crate::paths::get_config_dir(), ep_root().as_deref());
}

fn stamp() -> String {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string()
}

/// Carries out a plan's quarantine under `ep`. Returns a log line.
pub fn apply_quarantine(ep: &Path, scope: &QuarantineScope) -> String {
    let target = match scope {
        QuarantineScope::Provider(p) => ep.join(p),
        QuarantineScope::All => ep.to_path_buf(),
    };
    match quarantine(&target, &stamp()) {
        Ok(Some(to)) => format!("quarantined {} -> {}", target.display(), to.display()),
        Ok(None) => format!("nothing to quarantine at {}", target.display()),
        Err(e) => format!("could not quarantine {}: {e}", target.display()),
    }
}

/// A crash blocklist entry written for "a crash inside the GPU runtime" belongs to the engine's
/// files, not the model; clear those when the files are replaced.
pub fn clear_engine_blame(settings: &mut crate::settings::AppSettings) {
    let blamed: Vec<String> = settings
        .incompatible_model_reasons
        .iter()
        .filter(|(_, r)| r.contains("crash inside the GPU runtime"))
        .map(|(m, _)| m.clone())
        .collect();
    for m in blamed {
        settings.incompatible_models.remove(&m);
        settings.incompatible_model_reasons.remove(&m);
    }
    settings.native_crash_counts.clear();
}

/// Applies the launch decision to the disk, the health file and the settings. Returns the notice.
pub fn apply_at_launch(last: &LastRun, settings: &mut crate::settings::AppSettings) -> LaunchPlan {
    let dir = crate::paths::get_config_dir();
    let mut health = health_in(&dir);
    let ep = ep_root();
    let marker = ep.as_deref().map(check_marker).unwrap_or(MarkerCheck::NoEp);
    let marker = if test_fault("ep-mismatch") { MarkerCheck::Mismatch } else { marker };
    let fault = if test_fault("ep-fault") && !matches!(last, LastRun::Clean) {
        Some(NativeFault { code: "0xc0000409".into(), module_path: "C:\\Users\\test\\.plugable-chat\\ep\\webgpu-ep\\onnxruntime_providers_webgpu.dll".into(), report_id: format!("test-{}", stamp()) })
    } else if !matches!(last, LastRun::Clean) {
        newest_unseen_fault(&health.seen_report_ids)
    } else {
        None
    };
    if let Some(f) = &fault {
        health.seen_report_ids.push(f.report_id.clone());
        let n = health.seen_report_ids.len();
        if n > 20 {
            health.seen_report_ids.drain(..n - 20);
        }
        crate::app_log::log("crash", &format!("native fault {} in {}", f.code, f.module_path));
    }
    let plan = decide_launch(last, fault.as_ref(), &marker, &mut health);
    if let (Some(scope), Some(ep)) = (&plan.quarantine, ep.as_deref()) {
        let line = apply_quarantine(ep, scope);
        crate::app_log::log("crash", &line);
        health.last_quarantine = Some(line);
        clear_engine_blame(settings);
    }
    save_health_in(&dir, &health);
    set_gpu_off(plan.gpu_off);
    plan
}

/// Settings > Models > "Repair": set the engine's downloaded parts aside, forget crash history and
/// the GPU-off switch, and clear engine-blamed model blocks. Models themselves are left alone.
pub fn repair(settings: &mut crate::settings::AppSettings) -> String {
    let dir = crate::paths::get_config_dir();
    let mut health = health_in(&dir);
    let mut lines = Vec::new();
    if let Some(ep) = ep_root() {
        lines.push(apply_quarantine(&ep, &QuarantineScope::All));
    }
    health.unclean_deaths = 0;
    health.gpu_off = false;
    health.gpu_off_reason = None;
    save_health_in(&dir, &health);
    set_gpu_off(false);
    clear_engine_blame(settings);
    lines.push("crash history, GPU-off switch and engine-blamed model blocks cleared".into());
    lines.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-enginehealth-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ep_fault() -> NativeFault {
        NativeFault {
            code: "0xc0000409".into(),
            module_path: "C:\\Users\\washb\\.plugable-chat\\ep\\webgpu-ep\\onnxruntime_providers_webgpu.dll".into(),
            report_id: "3411".into(),
        }
    }

    const EVENT_TEXT: &str = "Event[0]:\n  Log Name: Application\n  Source: Application Error\n  Description:\nFaulting application name: plugable-chat.exe, version: 0.1.0.0, time stamp: 0x6ad9c769\nFaulting module name: onnxruntime_providers_webgpu.dll, version: 1.26.26.522, time stamp: 0x6a10aa5e\nException code: 0xc0000409\nFault offset: 0x0000000000382825\nFaulting module path: C:\\Users\\washb\\.plugable-chat\\ep\\webgpu-ep\\onnxruntime_providers_webgpu.dll\nReport Id: 3411f1d0-1f8d-42f3-997a-70640d273a61\n\nEvent[1]:\n  Source: Application Error\n  Description:\nFaulting application name: notepad.exe, version: 1\nException code: 0xc0000005\nFaulting module path: C:\\Windows\\x.dll\nReport Id: other\n";

    #[test]
    fn the_application_log_is_parsed_and_other_programs_are_ignored() {
        let faults = parse_application_errors(EVENT_TEXT);
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].code, "0xc0000409");
        assert!(faults[0].module_path.ends_with("onnxruntime_providers_webgpu.dll"));
        assert!(faults[0].report_id.starts_with("3411f1d0"));
    }

    #[test]
    fn a_dll_under_the_ep_folder_is_the_providers_fault() {
        assert_eq!(attribute(&ep_fault()), FaultSite::Ep { provider: Some("webgpu-ep".into()) });
        let other = NativeFault { module_path: "C:\\Program Files\\plugable-chat\\foundry-libs\\onnxruntime.dll".into(), ..ep_fault() };
        assert_eq!(attribute(&other), FaultSite::Other);
    }

    #[test]
    fn marker_is_absent_valid_or_mismatched() {
        let d = temp("marker");
        assert_eq!(check_marker(&d), MarkerCheck::NoEp);
        std::fs::create_dir_all(d.join("webgpu-ep")).unwrap();
        let dll = d.join("webgpu-ep").join("onnxruntime_providers_webgpu.dll");
        std::fs::write(&dll, vec![7u8; 200_000]).unwrap();
        assert_eq!(check_marker(&d), MarkerCheck::Unverified);
        write_marker(&d);
        assert_eq!(check_marker(&d), MarkerCheck::Valid);
        // bookkeeping files and new providers do not upset a verified marker
        std::fs::write(d.join("state.json"), "x").unwrap();
        std::fs::write(d.join("new.dll"), "y").unwrap();
        assert_eq!(check_marker(&d), MarkerCheck::Valid);
        // a truncated dll (a download cut off by a network drop) does
        std::fs::write(&dll, vec![7u8; 1000]).unwrap();
        assert_eq!(check_marker(&d), MarkerCheck::Mismatch);
    }

    #[test]
    fn a_same_size_corruption_in_the_tail_is_caught() {
        let d = temp("tail");
        let dll = d.join("a.dll");
        let mut bytes = vec![1u8; 300_000];
        std::fs::write(&dll, &bytes).unwrap();
        write_marker(&d);
        *bytes.last_mut().unwrap() = 9;
        std::fs::write(&dll, &bytes).unwrap();
        assert_eq!(check_marker(&d), MarkerCheck::Mismatch);
    }

    #[test]
    fn quarantine_renames_and_never_deletes() {
        let d = temp("q");
        let ep = d.join("ep");
        std::fs::create_dir_all(ep.join("webgpu-ep")).unwrap();
        std::fs::write(ep.join("webgpu-ep").join("a.dll"), "x").unwrap();
        let to = quarantine(&ep.join("webgpu-ep"), "42").unwrap().unwrap();
        assert!(!ep.join("webgpu-ep").exists());
        assert!(to.join("a.dll").exists());
        assert_eq!(quarantine(&ep.join("webgpu-ep"), "43").unwrap(), None);
    }

    #[test]
    fn a_clean_launch_with_good_files_changes_nothing() {
        let mut h = EngineHealth::default();
        let plan = decide_launch(&LastRun::Clean, None, &MarkerCheck::Valid, &mut h);
        assert_eq!(plan, LaunchPlan::default());
        assert_eq!(h.unclean_deaths, 0);
    }

    #[test]
    fn a_death_in_a_provider_dll_quarantines_that_provider_and_spares_the_model() {
        let mut h = EngineHealth::default();
        let last = LastRun::DiedIdle;
        let plan = decide_launch(&last, Some(&ep_fault()), &MarkerCheck::Valid, &mut h);
        assert_eq!(plan.quarantine, Some(QuarantineScope::Provider("webgpu-ep".into())));
        assert!(plan.spare_the_model && !plan.gpu_off);
        assert!(plan.notice.unwrap().contains("set aside"));
    }

    #[test]
    fn a_death_on_files_never_verified_quarantines_everything() {
        let mut h = EngineHealth::default();
        let plan = decide_launch(&LastRun::DiedDuringStartup, None, &MarkerCheck::Unverified, &mut h);
        assert_eq!(plan.quarantine, Some(QuarantineScope::All));
    }

    #[test]
    fn a_death_on_verified_files_with_no_named_module_only_counts() {
        let mut h = EngineHealth::default();
        let plan = decide_launch(&LastRun::DiedIdle, None, &MarkerCheck::Valid, &mut h);
        assert_eq!(plan.quarantine, None);
        assert_eq!(h.unclean_deaths, 1);
        assert!(!plan.gpu_off && !plan.spare_the_model);
    }

    #[test]
    fn damaged_files_are_quarantined_even_after_a_clean_exit() {
        let mut h = EngineHealth::default();
        let plan = decide_launch(&LastRun::Clean, None, &MarkerCheck::Mismatch, &mut h);
        assert_eq!(plan.quarantine, Some(QuarantineScope::All));
    }

    #[test]
    fn the_second_death_in_a_row_turns_the_gpu_off_and_it_stays_off() {
        let mut h = EngineHealth::default();
        decide_launch(&LastRun::DiedIdle, None, &MarkerCheck::Valid, &mut h);
        let plan = decide_launch(&LastRun::DiedIdle, None, &MarkerCheck::Valid, &mut h);
        assert!(plan.gpu_off && plan.spare_the_model && h.gpu_off);
        assert!(plan.notice.unwrap().contains("Try GPU again"));
        // a later clean launch keeps it off until the user (or Repair) turns it on
        let later = decide_launch(&LastRun::Clean, None, &MarkerCheck::Valid, &mut h);
        assert!(later.gpu_off);
    }

    #[test]
    fn a_good_session_between_deaths_resets_the_count() {
        let d = temp("good");
        let mut h = EngineHealth::default();
        decide_launch(&LastRun::DiedIdle, None, &MarkerCheck::Valid, &mut h);
        save_health_in(&d, &h);
        note_good_session_in(&d, None);
        let mut h = health_in(&d);
        assert_eq!(h.unclean_deaths, 0);
        let plan = decide_launch(&LastRun::DiedIdle, None, &MarkerCheck::Valid, &mut h);
        assert!(!plan.gpu_off);
    }

    #[test]
    fn a_good_session_records_the_files_it_worked_with() {
        let d = temp("good2");
        let ep = d.join("ep");
        std::fs::create_dir_all(&ep).unwrap();
        std::fs::write(ep.join("p.dll"), "abc").unwrap();
        assert_eq!(check_marker(&ep), MarkerCheck::Unverified);
        note_good_session_in(&d, Some(&ep));
        assert_eq!(check_marker(&ep), MarkerCheck::Valid);
    }

    #[test]
    fn engine_blamed_model_blocks_are_cleared_but_real_incompatibilities_stay() {
        let mut st = crate::settings::AppSettings::default();
        st.incompatible_models.insert("a".into(), "k".into());
        st.incompatible_model_reasons.insert("a".into(), "It closed Plugable Chat unexpectedly 2 times while answering (a crash inside the GPU runtime).".into());
        st.incompatible_models.insert("b".into(), "k".into());
        st.incompatible_model_reasons.insert("b".into(), "does not load".into());
        st.native_crash_counts.insert("a".into(), 1);
        clear_engine_blame(&mut st);
        assert!(!st.incompatible_models.contains_key("a"));
        assert!(st.incompatible_models.contains_key("b"));
        assert!(st.native_crash_counts.is_empty());
    }

    #[test]
    fn health_round_trips_and_garbage_reads_as_default() {
        let d = temp("health");
        assert_eq!(health_in(&d), EngineHealth::default());
        let h = EngineHealth { unclean_deaths: 1, gpu_off: true, ..Default::default() };
        save_health_in(&d, &h);
        assert_eq!(health_in(&d), h);
        std::fs::write(d.join(HEALTH_FILE), "{nope").unwrap();
        assert_eq!(health_in(&d), EngineHealth::default());
    }
}
