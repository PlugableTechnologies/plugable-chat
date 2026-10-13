//! Looks at the machine before and during start-up and keeps the answers, so a failure can be
//! explained from facts instead of guesses.
//!
//! The model engine decides on its own whether a GPU is usable and never says why when it
//! decides "no". This module asks Windows independently (display adapters, Thunderbolt devices,
//! `nvidia-smi`, the Visual C++ runtime, disk and RAM) and remembers what the engine itself
//! answered, so both views land in one report that "Copy details for support" can attach.
//!
//! Nothing here caches a failure: each refresh asks again, because a driver, cable or runtime
//! may have changed since the last look.

use crate::gpu_assessment::{self, Adapter, AdapterScan, CatalogCounts, EpDiscovery, EpObservation, GpuAssessment};
use crate::process_utils::HideConsoleWindow;
use serde::Serialize;
use std::sync::Mutex;

pub const TEST_ADAPTERS_ENV: &str = "PLUGABLE_CHAT_TEST_ADAPTERS";
pub const TEST_EPS_ENV: &str = "PLUGABLE_CHAT_TEST_EPS";
pub const TEST_NVIDIA_SMI_ENV: &str = "PLUGABLE_CHAT_TEST_NVIDIA_SMI";
/// Provider list the engine "reports" once the user has pressed Retry (staging "works second time").
pub const TEST_EPS_AFTER_RETRY_ENV: &str = "PLUGABLE_CHAT_TEST_EPS_AFTER_RETRY";
/// Adapter list that "appears" `PLUGABLE_CHAT_TEST_ADAPTERS_LATE_AFTER_SECS` after launch (an eGPU
/// that Windows enumerates late).
pub const TEST_ADAPTERS_LATE_ENV: &str = "PLUGABLE_CHAT_TEST_ADAPTERS_LATE";
pub const TEST_ADAPTERS_LATE_AFTER_ENV: &str = "PLUGABLE_CHAT_TEST_ADAPTERS_LATE_AFTER_SECS";

static RETRY_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called when the user (or the hardware watcher) asks for another GPU setup attempt.
pub fn mark_retry_requested() {
    RETRY_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn process_age_secs() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs()
}

/// Start the clock used by the late-adapter seam; call once at launch.
pub fn note_process_start() {
    let _ = process_age_secs();
}

/// The injection seams only work when the unattended-test switch is also set, so a stray
/// variable on a user's machine can never change what the shipped app reports.
fn seam(var: &str) -> Option<String> {
    std::env::var_os(crate::test_state::ENV_VAR).filter(|v| !v.is_empty())?;
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

// ---------------------------------------------------------------------------------------------
// Adapter scan
// ---------------------------------------------------------------------------------------------

/// PowerShell prints a single object as an object and several as an array; accept both.
fn as_vec(v: &serde_json::Value) -> Vec<serde_json::Value> {
    match v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
}

/// Parse the JSON the scan script prints: `{video:[...], thunderbolt:[...]}`.
pub fn parse_scan_json(text: &str) -> AdapterScan {
    let Some(start) = text.find('{') else {
        return AdapterScan { error: Some(format!("the adapter scan printed no data: {}", text.trim().chars().take(200).collect::<String>())), ..Default::default() };
    };
    let value: serde_json::Value = match serde_json::from_str(&text[start..]) {
        Ok(v) => v,
        Err(e) => return AdapterScan { error: Some(format!("the adapter scan printed unreadable data: {e}")), ..Default::default() },
    };
    let adapters = as_vec(value.get("video").unwrap_or(&serde_json::Value::Null))
        .iter()
        .map(|a| Adapter {
            name: str_field(a, "Name"),
            vendor: str_field(a, "AdapterCompatibility"),
            // AdapterRAM is a 32-bit field in WMI: it caps at 4 GB and can be negative or absent.
            vram_mb: a.get("AdapterRAM").and_then(|x| x.as_u64()).map(|b| b / (1024 * 1024)).unwrap_or(0),
            pnp_id: str_field(a, "PNPDeviceID"),
            status: str_field(a, "Status"),
            problem_code: a.get("ConfigManagerErrorCode").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        })
        .filter(|a| !a.name.is_empty())
        .collect();
    let thunderbolt_devices = as_vec(value.get("thunderbolt").unwrap_or(&serde_json::Value::Null))
        .iter()
        .filter_map(|t| t.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from))
        .collect();
    AdapterScan { adapters, thunderbolt_devices, error: None }
}

#[cfg(windows)]
const SCAN_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$video = @(Get-CimInstance Win32_VideoController | Select-Object Name, AdapterCompatibility, AdapterRAM, PNPDeviceID, Status, ConfigManagerErrorCode)
$tb = @(Get-CimInstance Win32_PnPEntity | Where-Object { $_.Name -match 'Thunderbolt' } | ForEach-Object { $_.Name })
[pscustomobject]@{ video = $video; thunderbolt = $tb } | ConvertTo-Json -Depth 4 -Compress
"#;

/// Ask Windows which display adapters exist, independent of the model engine.
pub fn scan_adapters() -> AdapterScan {
    if let (Some(json), Some(after)) = (seam(TEST_ADAPTERS_LATE_ENV), seam(TEST_ADAPTERS_LATE_AFTER_ENV)) {
        if process_age_secs() >= after.trim().parse::<u64>().unwrap_or(0) {
            return serde_json::from_str::<AdapterScan>(&json)
                .unwrap_or_else(|e| AdapterScan { error: Some(format!("{TEST_ADAPTERS_LATE_ENV} is not valid: {e}")), ..Default::default() });
        }
    }
    if let Some(json) = seam(TEST_ADAPTERS_ENV) {
        return match serde_json::from_str::<AdapterScan>(&json) {
            Ok(s) => s,
            Err(e) => AdapterScan { error: Some(format!("{TEST_ADAPTERS_ENV} is not valid: {e}")), ..Default::default() },
        };
    }
    scan_adapters_os()
}

#[cfg(windows)]
fn scan_adapters_os() -> AdapterScan {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCAN_SCRIPT])
        .hide_console_window()
        .output();
    match out {
        Ok(o) if o.status.success() => parse_scan_json(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => AdapterScan {
            error: Some(format!("PowerShell exited with {}: {}", o.status.code().map(|c| c.to_string()).unwrap_or_else(|| "no code".into()), String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>())),
            ..Default::default()
        },
        Err(e) => AdapterScan { error: Some(format!("could not start PowerShell: {e}")), ..Default::default() },
    }
}

#[cfg(not(windows))]
fn scan_adapters_os() -> AdapterScan {
    AdapterScan { error: Some("display adapters are only listed on Windows".to_string()), ..Default::default() }
}

// ---------------------------------------------------------------------------------------------
// What the engine and the catalog said
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Observed {
    scan: Option<AdapterScan>,
    eps: Option<EpDiscovery>,
    catalog: Option<Result<CatalogCounts, String>>,
    /// `(driver version, nvidia-smi detail)`: the detail keeps the exit code and stderr.
    nvidia_smi: Option<(Option<String>, String)>,
    vcpp: Option<bool>,
}

fn observed() -> &'static Mutex<Observed> {
    static OBS: std::sync::OnceLock<Mutex<Observed>> = std::sync::OnceLock::new();
    OBS.get_or_init(Default::default)
}

fn with_observed<T>(f: impl FnOnce(&mut Observed) -> T) -> T {
    let mut g = observed().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

/// Record what `discover_eps` returned. Called from every place the engine is asked, so the raw
/// answer is never lost to an early return.
pub fn record_eps(result: &EpDiscovery) {
    match result {
        Ok(list) => crate::app_log::log(
            "gpu",
            &format!("engine providers: {}", if list.is_empty() { "(none)".to_string() } else { list.iter().map(|e| format!("{}={}", e.name, if e.is_registered { "registered" } else { "not registered" })).collect::<Vec<_>>().join(", ") }),
        ),
        Err(e) => crate::app_log::log("gpu", &format!("engine could not list providers: {e}")),
    }
    with_observed(|o| o.eps = Some(result.clone()));
}

pub fn record_catalog(result: &Result<CatalogCounts, String>) {
    match result {
        Ok(c) => crate::app_log::log("gpu", &format!("model list: {} GPU, {} CPU", c.gpu, c.cpu)),
        Err(e) => crate::app_log::log("gpu", &format!("model list could not be read: {e}")),
    }
    with_observed(|o| o.catalog = Some(result.clone()));
    // The list is what turns "providers registered" into "GPU models ready" (or into "the list
    // could not be read"), so the UI hears about it now rather than at the next refresh.
    let _ = assessment_now();
}

pub fn eps_seam_active() -> bool {
    seam(TEST_EPS_ENV).is_some()
}

fn eps_seam_json() -> Option<String> {
    if RETRY_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
        if let Some(after) = seam(TEST_EPS_AFTER_RETRY_ENV) {
            return Some(after);
        }
    }
    seam(TEST_EPS_ENV)
}

/// Why the last model-list read failed, if it did. `get_catalog_models` returns a plain list, so
/// without this a failed read and an empty catalog look identical to the UI.
pub fn last_catalog_error() -> Option<String> {
    with_observed(|o| match &o.catalog {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    })
}

/// The engine's provider list with the test override applied. `real` runs only when no override
/// is set, so unattended tests can stage "driver present, engine reports only CPU".
pub fn discover_eps_with_seam(real: impl FnOnce() -> EpDiscovery) -> EpDiscovery {
    let result = match eps_seam_json() {
        Some(json) => serde_json::from_str::<Vec<EpObservation>>(&json).map_err(|e| format!("{TEST_EPS_ENV} is not valid: {e}")),
        None => real(),
    };
    record_eps(&result);
    result
}

// ---------------------------------------------------------------------------------------------
// Host facts
// ---------------------------------------------------------------------------------------------

/// `(version, detail)`. The detail always says what happened, including exit code and stderr, so
/// "not detected" can be told apart from "nvidia-smi crashed" or "not on PATH".
pub fn probe_nvidia_smi() -> (Option<String>, String) {
    if let Some(v) = seam(TEST_NVIDIA_SMI_ENV) {
        let version = crate::gpu_diagnostics::parse_nvidia_smi_driver(&v);
        return (version.clone(), format!("test override: {}", version.as_deref().unwrap_or("no driver")));
    }
    match std::process::Command::new("nvidia-smi").args(["--query-gpu=name,driver_version", "--format=csv,noheader"]).hide_console_window().output() {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // "NVIDIA GeForce RTX 4090, 572.16" -> take the last field of the first line.
            let version = text.lines().next().and_then(|l| l.rsplit(',').next()).map(str::trim).filter(|v| v.chars().next().map_or(false, |c| c.is_ascii_digit())).map(String::from);
            (version, format!("ok: {text}"))
        }
        Ok(o) => (None, format!("exit {:?}: {}", o.status.code(), String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>())),
        Err(e) => (None, format!("could not run nvidia-smi: {e}")),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderFacts {
    pub path: String,
    pub writable: bool,
    pub free_mb: Option<u64>,
}

/// True when a file can be created and removed in `dir` (creating the directory if needed).
pub fn dir_is_writable(dir: &std::path::Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(windows)]
fn free_disk_mb(dir: &std::path::Path) -> Option<u64> {
    // `dir` may not exist yet; ask about the drive instead.
    let drive = dir.components().next()?.as_os_str().to_string_lossy().trim_end_matches('\\').to_string();
    let script = format!("(Get-PSDrive -Name '{}' ).Free", drive.trim_end_matches(':'));
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).hide_console_window().output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok().map(|b| b / (1024 * 1024))
}

#[cfg(not(windows))]
fn free_disk_mb(_dir: &std::path::Path) -> Option<u64> {
    None
}

fn folder_facts(path: std::path::PathBuf) -> FolderFacts {
    FolderFacts { writable: dir_is_writable(&path), free_mb: free_disk_mb(&path), path: path.display().to_string() }
}

#[cfg(windows)]
fn ram_and_os() -> (Option<u64>, String) {
    let script = "$o=Get-CimInstance Win32_OperatingSystem; $c=Get-CimInstance Win32_ComputerSystem; \"$($o.Caption) build $($o.BuildNumber)|$([math]::Round($c.TotalPhysicalMemory/1MB))\"";
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).hide_console_window().output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let mut parts = text.splitn(2, '|');
            let os = parts.next().unwrap_or("").to_string();
            (parts.next().and_then(|r| r.trim().parse().ok()), if os.is_empty() { "Windows (version unknown)".into() } else { os })
        }
        Err(_) => (None, "Windows (version unknown)".into()),
    }
}

#[cfg(not(windows))]
fn ram_and_os() -> (Option<u64>, String) {
    (None, format!("{} {}", std::env::consts::OS, std::env::consts::ARCH))
}

/// Native libraries next to the engine that GPU providers load. Presence only: a file that is
/// there can still fail to load, and the engine's own error says so.
pub fn find_provider_dlls(dir: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let l = name.to_ascii_lowercase();
        if l.ends_with(".dll") && (l.starts_with("onnxruntime") || l.contains("cudart") || l.contains("cublas") || l.contains("cudnn")) {
            found.push(name);
        }
    }
    found.sort();
    found
}

// ---------------------------------------------------------------------------------------------
// Report and assessment
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub version: crate::version_info::VersionInfo,
    pub os: String,
    pub ram_mb: Option<u64>,
    pub config_dir: FolderFacts,
    pub data_dir: FolderFacts,
    pub adapters: AdapterScan,
    pub nvidia_smi: String,
    pub vcpp_installed: Option<bool>,
    pub provider_dlls: Vec<String>,
    pub engine_providers: Option<EpDiscovery>,
    pub catalog: Option<Result<CatalogCounts, String>>,
    pub assessment: GpuAssessment,
    pub generated_unix: u64,
}

/// Re-ask Windows (never reusing a failed answer) and combine it with what the engine said.
pub fn refresh_blocking() -> GpuAssessment {
    let scan = scan_adapters();
    match &scan.error {
        Some(e) => crate::app_log::log("preflight", &format!("adapter scan failed: {e}")),
        None => crate::app_log::log(
            "preflight",
            &format!("adapter scan: {} adapter(s), {} Thunderbolt device(s)", scan.adapters.len(), scan.thunderbolt_devices.len()),
        ),
    }
    let (driver, smi_detail) = probe_nvidia_smi();
    let vcpp = crate::gpu_diagnostics::probe_environment().1;
    with_observed(|o| {
        o.scan = Some(scan);
        o.nvidia_smi = Some((driver, smi_detail));
        o.vcpp = vcpp;
    });
    assessment_now()
}

static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// Lets assessment changes reach the UI as `gpu-assessment` events.
pub fn set_app_handle(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

/// Write the assessment to the log and the unattended-test state file when it changes.
fn announce(a: &GpuAssessment) {
    static LAST: Mutex<Option<String>> = Mutex::new(None);
    let line = format!("{:?}: {}", a.state, a.headline);
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    if last.as_deref() != Some(line.as_str()) {
        crate::app_log::log("gpu", &format!("assessment {line}"));
        let status = if matches!(a.state, gpu_assessment::GpuStateKind::Ready | gpu_assessment::GpuStateKind::NoHardware) {
            crate::test_state::status::OK
        } else if matches!(a.state, gpu_assessment::GpuStateKind::Checking) {
            crate::test_state::status::PROGRESS
        } else {
            crate::test_state::status::FAILED
        };
        crate::test_state::record(crate::test_state::Phase::GpuAssessment, status, &format!("{:?} | {}", a.state, a.headline));
        *last = Some(line);
        if let Some(app) = APP.get() {
            use tauri::Emitter;
            let _ = app.emit("gpu-assessment", a);
        }
    }
}

pub fn assessment_now() -> GpuAssessment {
    let diag = crate::gpu_diagnostics::snapshot_without_probe();
    let failures: Vec<(String, String, String)> = diag
        .failed
        .iter()
        .map(|f| (f.ep.clone(), crate::actors::foundry::gpu_registration::kind_label(f.kind).to_string(), f.message.clone()))
        .collect();
    let a = with_observed(|o| {
        let driver = o.nvidia_smi.as_ref().and_then(|(d, _)| d.clone());
        gpu_assessment::assess(&gpu_assessment::AssessInputs {
            scan: o.scan.as_ref(),
            eps: o.eps.as_ref(),
            catalog: o.catalog.as_ref(),
            driver_version: driver.as_deref(),
            vcpp_installed: o.vcpp,
            failures: &failures,
        })
    });
    announce(&a);
    a
}

/// Adapter identities, for noticing that hardware appeared or went away between two scans.
/// `None` until a scan has run: "no adapters" and "never looked" must not compare equal.
pub fn adapter_fingerprint() -> Option<Vec<String>> {
    with_observed(|o| o.scan.as_ref().map(|s| s.adapters.iter().map(|a| format!("{}|{}|{}", a.name, a.pnp_id, a.problem_code)).collect()))
}

pub fn build_report_blocking() -> PreflightReport {
    let assessment = refresh_blocking();
    let (ram_mb, os) = ram_and_os();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let report = with_observed(|o| PreflightReport {
        version: crate::version_info::VersionInfo::current(),
        os,
        ram_mb,
        config_dir: folder_facts(crate::paths::get_config_dir()),
        data_dir: folder_facts(crate::paths::get_data_dir()),
        adapters: o.scan.clone().unwrap_or_default(),
        nvidia_smi: o.nvidia_smi.as_ref().map(|(_, d)| d.clone()).unwrap_or_default(),
        vcpp_installed: o.vcpp,
        provider_dlls: exe_dir.map(|d| find_provider_dlls(&d)).unwrap_or_default(),
        engine_providers: o.eps.clone(),
        catalog: o.catalog.clone(),
        assessment,
        generated_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    });
    write_report(&report);
    report
}

fn write_report(report: &PreflightReport) {
    let dir = crate::app_log::log_dir();
    let path = dir.join(format!("preflight-{}.json", report.generated_unix));
    let result = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, serde_json::to_string_pretty(report).unwrap_or_default()));
    match result {
        Ok(()) => {
            crate::app_log::log("preflight", &format!("{} -> {}", report.assessment.headline, path.display()));
            prune_reports(&dir, 5);
        }
        Err(e) => crate::app_log::log("preflight", &format!("could not write {}: {e}", path.display())),
    }
}

/// Keep the newest `keep` reports.
pub fn prune_reports(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("preflight-")).map(|e| e.path()).collect();
    files.sort();
    if files.len() > keep {
        for old in &files[..files.len() - keep] {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// Plain text for "Copy details for support".
pub fn format_report(r: &PreflightReport) -> String {
    let mut out = vec![
        "PLUGABLE CHAT SUPPORT DETAILS".to_string(),
        r.version.report_lines(),
        format!("OS: {}", r.os),
        format!("RAM: {}", r.ram_mb.map(|m| format!("{m} MB")).unwrap_or_else(|| "unknown".into())),
        format!("Config folder: {} (writable: {}, free: {})", r.config_dir.path, r.config_dir.writable, r.config_dir.free_mb.map(|m| format!("{m} MB")).unwrap_or_else(|| "unknown".into())),
        format!("Data folder: {} (writable: {}, free: {})", r.data_dir.path, r.data_dir.writable, r.data_dir.free_mb.map(|m| format!("{m} MB")).unwrap_or_else(|| "unknown".into())),
        String::new(),
        format!("GPU: {}", r.assessment.headline),
    ];
    out.extend(r.assessment.detail.iter().map(|d| format!("  {d}")));
    out.extend(r.assessment.hints.iter().map(|h| format!("  Try: {h}")));
    out.push(format!("nvidia-smi: {}", r.nvidia_smi));
    out.push(format!("Visual C++ runtime: {}", match r.vcpp_installed { Some(true) => "installed", Some(false) => "NOT FOUND", None => "not checked" }));
    out.push(format!("Provider libraries next to the app: {}", if r.provider_dlls.is_empty() { "none".to_string() } else { r.provider_dlls.join(", ") }));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_single_adapter_object_and_an_array() {
        let one = r#"{"video":{"Name":"NVIDIA RTX 4090","AdapterCompatibility":"NVIDIA","AdapterRAM":4293918720,"PNPDeviceID":"PCI\\VEN_10DE&DEV_2684","Status":"OK","ConfigManagerErrorCode":0},"thunderbolt":null}"#;
        let s = parse_scan_json(one);
        assert_eq!(s.adapters.len(), 1);
        assert!(s.adapters[0].is_nvidia());
        assert_eq!(s.adapters[0].vram_mb, 4095);
        let many = r#"{"video":[{"Name":"Intel UHD","AdapterCompatibility":"Intel"},{"Name":"NVIDIA RTX","AdapterCompatibility":"NVIDIA","ConfigManagerErrorCode":43}],"thunderbolt":["Intel Thunderbolt Controller"]}"#;
        let s = parse_scan_json(many);
        assert_eq!(s.adapters.len(), 2);
        assert_eq!(s.adapters[1].problem_code, 43);
        assert_eq!(s.thunderbolt_devices, vec!["Intel Thunderbolt Controller"]);
        assert!(s.error.is_none());
    }

    #[test]
    fn garbage_and_empty_output_are_scan_errors_not_empty_scans() {
        assert!(parse_scan_json("").error.is_some());
        assert!(parse_scan_json("Get-CimInstance : Access denied").error.is_some());
        assert!(parse_scan_json("{ not json").error.is_some());
    }

    #[test]
    fn leading_noise_before_the_json_is_tolerated() {
        let s = parse_scan_json("WARNING: profile loaded\r\n{\"video\":[],\"thunderbolt\":[]}");
        assert!(s.error.is_none() && s.adapters.is_empty());
    }

    // R3 (Pat): rebooting changed nothing, because an earlier failed look was reused. Every refresh
    // must ask again, and the seams must not exist outside an unattended test run.
    // One test, because it changes process-wide environment variables.
    #[test]
    fn r3_every_refresh_asks_again_and_seams_need_the_test_switch() {
        let nvidia = r#"{"adapters":[{"name":"NVIDIA GeForce RTX 4090","vendor":"NVIDIA","vramMb":4095,"pnpId":"PCI\\VEN_10DE","status":"OK","problemCode":0}],"thunderboltDevices":[]}"#;
        std::env::remove_var(crate::test_state::ENV_VAR);
        std::env::set_var(TEST_ADAPTERS_ENV, nvidia);
        assert!(seam(TEST_ADAPTERS_ENV).is_none(), "a stray variable must not change what the shipped app reports");
        let scan = scan_adapters();
        assert!(scan.nvidia().is_empty(), "the real scan must be used without the test switch");

        let state_file = std::env::temp_dir().join(format!("pc-seam-{}.jsonl", std::process::id()));
        std::env::set_var(crate::test_state::ENV_VAR, &state_file);
        std::env::set_var(TEST_NVIDIA_SMI_ENV, "572.16");
        std::env::set_var(TEST_EPS_ENV, r#"[{"name":"CPUExecutionProvider","isRegistered":true}]"#);
        let eps = discover_eps_with_seam(|| panic!("the engine must not be asked when a test list is staged"));
        assert_eq!(eps.unwrap().len(), 1);
        record_catalog(&Ok(CatalogCounts { gpu: 0, cpu: 5 }));

        let first = refresh_blocking();
        assert_eq!(first.state, gpu_assessment::GpuStateKind::HardwareNotUsed, "{first:?}");

        // the same process, but the adapter is now gone and the driver stops answering
        std::env::set_var(TEST_ADAPTERS_ENV, r#"{"adapters":[],"thunderboltDevices":[]}"#);
        std::env::remove_var(TEST_NVIDIA_SMI_ENV);
        let second = refresh_blocking();
        assert_eq!(second.state, gpu_assessment::GpuStateKind::NoHardware, "an earlier answer was reused: {second:?}");

        // and a broken scan is reported as a failed scan, not as "no hardware"
        std::env::set_var(TEST_ADAPTERS_ENV, "not json");
        let third = refresh_blocking();
        assert_eq!(third.state, gpu_assessment::GpuStateKind::ScanFailed, "{third:?}");

        for v in [TEST_ADAPTERS_ENV, TEST_EPS_ENV, TEST_NVIDIA_SMI_ENV, crate::test_state::ENV_VAR] {
            std::env::remove_var(v);
        }
        let _ = std::fs::remove_file(&state_file);
    }

    #[test]
    fn writable_dir_probe_detects_a_file_in_the_way() {
        let base = std::env::temp_dir().join(format!("pc-pre-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        assert!(dir_is_writable(&base.join("sub")));
        let blocker = base.join("file");
        std::fs::write(&blocker, b"x").unwrap();
        assert!(!dir_is_writable(&blocker.join("child")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn provider_dll_listing() {
        let d = std::env::temp_dir().join(format!("pc-dll-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for n in ["onnxruntime.dll", "cudart64_12.dll", "other.dll", "readme.txt"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        assert_eq!(find_provider_dlls(&d), vec!["cudart64_12.dll", "onnxruntime.dll"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_reports_are_pruned_newest_kept() {
        let d = std::env::temp_dir().join(format!("pc-prune-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for i in 1..=8 {
            std::fs::write(d.join(format!("preflight-{i:010}.json")), b"{}").unwrap();
        }
        prune_reports(&d, 5);
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(left.len(), 5);
        assert!(!left.contains(&"preflight-0000000001.json".to_string()));
        assert!(left.contains(&"preflight-0000000008.json".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
